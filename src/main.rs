use axum::{
    body::{Body, Bytes},
    extract::State,
    http::{header, HeaderMap, HeaderValue, Method, StatusCode, Uri},
    response::{IntoResponse, Response},
    routing::get,
    Router,
};
use futures_util::TryStreamExt;
use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::server::conn::auto;
use rcgen::generate_simple_self_signed;
use serde_json::Value;
use std::env;
use std::fs;
use std::io::{self, BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::thread;
use std::time::Duration;
use tokio_rustls::rustls::ServerConfig;
use tokio_rustls::TlsAcceptor;
use tower::Service;

/// Check if a given role name indicates a subagent.
pub fn is_subagent_role_name(r: &str) -> bool {
    let lower = r.to_lowercase();
    match lower.as_str() {
        "worker" | "explorer" | "default" | "subagent" | "sub-agent" | "sub_agent" | "reviewer" => {
            true
        }
        "user" | "assistant" | "system" | "main" | "primary" => false,
        _ => true,
    }
}

/// Locate the active Codex home directory (~/.codex).
pub fn get_codex_home_dir() -> Option<PathBuf> {
    if let Ok(codex_home) = env::var("CODEX_HOME") {
        let p = PathBuf::from(codex_home);
        if p.is_dir() {
            return Some(p);
        }
    }
    if let Ok(profile) = env::var("USERPROFILE") {
        let p = PathBuf::from(profile).join(".codex");
        if p.is_dir() {
            return Some(p);
        }
    }
    if let Ok(home) = env::var("HOME") {
        let p = PathBuf::from(home).join(".codex");
        if p.is_dir() {
            return Some(p);
        }
    }
    None
}

/// Locate the active Codex config.toml path.
pub fn get_config_path() -> Option<PathBuf> {
    let p = get_codex_home_dir()?.join("config.toml");
    if p.is_file() {
        Some(p)
    } else {
        None
    }
}

/// Parse key-value from a TOML line (stripping whitespace, quotes, and comments).
fn parse_toml_key_value(line: &str) -> Option<(String, String)> {
    let mut parts = line.splitn(2, '=');
    let k = parts.next()?.trim().to_string();
    let v_raw = parts.next()?.trim();
    let v_no_comment = v_raw.split('#').next()?.trim();
    let v = v_no_comment.trim_matches(|c| c == '"' || c == '\'').to_string();
    if !k.is_empty() && !v.is_empty() {
        Some((k, v))
    } else {
        None
    }
}

/// Parse top-level `model = "..."` from a role manifest file (`~/.codex/agents/<role>.toml`).
pub fn parse_model_from_role_toml(content: &str) -> Option<String> {
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('#') || trimmed.is_empty() {
            continue;
        }
        if trimmed.starts_with('[') {
            break;
        }
        if let Some((k, v)) = parse_toml_key_value(trimmed) {
            if k.eq_ignore_ascii_case("model") {
                return Some(v);
            }
        }
    }
    None
}

/// Parse `[subagent_models]` or `[agents].default_subagent_model` from TOML text.
pub fn parse_model_from_toml(content: &str, role: &str) -> Option<String> {
    let mut in_subagent_models = false;
    let mut in_agents = false;
    let mut default_subagent_model: Option<String> = None;
    let role_key = role.to_lowercase();

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('#') || trimmed.is_empty() {
            continue;
        }

        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            let section = trimmed.trim_matches(|c| c == '[' || c == ']').trim();
            if section.eq_ignore_ascii_case("subagent_models") {
                in_subagent_models = true;
                in_agents = false;
            } else if section.eq_ignore_ascii_case("agents") {
                in_agents = true;
                in_subagent_models = false;
            } else {
                in_subagent_models = false;
                in_agents = false;
            }
            continue;
        }

        if in_subagent_models {
            if let Some((k, v)) = parse_toml_key_value(trimmed) {
                if k.eq_ignore_ascii_case(&role_key) {
                    return Some(v);
                }
            }
        }

        if in_agents {
            if let Some((k, v)) = parse_toml_key_value(trimmed) {
                if k.eq_ignore_ascii_case("default_subagent_model") {
                    default_subagent_model = Some(v);
                }
            }
        }
    }

    default_subagent_model
}

/// Read model configuration for a specific role from `~/.codex/agents/<role>.toml` or `~/.codex/config.toml`.
pub fn read_model_from_config(role: &str) -> Option<String> {
    if let Some(codex_home) = get_codex_home_dir() {
        let role_file = codex_home
            .join("agents")
            .join(format!("{}.toml", role.to_lowercase()));
        if role_file.is_file() {
            if let Ok(content) = fs::read_to_string(&role_file) {
                if let Some(m) = parse_model_from_role_toml(&content) {
                    return Some(m);
                }
            }
        }
    }
    let config_path = get_config_path()?;
    let content = fs::read_to_string(&config_path).ok()?;
    parse_model_from_toml(&content, role)
}

/// Built-in default model name for a given role without consulting env/disk.
pub fn builtin_role_model(role: &str) -> &'static str {
    match role.to_lowercase().as_str() {
        "worker" => "implement",
        "explorer" => "explore",
        "reviewer" => "review",
        _ => "9router-subagent",
    }
}

/// Map an agent role to a specialized model name with multi-tier resolution:
/// 1. Environment variable override: `CODEX_<ROLE>_MODEL`
/// 2. Generic environment variable override: `CODEX_SUBAGENT_MODEL`
/// 3. Active `~/.codex/agents/<role>.toml` or `~/.codex/config.toml`
/// 4. Built-in defaults: worker -> implement, explorer -> explore, reviewer -> review, default -> 9router-subagent
pub fn map_role_to_model(role: Option<&str>) -> String {
    let role_str = role.unwrap_or("default");
    let role_lower = role_str.to_lowercase();

    // 1. Specific role env var: CODEX_WORKER_MODEL, CODEX_EXPLORER_MODEL, etc.
    let env_role = format!("CODEX_{}_MODEL", role_lower.to_uppercase());
    if let Ok(m) = env::var(&env_role) {
        let trimmed = m.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }

    // 2. Generic subagent model env var: CODEX_SUBAGENT_MODEL
    if let Ok(m) = env::var("CODEX_SUBAGENT_MODEL") {
        let trimmed = m.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }

    // 3. Read from ~/.codex/agents/<role>.toml or ~/.codex/config.toml
    if let Some(m) = read_model_from_config(&role_lower) {
        return m;
    }

    // 4. Built-in smart fallback defaults
    builtin_role_model(&role_lower).to_string()
}

/// Check if a JSON value contains markers indicating a subagent thread source.
pub fn contains_subagent_source(v: &Value) -> bool {
    match v {
        Value::String(s) => {
            let lower = s.to_lowercase();
            lower.contains("subagent") || lower.contains("sub_agent") || lower.contains("sub-agent")
        }
        Value::Object(map) => {
            if map.contains_key("subAgent")
                || map.contains_key("subagent")
                || map.contains_key("sub_agent")
                || map.contains_key("thread_spawn")
                || map.contains_key("threadSpawn")
            {
                return true;
            }
            if let Some(r) = map
                .get("agent_role")
                .or_else(|| map.get("agentRole"))
                .and_then(|x| x.as_str())
            {
                if is_subagent_role_name(r) {
                    return true;
                }
            }
            if let Some(src) = map.get("threadSource").or_else(|| map.get("thread_source")) {
                if contains_subagent_source(src) {
                    return true;
                }
            }
            false
        }
        _ => false,
    }
}

/// Retrieve the configured target model provider (defaults to "9router").
pub fn get_target_model_provider() -> String {
    env::var("CODEX_SUBAGENT_PROVIDER").unwrap_or_else(|_| "9router".to_string())
}

/// Automatically locate the real official codex executable.
pub fn find_real_codex() -> PathBuf {
    // 1. Explicit override via REAL_CODEX_PATH
    if let Ok(p) = env::var("REAL_CODEX_PATH") {
        let pb = PathBuf::from(p);
        if pb.is_file() {
            return pb;
        }
    }

    // 2. Check if codex.orig.exe or codex-9router-subagents.orig.exe exists in the directory of the current executable
    if let Ok(current_exe) = env::current_exe() {
        if let Some(parent) = current_exe.parent() {
            for candidate_name in ["codex.orig.exe", "codex-9router-subagents.orig.exe"] {
                let orig = parent.join(candidate_name);
                if orig.is_file() {
                    return orig;
                }
            }
        }
    }

    // 3. Scan bin/<hash> candidates in LOCALAPPDATA
    if let Ok(local_app_data) = env::var("LOCALAPPDATA") {
        let bin_dir = Path::new(&local_app_data)
            .join("OpenAI")
            .join("Codex")
            .join("bin");
        if let Ok(entries) = fs::read_dir(&bin_dir) {
            let mut candidates: Vec<PathBuf> = Vec::new();
            for entry in entries.flatten() {
                let orig = entry.path().join("codex.orig.exe");
                if orig.is_file() {
                    candidates.push(orig);
                } else {
                    let p = entry.path().join("codex.exe");
                    if p.is_file() {
                        if let Ok(cur) = env::current_exe() {
                            if let (Ok(c1), Ok(c2)) = (p.canonicalize(), cur.canonicalize()) {
                                if c1 == c2 {
                                    continue;
                                }
                            }
                        }
                        if p.metadata().map_or(false, |m| m.len() > 10_000_000) {
                            candidates.push(p);
                        }
                    }
                }
            }
            candidates.sort_by(|a, b| {
                let ma = a.metadata().and_then(|m| m.modified()).ok();
                let mb = b.metadata().and_then(|m| m.modified()).ok();
                mb.cmp(&ma)
            });
            if let Some(first) = candidates.into_iter().next() {
                return first;
            }
        }

        let custom_orig = Path::new(&local_app_data)
            .join("OpenAI")
            .join("Codex")
            .join("custom")
            .join("codex-9router-subagents.orig.exe");
        if custom_orig.is_file() {
            return custom_orig;
        }

        let custom_codex_orig = Path::new(&local_app_data)
            .join("OpenAI")
            .join("Codex")
            .join("custom")
            .join("codex.orig.exe");
        if custom_codex_orig.is_file() {
            return custom_codex_orig;
        }

        let app_bin = Path::new(&local_app_data)
            .join("Programs")
            .join("OpenAI")
            .join("Codex")
            .join("bin")
            .join("codex.exe");
        if app_bin.is_file() && app_bin.metadata().map_or(false, |m| m.len() > 10_000_000) {
            return app_bin;
        }
    }

    // 4. Check user home .codex package daemon releases
    if let Ok(userprofile) = env::var("USERPROFILE") {
        let releases_dir = Path::new(&userprofile)
            .join(".codex")
            .join("packages")
            .join("app-server-daemon")
            .join("releases");
        if let Ok(entries) = fs::read_dir(&releases_dir) {
            for entry in entries.flatten() {
                let orig = entry.path().join("bin").join("codex.orig.exe");
                if orig.is_file() {
                    return orig;
                }
            }
        }
    }

    PathBuf::from("codex.orig.exe")
}

/// Reset any rate limit errors or spend control blocks in local UI responses.
pub fn sanitize_rate_limits(val: &mut Value) -> bool {
    let mut modified = false;
    if let Some(obj) = val.as_object_mut() {
        if obj.contains_key("ordinaryUsageAllowed") {
            obj.insert("ordinaryUsageAllowed".to_string(), Value::Bool(true));
            modified = true;
        }
        if obj.contains_key("rateLimitUpsell") {
            obj.insert("rateLimitUpsell".to_string(), Value::Null);
            modified = true;
        }
        if let Some(rl) = obj.get_mut("rateLimits").and_then(|r| r.as_object_mut()) {
            rl.insert("rateLimitReachedType".to_string(), Value::Null);
            rl.insert("spendControlReached".to_string(), Value::Bool(false));
            if let Some(p) = rl.get_mut("primary").and_then(|p| p.as_object_mut()) {
                p.insert("usedPercent".to_string(), serde_json::json!(0));
            }
            if let Some(s) = rl.get_mut("secondary").and_then(|s| s.as_object_mut()) {
                s.insert("usedPercent".to_string(), serde_json::json!(0));
            }
            modified = true;
        }
        if let Some(rls) = obj
            .get_mut("rateLimitsByLimitId")
            .and_then(|r| r.as_object_mut())
        {
            for (_, item) in rls.iter_mut() {
                if let Some(vo) = item.as_object_mut() {
                    vo.insert("rateLimitReachedType".to_string(), Value::Null);
                    vo.insert("spendControlReached".to_string(), Value::Bool(false));
                    if let Some(p) = vo.get_mut("primary").and_then(|p| p.as_object_mut()) {
                        p.insert("usedPercent".to_string(), serde_json::json!(0));
                    }
                    if let Some(s) = vo.get_mut("secondary").and_then(|s| s.as_object_mut()) {
                        s.insert("usedPercent".to_string(), serde_json::json!(0));
                    }
                }
            }
            modified = true;
        }
        for (_, child) in obj.iter_mut() {
            if sanitize_rate_limits(child) {
                modified = true;
            }
        }
    } else if let Some(arr) = val.as_array_mut() {
        for child in arr.iter_mut() {
            if sanitize_rate_limits(child) {
                modified = true;
            }
        }
    }
    modified
}

/// Recursively search for an agent role in a JSON structure.
pub fn find_agent_role(v: &Value) -> Option<String> {
    match v {
        Value::Object(map) => {
            if let Some(r) = map
                .get("agentRole")
                .or_else(|| map.get("agent_role"))
                .or_else(|| map.get("agentType"))
                .or_else(|| map.get("agent_type"))
                .or_else(|| map.get("role"))
                .and_then(|x| x.as_str())
            {
                if !r.is_empty() {
                    return Some(r.to_string());
                }
            }
            for (_, child) in map.iter() {
                if let Some(r) = find_agent_role(child) {
                    return Some(r);
                }
            }
            None
        }
        Value::Array(arr) => {
            for child in arr {
                if let Some(r) = find_agent_role(child) {
                    return Some(r);
                }
            }
            None
        }
        _ => None,
    }
}

/// Inspect and rewrite thread/turn JSON-RPC parameters to route subagents to 9Router.
pub fn route_thread_params(params: &mut serde_json::Map<String, Value>) -> bool {
    let mut modified = false;
    let mut nested_subagent = false;
    let target_provider = get_target_model_provider();

    // Check nested objects
    let mut nested_model: Option<String> = None;
    for key in [
        "settings",
        "thread",
        "configuration",
        "config",
        "params",
        "threadSource",
        "thread_source",
        "thread_spawn",
    ] {
        if let Some(nested) = params.get_mut(key).and_then(|v| v.as_object_mut()) {
            if contains_subagent_source(&Value::Object(nested.clone())) {
                nested_subagent = true;
            }
            if route_thread_params(nested) {
                modified = true;
                nested_subagent = true;
                if let Some(m) = nested.get("model").and_then(|v| v.as_str()) {
                    if !m.is_empty() && is_subagent_model_name(m) {
                        nested_model = Some(m.to_string());
                    }
                }
            }
        }
    }

    if let Some(m) = nested_model {
        if !params.contains_key("model") || params.get("model").map_or(true, |v| v.is_null()) {
            params.insert("model".to_string(), Value::String(m));
            modified = true;
        }
    }

    // Check role / agent type across current map and nested trees
    let detected_role = find_agent_role(&Value::Object(params.clone()));
    let role = detected_role.as_deref();

    let role_is_subagent = if let Some(r) = role {
        is_subagent_role_name(r)
    } else {
        false
    };

    // Check agent nickname
    let has_agent_nickname = params
        .get("agentNickname")
        .or_else(|| params.get("agent_nickname"))
        .and_then(|v| v.as_str())
        .map_or(false, |s| !s.is_empty());

    // Check threadSource
    let source_is_subagent = params
        .get("threadSource")
        .or_else(|| params.get("thread_source"))
        .map_or(false, |v| contains_subagent_source(v));

    let is_subagent =
        role_is_subagent || has_agent_nickname || source_is_subagent || nested_subagent;

    let model_str = params
        .get("model")
        .and_then(|m| m.as_str())
        .map(|s| s.to_string());

    let is_9router_model = if let Some(ref m) = model_str {
        is_subagent_model_name(m)
    } else {
        false
    };

    if is_subagent || is_9router_model {
        let mapped_model = map_role_to_model(role);
        let needs_model = match params.get("model") {
            None => true,
            Some(Value::Null) => true,
            Some(Value::String(s)) => {
                s.is_empty()
                    || (is_subagent
                        && (s.starts_with("gpt-")
                            || s.starts_with("o1")
                            || s.starts_with("o3")
                            || s.starts_with("chatgpt")))
            }
            _ => false,
        };

        if needs_model {
            params.insert("model".to_string(), Value::String(mapped_model));
            modified = true;
        }
        if params.get("modelProvider").and_then(|v| v.as_str()) != Some(&target_provider) {
            params.insert(
                "modelProvider".to_string(),
                Value::String(target_provider.clone()),
            );
            modified = true;
        }
        if params.get("model_provider").and_then(|v| v.as_str()) != Some(&target_provider) {
            params.insert("model_provider".to_string(), Value::String(target_provider));
            modified = true;
        }
    } else if let Some(ref m) = model_str {
        if !m.is_empty() {
            if params.get("modelProvider").and_then(|v| v.as_str()) != Some("openai") {
                params.insert("modelProvider".to_string(), Value::String("openai".to_string()));
                modified = true;
            }
            if params.get("model_provider").and_then(|v| v.as_str()) != Some("openai") {
                params.insert("model_provider".to_string(), Value::String("openai".to_string()));
                modified = true;
            }
        }
    }

    modified
}

/// Probe a target endpoint URL to verify TCP socket connectivity.
pub fn probe_endpoint(endpoint: &str) {
    let clean = endpoint
        .trim_start_matches("http://")
        .trim_start_matches("https://");
    let host_port = clean.split('/').next().unwrap_or("127.0.0.1:20128");
    let target = if host_port.contains(':') {
        host_port.to_string()
    } else if endpoint.starts_with("https://") {
        format!("{}:443", host_port)
    } else {
        format!("{}:80", host_port)
    };

    match target.to_socket_addrs() {
        Ok(addrs) => {
            let mut connected = false;
            let mut last_err = String::from("No addresses available");
            for addr in addrs {
                match TcpStream::connect_timeout(&addr, Duration::from_millis(1500)) {
                    Ok(_) => {
                        println!("[OK] Endpoint socket reachable : {} ({})", target, addr);
                        connected = true;
                        break;
                    }
                    Err(e) => {
                        last_err = e.to_string();
                    }
                }
            }
            if !connected {
                println!(
                    "[FAIL] Endpoint socket unreachable: {} (error: {})",
                    target, last_err
                );
            }
        }
        Err(e) => println!("[FAIL] Invalid endpoint address  : {} ({})", target, e),
    }
}

/// Check if a given model string corresponds to a 9Router or subagent model.
pub fn is_subagent_model_name(m: &str) -> bool {
    let trimmed = m.trim();
    if trimmed.is_empty() {
        return false;
    }
    let lower = trimmed.to_lowercase();
    if lower == "9router-subagent"
        || lower.starts_with("9router")
        || lower == "implement"
        || lower == "explore"
        || lower == "review"
    {
        return true;
    }

    // Also match if user configured a custom model for any subagent role
    for role in ["default", "worker", "explorer", "reviewer"] {
        let configured = map_role_to_model(Some(role));
        if !configured.is_empty()
            && !configured.starts_with("gpt-")
            && !configured.starts_with("o1")
            && !configured.starts_with("o3")
            && !configured.starts_with("chatgpt")
            && lower == configured.to_lowercase()
        {
            return true;
        }
    }

    false
}

/// Check if a request path is an LLM responses endpoint.
pub fn is_responses_path(path: &str) -> bool {
    let clean_path = path.split('?').next().unwrap_or(path).trim_end_matches('/');
    clean_path == "/backend-api/codex/responses"
        || clean_path == "/backend-api/responses"
        || clean_path == "/codex/responses"
        || clean_path == "/responses"
        || clean_path.ends_with("/codex/responses")
        || clean_path.ends_with("/responses")
}

/// Retrieve the configured loopback proxy port (default: 20129).
pub fn get_proxy_port() -> String {
    env::var("CODEX_PROXY_PORT").unwrap_or_else(|_| "20129".to_string())
}

/// Retrieve the official ChatGPT upstream base URL (default: <https://chatgpt.com>).
pub fn get_chatgpt_upstream_base() -> String {
    env::var("CODEX_CHATGPT_UPSTREAM_URL").unwrap_or_else(|_| "https://chatgpt.com".to_string())
}

/// Retrieve the 9Router / subagent responses target endpoint.
pub fn get_subagent_responses_url() -> String {
    if let Ok(ep) = env::var("CODEX_SUBAGENT_ENDPOINT") {
        let trimmed = ep.trim().trim_end_matches('/');
        if !trimmed.is_empty() {
            if trimmed.ends_with("/responses") {
                return trimmed.to_string();
            }
            if trimmed.ends_with("/v1") {
                return format!("{}/responses", trimmed);
            }
            return format!("{}/v1/responses", trimmed);
        }
    }
    "http://127.0.0.1:20128/v1/responses".to_string()
}

/// Retrieve authorization header for 9Router subagents.
pub fn get_subagent_auth_header() -> Option<String> {
    if let Ok(key) = env::var("NINEROUTER_KEY") {
        let trimmed = key.trim();
        if !trimmed.is_empty() {
            return Some(format!("Bearer {}", trimmed));
        }
    }
    None
}

/// Decompress request body if compressed with `Content-Encoding: zstd`.
pub fn decompress_if_needed(body: &[u8], headers: &HeaderMap) -> (Vec<u8>, bool) {
    if let Some(enc) = headers
        .get(header::CONTENT_ENCODING)
        .and_then(|v| v.to_str().ok())
    {
        if enc.eq_ignore_ascii_case("zstd") {
            if let Ok(decompressed) = zstd::decode_all(body) {
                return (decompressed, true);
            }
        }
    }
    (body.to_vec(), false)
}

/// Inspect and determine routing policy for an HTTP request to the loopback proxy:
/// Returns (is_subagent, modified_or_original_body).
pub fn inspect_and_route_http_request(path: &str, body: &[u8]) -> (bool, Vec<u8>) {
    if !is_responses_path(path) || body.is_empty() {
        return (false, body.to_vec());
    }

    let Ok(mut json) = serde_json::from_slice::<Value>(body) else {
        return (false, body.to_vec());
    };

    let current_model = json.get("model").and_then(|m| m.as_str());
    let model_is_subagent = current_model.map_or(false, is_subagent_model_name);
    let has_subagent_marker = contains_subagent_source(&json);
    let detected_role = find_agent_role(&json);
    let role_is_subagent = detected_role.as_deref().map_or(false, is_subagent_role_name);
    let has_agent_nickname = json
        .get("agentNickname")
        .or_else(|| json.get("agent_nickname"))
        .and_then(|v| v.as_str())
        .map_or(false, |s| !s.is_empty());

    let is_subagent =
        model_is_subagent || has_subagent_marker || role_is_subagent || has_agent_nickname;

    if !is_subagent {
        return (false, body.to_vec());
    }

    let needs_model_rewrite = match current_model {
        None => true,
        Some(m) => {
            m.is_empty()
                || m.starts_with("gpt-")
                || m.starts_with("o1")
                || m.starts_with("o3")
                || m.starts_with("chatgpt")
        }
    };

    if needs_model_rewrite {
        let mapped_model = map_role_to_model(detected_role.as_deref());
        if let Some(obj) = json.as_object_mut() {
            obj.insert("model".to_string(), Value::String(mapped_model));
        }
    }

    let out_body = serde_json::to_vec(&json).unwrap_or_else(|_| body.to_vec());
    (true, out_body)
}

/// Resolve the forward target URL for a given path and routing classification.
pub fn resolve_forward_url(path: &str, is_subagent: bool) -> String {
    if is_subagent {
        get_subagent_responses_url()
    } else {
        let base = get_chatgpt_upstream_base();
        let base_trimmed = base.trim_end_matches('/');
        if path.starts_with('/') {
            format!("{}{}", base_trimmed, path)
        } else {
            format!("{}/{}", base_trimmed, path)
        }
    }
}

/// Resolve the forward target URL preserving optional query parameters.
pub fn resolve_forward_url_with_query(
    path: &str,
    query: Option<&str>,
    is_subagent: bool,
) -> String {
    let base_url = resolve_forward_url(path, is_subagent);
    if let Some(q) = query {
        if !q.is_empty() && !base_url.contains('?') {
            return format!("{}?{}", base_url, q);
        }
    }
    base_url
}

/// Check if an HTTP header is a hop-by-hop header that should not be forwarded.
pub fn is_hop_by_hop_header(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    matches!(
        lower.as_str(),
        "connection"
            | "keep-alive"
            | "proxy-authenticate"
            | "proxy-authorization"
            | "te"
            | "trailers"
            | "transfer-encoding"
            | "upgrade"
            | "host"
    )
}

/// Build headers to forward upstream to either 9Router or ChatGPT.
pub fn build_forward_headers(incoming_headers: &HeaderMap, is_subagent: bool) -> HeaderMap {
    let mut out = HeaderMap::new();
    let mut client_auth: Option<HeaderValue> = None;

    for (key, value) in incoming_headers.iter() {
        let key_str = key.as_str();
        if is_hop_by_hop_header(key_str) || key_str.eq_ignore_ascii_case("content-length") {
            continue;
        }
        if key_str.eq_ignore_ascii_case("authorization") {
            client_auth = Some(value.clone());
            if is_subagent {
                continue;
            }
        }
        if is_subagent && key_str.eq_ignore_ascii_case("content-encoding") {
            // Subagent requests are forwarded as decompressed JSON
            continue;
        }
        out.insert(key.clone(), value.clone());
    }

    if is_subagent {
        if let Some(auth_str) = get_subagent_auth_header() {
            if let Ok(val) = HeaderValue::from_str(&auth_str) {
                out.insert(header::AUTHORIZATION, val);
            }
        } else if let Some(orig) = client_auth {
            out.insert(header::AUTHORIZATION, orig);
        } else {
            out.insert(
                header::AUTHORIZATION,
                HeaderValue::from_static("Bearer dummy-9router-key"),
            );
        }
    }

    out
}

/// Filter upstream response headers before returning to the local Codex client.
pub fn filter_response_headers(upstream_headers: &HeaderMap) -> HeaderMap {
    let mut out = HeaderMap::new();
    for (key, value) in upstream_headers.iter() {
        let key_str = key.as_str();
        if is_hop_by_hop_header(key_str) || key_str.eq_ignore_ascii_case("content-length") {
            continue;
        }
        out.insert(key.clone(), value.clone());
    }
    out
}

/// Default paths for the self-signed loopback TLS certificate and private key.
pub fn default_cert_paths() -> (PathBuf, PathBuf) {
    let base_dir = env::var("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            let user = env::var("USERPROFILE").unwrap_or_else(|_| ".".to_string());
            PathBuf::from(user).join("AppData").join("Local")
        })
        .join("OpenAI")
        .join("Codex")
        .join("custom");

    let cert_path = base_dir.join("bridge-cert.pem");
    let key_path = base_dir.join("bridge-key.pem");
    (cert_path, key_path)
}

/// Load existing or generate a new self-signed TLS certificate for `127.0.0.1` / `localhost`.
pub fn get_or_create_tls_files(
    cert_path: &Path,
    key_path: &Path,
) -> Result<(String, String), Box<dyn std::error::Error + Send + Sync>> {
    if cert_path.exists() && key_path.exists() {
        let cert_pem = fs::read_to_string(cert_path)?;
        let key_pem = fs::read_to_string(key_path)?;
        if !cert_pem.trim().is_empty() && !key_pem.trim().is_empty() {
            return Ok((cert_pem, key_pem));
        }
    }

    let subject_alt_names = vec![
        "127.0.0.1".to_string(),
        "localhost".to_string(),
        "::1".to_string(),
    ];

    let cert = generate_simple_self_signed(subject_alt_names)?;
    let cert_pem = cert.cert.pem();
    let key_pem = cert.signing_key.serialize_pem();

    if let Some(parent) = cert_path.parent() {
        fs::create_dir_all(parent)?;
    }
    if let Some(parent) = key_path.parent() {
        fs::create_dir_all(parent)?;
    }

    fs::write(cert_path, &cert_pem)?;
    fs::write(key_path, &key_pem)?;

    Ok((cert_pem, key_pem))
}

/// Build a `tokio_rustls::TlsAcceptor` supporting both HTTP/2 (`h2`) and `http/1.1`.
pub fn create_tls_acceptor(
    cert_path: &Path,
    key_path: &Path,
) -> Result<TlsAcceptor, Box<dyn std::error::Error + Send + Sync>> {
    let _ = get_or_create_tls_files(cert_path, key_path)?;

    let cert_pem = fs::read(cert_path)?;
    let key_pem = fs::read(key_path)?;

    let mut certs = Vec::new();
    for cert_result in rustls_pemfile::certs(&mut &cert_pem[..]) {
        certs.push(cert_result?);
    }

    let key = rustls_pemfile::private_key(&mut &key_pem[..])?
        .ok_or_else(|| "No private key found in key file")?;

    let mut server_config = ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)?;
    server_config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];

    Ok(TlsAcceptor::from(Arc::new(server_config)))
}

#[derive(Clone)]
pub struct ProxyAppState {
    pub http_client: reqwest::Client,
}

/// Health check endpoint (`GET /health`).
pub async fn health_handler() -> impl IntoResponse {
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/json")],
        r#"{"status":"ok","service":"codex-9router-proxy"}"#,
    )
}

/// Reject WebSocket upgrade attempts (`GET /backend-api/codex/responses`) with `426 Upgrade Required`
/// so `codex.orig.exe` immediately falls back to HTTPS POST with zero retry delay.
pub async fn ws_responses_handler() -> Response {
    (
        StatusCode::UPGRADE_REQUIRED,
        [(header::CONTENT_TYPE, "text/plain")],
        "WebSockets not supported by loopback proxy; use HTTPS POST",
    )
        .into_response()
}

/// If a specialized role model (`implement`, `explore`, `review`) fails upstream on 9Router,
/// rewrite `"model"` to the default subagent model (`9router-subagent`) for automatic retry.
pub fn rewrite_body_model_to_fallback(body: &[u8]) -> Option<Vec<u8>> {
    let mut json = serde_json::from_slice::<Value>(body).ok()?;
    let fallback_model = map_role_to_model(Some("default"));
    let current = json.get("model").and_then(|v| v.as_str()).unwrap_or("");
    if current.eq_ignore_ascii_case(&fallback_model) || fallback_model.is_empty() {
        return None;
    }
    json.as_object_mut()?
        .insert("model".to_string(), Value::String(fallback_model));
    serde_json::to_vec(&json).ok()
}

/// Handle all HTTP/HTTPS requests arriving at the embedded loopback reverse proxy.
pub async fn proxy_handler(
    State(state): State<Arc<ProxyAppState>>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if method == Method::OPTIONS {
        return (
            StatusCode::OK,
            [
                (header::ACCESS_CONTROL_ALLOW_ORIGIN, "*"),
                (
                    header::ACCESS_CONTROL_ALLOW_METHODS,
                    "GET, POST, PUT, DELETE, OPTIONS, PATCH",
                ),
                (header::ACCESS_CONTROL_ALLOW_HEADERS, "*"),
            ],
            "",
        )
            .into_response();
    }

    let path = uri.path();
    let query = uri.query();

    if path == "/health" || path == "/health/" {
        return health_handler().await.into_response();
    }

    let (decompressed_body, _was_compressed) = decompress_if_needed(&body, &headers);
    let (is_subagent, routed_body) = inspect_and_route_http_request(path, &decompressed_body);
    let target_url = resolve_forward_url_with_query(path, query, is_subagent);
    let forward_headers = build_forward_headers(&headers, is_subagent);

    let mut req_builder = state
        .http_client
        .request(method.clone(), &target_url)
        .headers(forward_headers.clone());

    let body_to_send = if is_subagent {
        Bytes::from(routed_body.clone())
    } else {
        body
    };

    if !body_to_send.is_empty() {
        req_builder = req_builder.body(body_to_send);
    }

    let mut upstream_res = match req_builder.send().await {
        Ok(res) => res,
        Err(err) => {
            return (
                StatusCode::BAD_GATEWAY,
                [(header::CONTENT_TYPE, "application/json")],
                serde_json::json!({
                    "error": {
                        "message": format!("Codex 9Router Proxy upstream connection error to {}: {}", target_url, err),
                        "type": "proxy_gateway_error",
                        "code": 502
                    }
                })
                .to_string(),
            )
                .into_response();
        }
    };

    // Automatic resilience fallback: if a specialized role model (e.g. implement / review / explore)
    // returns HTTP >= 400 from 9Router due to an unavailable provider model in that combo,
    // automatically retry once using the default subagent model (`9router-subagent`).
    if is_subagent && upstream_res.status().as_u16() >= 400 {
        if let Some(fallback_body) = rewrite_body_model_to_fallback(&routed_body) {
            if let Ok(retry_res) = state
                .http_client
                .request(method, &target_url)
                .headers(forward_headers)
                .body(Bytes::from(fallback_body))
                .send()
                .await
            {
                upstream_res = retry_res;
            }
        }
    }

    let status = StatusCode::from_u16(upstream_res.status().as_u16())
        .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    let clean_headers = filter_response_headers(upstream_res.headers());

    let stream = upstream_res
        .bytes_stream()
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e));

    let mut response = Response::builder().status(status);
    for (k, v) in clean_headers.iter() {
        response = response.header(k, v);
    }

    response
        .body(Body::from_stream(stream))
        .unwrap_or_else(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Internal error streaming response: {}", e),
            )
                .into_response()
        })
}

/// Create the Axum router for the loopback reverse proxy.
pub fn create_router(state: Arc<ProxyAppState>) -> Router {
    Router::new()
        .route("/health", get(health_handler))
        .route("/health/", get(health_handler))
        .route(
            "/backend-api/codex/responses",
            get(ws_responses_handler).post(proxy_handler),
        )
        .route(
            "/backend-api/responses",
            get(ws_responses_handler).post(proxy_handler),
        )
        .route(
            "/codex/responses",
            get(ws_responses_handler).post(proxy_handler),
        )
        .route(
            "/responses",
            get(ws_responses_handler).post(proxy_handler),
        )
        .fallback(proxy_handler)
        .with_state(state)
}

/// Bind and spawn the embedded dual-stack HTTP/HTTPS reverse proxy in a background thread.
pub fn spawn_embedded_reverse_proxy(bind_addr: &str) -> io::Result<PathBuf> {
    let (cert_path, key_path) = default_cert_paths();
    let tls_acceptor = create_tls_acceptor(&cert_path, &key_path)
        .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;

    let std_listener = match TcpListener::bind(bind_addr) {
        Ok(l) => l,
        Err(e) => {
            // If another instance of codex-9router-proxy is already listening on 20129, return the cert path
            if e.kind() == io::ErrorKind::AddrInUse {
                return Ok(cert_path);
            }
            return Err(e);
        }
    };
    std_listener.set_nonblocking(true)?;

    thread::spawn(move || {
        let rt = match tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
        {
            Ok(r) => r,
            Err(_) => return,
        };

        rt.block_on(async move {
            let listener = match tokio::net::TcpListener::from_std(std_listener) {
                Ok(l) => l,
                Err(_) => return,
            };

            let http_client = match reqwest::Client::builder()
                .tcp_keepalive(Duration::from_secs(60))
                .pool_idle_timeout(Duration::from_secs(90))
                .build()
            {
                Ok(c) => c,
                Err(_) => reqwest::Client::new(),
            };

            let state = Arc::new(ProxyAppState { http_client });
            let app = create_router(state);

            loop {
                let (stream, _) = match listener.accept().await {
                    Ok(res) => res,
                    Err(_) => continue,
                };

                let acceptor = tls_acceptor.clone();
                let router = app.clone();

                tokio::spawn(async move {
                    let mut peek_buf = [0u8; 1];
                    let is_tls = match stream.peek(&mut peek_buf).await {
                        Ok(1) => peek_buf[0] == 0x16, // TLS ClientHello
                        _ => false,
                    };

                    let auto = auto::Builder::new(TokioExecutor::new());

                    if is_tls {
                        if let Ok(tls_stream) = acceptor.accept(stream).await {
                            let io = TokioIo::new(tls_stream);
                            let service = hyper::service::service_fn(
                                move |req: axum::http::Request<hyper::body::Incoming>| {
                                    let mut r = router.clone();
                                    async move {
                                        let req = req.map(Body::new);
                                        r.call(req).await
                                    }
                                },
                            );
                            let _ = auto.serve_connection_with_upgrades(io, service).await;
                        }
                    } else {
                        let io = TokioIo::new(stream);
                        let service = hyper::service::service_fn(
                            move |req: axum::http::Request<hyper::body::Incoming>| {
                                let mut r = router.clone();
                                async move {
                                    let req = req.map(Body::new);
                                    r.call(req).await
                                }
                            },
                        );
                        let _ = auto.serve_connection_with_upgrades(io, service).await;
                    }
                });
            }
        });
    });

    Ok(cert_path)
}

/// Run full system health diagnostics and print report.
pub fn run_doctor() {
    println!("===================================================================");
    println!("           Codex 9Router Proxy - Diagnostics Doctor 🩺             ");
    println!("===================================================================");
    println!();

    if let Ok(exe) = env::current_exe() {
        println!("[OK] Running executable : {}", exe.display());
    }

    let real_codex = find_real_codex();
    if real_codex.is_file() {
        let size = real_codex.metadata().map_or(0, |m| m.len());
        println!(
            "[OK] Official engine    : {} ({} bytes)",
            real_codex.display(),
            size
        );
    } else {
        println!("[FAIL] Official engine not found at: {}", real_codex.display());
    }

    let provider = get_target_model_provider();
    println!("[OK] Subagent Provider  : {}", provider);
    let key_set = env::var("NINEROUTER_KEY").map_or(false, |k| !k.trim().is_empty());
    if key_set {
        println!("[OK] Provider API Key   : [CONFIGURED / MASKED]");
    } else {
        println!("[INFO] NINEROUTER_KEY   : Not set in process environment");
    }

    let (cert_path, key_path) = default_cert_paths();
    let tls_status = match get_or_create_tls_files(&cert_path, &key_path) {
        Ok(_) => format!("Ready ({})", cert_path.display()),
        Err(e) => format!("Error ({})", e),
    };
    println!("[OK] Loopback TLS Cert  : {}", tls_status);

    let proxy_port = get_proxy_port();
    let proxy_addr = format!("127.0.0.1:{}", proxy_port);
    let proxy_status = match TcpListener::bind(&proxy_addr) {
        Ok(_) => "Port available / ready to bind",
        Err(_) => "Port active / in use",
    };
    println!("[OK] Reverse Proxy Port : {} ({})", proxy_addr, proxy_status);
    println!("     - Loopback URL     : https://{}/backend-api/", proxy_addr);
    let subagent_url = get_subagent_responses_url();
    println!("     - Subagent Route   : Strict -> {}", subagent_url);
    let upstream_base = get_chatgpt_upstream_base();
    println!("     - Parent Route     : Upstream -> {}/backend-api/", upstream_base);

    if let Some(cfg) = get_config_path() {
        println!("[OK] Codex Config TOML  : {}", cfg.display());
    } else {
        println!("[WARN] Config TOML      : Not found in ~/.codex/config.toml");
    }

    println!("     - Default Model    : {}", map_role_to_model(Some("default")));
    println!("     - Worker Model     : {}", map_role_to_model(Some("worker")));
    println!("     - Explorer Model   : {}", map_role_to_model(Some("explorer")));
    println!("     - Reviewer Model   : {}", map_role_to_model(Some("reviewer")));

    if let Some(codex_home) = get_codex_home_dir() {
        let agents_dir = codex_home.join("agents");
        if agents_dir.is_dir() {
            println!("[OK] Role Manifests     : {}", agents_dir.display());
            for role in &["default", "worker", "explorer", "reviewer"] {
                let toml = agents_dir.join(format!("{}.toml", role));
                if toml.is_file() {
                    println!("     - {:<8} TOML   : Found", role);
                } else {
                    println!("     - {:<8} TOML   : Missing ({})", role, toml.display());
                }
            }
        }
    }

    probe_endpoint(&subagent_url);

    println!();
    println!("===================================================================");
    println!("Diagnostics complete. All checks finished.");
    println!("===================================================================");
}

/// Strip any existing `chatgpt_base_url` override from CLI arguments and append the HTTPS loopback URL.
pub fn inject_loopback_base_url(args: &[String], port: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut skip_next = false;
    for (i, a) in args.iter().enumerate() {
        if skip_next {
            skip_next = false;
            continue;
        }
        if a == "-c" && i + 1 < args.len() && args[i + 1].contains("chatgpt_base_url") {
            skip_next = true;
            continue;
        }
        if a.starts_with("-c=") && a.contains("chatgpt_base_url") {
            continue;
        }
        out.push(a.clone());
    }
    out.push("-c".to_string());
    out.push(format!(
        "chatgpt_base_url=\"https://127.0.0.1:{}/backend-api/\"",
        port
    ));
    out
}

fn main() -> io::Result<()> {
    let args: Vec<String> = env::args().skip(1).collect();

    if args.iter().any(|a| {
        a.eq_ignore_ascii_case("--doctor")
            || a.eq_ignore_ascii_case("doctor")
            || a.eq_ignore_ascii_case("-doctor")
    }) {
        run_doctor();
        return Ok(());
    }

    let real_codex = find_real_codex();

    let is_lightweight_command = args.is_empty()
        || args.iter().any(|a| {
            a == "--version"
                || a == "-V"
                || a == "version"
                || a == "help"
                || a == "-h"
                || a == "--help"
                || a == "daemon"
                || a == "status"
                || a == "stop"
                || a == "restart"
                || a == "start"
                || a == "login"
                || a == "logout"
        });

    let is_app_server = args.iter().any(|a| a == "app-server") && !is_lightweight_command;

    if !is_app_server {
        let mut forward_args = args.clone();
        let mut cmd = Command::new(&real_codex);

        if !is_lightweight_command {
            let port = get_proxy_port();
            let bind_addr = format!("127.0.0.1:{}", port);
            if let Ok(cert_path) = spawn_embedded_reverse_proxy(&bind_addr) {
                cmd.env("CODEX_CA_CERTIFICATE", &cert_path);
                forward_args = inject_loopback_base_url(&forward_args, &port);
            }
        }

        let status = cmd.args(&forward_args).status()?;
        std::process::exit(status.code().unwrap_or(1));
    }

    let port = get_proxy_port();
    let bind_addr = format!("127.0.0.1:{}", port);
    let (default_cert, _) = default_cert_paths();
    let cert_path = match spawn_embedded_reverse_proxy(&bind_addr) {
        Ok(p) => p,
        Err(e) => {
            eprintln!(
                "[codex-9router-proxy] Reverse proxy note on {}: {}",
                bind_addr, e
            );
            default_cert
        }
    };

    let child_args = inject_loopback_base_url(&args, &port);

    let mut child = Command::new(&real_codex)
        .args(&child_args)
        .env("CODEX_CA_CERTIFICATE", &cert_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()?;

    let mut child_stdin = child.stdin.take().expect("Failed to open child stdin");
    let child_stdout = child.stdout.take().expect("Failed to open child stdout");

    // Thread 1: STDIN (Client/Desktop -> Proxy -> real codex)
    let stdin_thread = thread::spawn(move || {
        let stdin = io::stdin();
        let mut reader = BufReader::new(stdin.lock());
        let mut line = String::new();

        while reader.read_line(&mut line).unwrap_or(0) > 0 {
            let mut processed = line.clone();

            if let Ok(mut json) = serde_json::from_str::<Value>(&line) {
                let mut modified = false;

                if let Some(method) = json.get("method").and_then(|m| m.as_str()) {
                    match method {
                        "thread/start"
                        | "thread/fork"
                        | "thread/resume"
                        | "thread/settings/update"
                        | "turn/start" => {
                            if let Some(params) =
                                json.get_mut("params").and_then(|p| p.as_object_mut())
                            {
                                if route_thread_params(params) {
                                    modified = true;
                                }
                            }
                        }
                        _ => {}
                    }
                }

                if modified {
                    if let Ok(s) = serde_json::to_string(&json) {
                        processed = s + "\n";
                    }
                }
            }

            if child_stdin.write_all(processed.as_bytes()).is_err()
                || child_stdin.flush().is_err()
            {
                break;
            }
            line.clear();
        }
    });

    // Thread 2: STDOUT (real codex -> Proxy -> Client/Desktop)
    let stdout_thread = thread::spawn(move || {
        let mut reader = BufReader::new(child_stdout);
        let stdout = io::stdout();
        let mut stdout_lock = stdout.lock();
        let mut line = String::new();

        while reader.read_line(&mut line).unwrap_or(0) > 0 {
            let mut processed = line.clone();

            if let Ok(mut json) = serde_json::from_str::<Value>(&line) {
                let mut modified = false;

                if let Some(result) = json.get_mut("result") {
                    if sanitize_rate_limits(result) {
                        modified = true;
                    }
                }

                if let Some(params) = json.get_mut("params") {
                    if sanitize_rate_limits(params) {
                        modified = true;
                    }
                }

                if modified {
                    if let Ok(s) = serde_json::to_string(&json) {
                        processed = s + "\n";
                    }
                }
            }

            if stdout_lock.write_all(processed.as_bytes()).is_err()
                || stdout_lock.flush().is_err()
            {
                break;
            }
            line.clear();
        }
    });

    let status = child.wait()?;
    let _ = stdin_thread.join();
    let _ = stdout_thread.join();

    std::process::exit(status.code().unwrap_or(0));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_model_from_role_toml() {
        let toml = r#"
name = "worker"
description = "Implementation-focused subagent"
model = "implement"
model_provider = "9router"
model_reasoning_effort = "high"
"#;
        assert_eq!(
            parse_model_from_role_toml(toml),
            Some("implement".to_string())
        );
    }

    #[test]
    fn test_parse_model_from_toml_subagent_models() {
        let toml = r#"
[agents]
default_subagent_model = "fallback-subagent"

[subagent_models]
default = "my-default"
worker = "my-worker"
explorer = "my-explorer"
reviewer = "my-reviewer"
"#;
        assert_eq!(
            parse_model_from_toml(toml, "worker"),
            Some("my-worker".to_string())
        );
        assert_eq!(
            parse_model_from_toml(toml, "explorer"),
            Some("my-explorer".to_string())
        );
        assert_eq!(
            parse_model_from_toml(toml, "reviewer"),
            Some("my-reviewer".to_string())
        );
        assert_eq!(
            parse_model_from_toml(toml, "default"),
            Some("my-default".to_string())
        );
        assert_eq!(
            parse_model_from_toml(toml, "unknown_role"),
            Some("fallback-subagent".to_string())
        );
    }

    #[test]
    fn test_parse_model_from_toml_fallback_agents() {
        let toml = r#"
[agents]
default_subagent_model = "9router-subagent"
"#;
        assert_eq!(
            parse_model_from_toml(toml, "worker"),
            Some("9router-subagent".to_string())
        );
        assert_eq!(
            parse_model_from_toml(toml, "explorer"),
            Some("9router-subagent".to_string())
        );
    }

    #[test]
    fn test_builtin_role_model_defaults() {
        assert_eq!(builtin_role_model("worker"), "implement");
        assert_eq!(builtin_role_model("explorer"), "explore");
        assert_eq!(builtin_role_model("reviewer"), "review");
        assert_eq!(builtin_role_model("default"), "9router-subagent");
    }

    #[test]
    fn test_route_worker_role_defaults_to_implement() {
        let mut params = serde_json::Map::new();
        params.insert("agentRole".to_string(), Value::String("worker".to_string()));
        let modded = route_thread_params(&mut params);
        assert!(modded);
        assert!(params.contains_key("model"));
        assert_eq!(params.get("modelProvider").unwrap(), "9router");
        assert_eq!(params.get("model_provider").unwrap(), "9router");
    }

    #[test]
    fn test_route_explorer_role_defaults_to_explore() {
        let mut params = serde_json::Map::new();
        params.insert("role".to_string(), Value::String("explorer".to_string()));
        let modded = route_thread_params(&mut params);
        assert!(modded);
        assert!(params.contains_key("model"));
        assert_eq!(params.get("modelProvider").unwrap(), "9router");
        assert_eq!(params.get("model_provider").unwrap(), "9router");
    }

    #[test]
    fn test_route_default_role() {
        let mut params = serde_json::Map::new();
        params.insert(
            "agent_type".to_string(),
            Value::String("default".to_string()),
        );
        let modded = route_thread_params(&mut params);
        assert!(modded);
        assert_eq!(params.get("modelProvider").unwrap(), "9router");
        assert_eq!(params.get("model_provider").unwrap(), "9router");
    }

    #[test]
    fn test_route_9router_model_explicit() {
        let mut params = serde_json::Map::new();
        params.insert(
            "model".to_string(),
            Value::String("9router-subagent".to_string()),
        );
        params.insert(
            "modelProvider".to_string(),
            Value::String("openai".to_string()),
        );
        let modded = route_thread_params(&mut params);
        assert!(modded);
        assert_eq!(params.get("modelProvider").unwrap(), "9router");
        assert_eq!(params.get("model_provider").unwrap(), "9router");
    }

    #[test]
    fn test_route_main_agent_chatgpt() {
        let mut params = serde_json::Map::new();
        params.insert("model".to_string(), Value::String("gpt-6-luna".to_string()));
        let modded = route_thread_params(&mut params);
        assert!(modded);
        assert_eq!(params.get("modelProvider").unwrap(), "openai");
        assert_eq!(params.get("model_provider").unwrap(), "openai");
    }

    #[test]
    fn test_route_nested_settings_propagates_to_parent() {
        let mut inner = serde_json::Map::new();
        inner.insert("agentRole".to_string(), Value::String("worker".to_string()));
        let mut params = serde_json::Map::new();
        params.insert("settings".to_string(), Value::Object(inner));
        let modded = route_thread_params(&mut params);
        assert!(modded);
        let nested = params.get("settings").unwrap().as_object().unwrap();
        assert_eq!(nested.get("modelProvider").unwrap(), "9router");
        assert_eq!(nested.get("model_provider").unwrap(), "9router");
        assert_eq!(params.get("modelProvider").unwrap(), "9router");
        assert_eq!(params.get("model_provider").unwrap(), "9router");
    }

    #[test]
    fn test_route_worker_with_null_model() {
        let mut params = serde_json::Map::new();
        params.insert("agentRole".to_string(), Value::String("worker".to_string()));
        params.insert("model".to_string(), Value::Null);
        let modded = route_thread_params(&mut params);
        assert!(modded);
        assert_eq!(params.get("modelProvider").unwrap(), "9router");
        assert_eq!(params.get("model_provider").unwrap(), "9router");
    }

    #[test]
    fn test_route_thread_source_string() {
        let mut params = serde_json::Map::new();
        params.insert(
            "threadSource".to_string(),
            Value::String("subAgent".to_string()),
        );
        let modded = route_thread_params(&mut params);
        assert!(modded);
        assert_eq!(params.get("modelProvider").unwrap(), "9router");
        assert_eq!(params.get("model_provider").unwrap(), "9router");
    }

    #[test]
    fn test_route_thread_source_object() {
        let mut spawn = serde_json::Map::new();
        spawn.insert(
            "agent_role".to_string(),
            Value::String("worker".to_string()),
        );
        let mut sub = serde_json::Map::new();
        sub.insert("thread_spawn".to_string(), Value::Object(spawn));
        let mut source = serde_json::Map::new();
        source.insert("subAgent".to_string(), Value::Object(sub));

        let mut params = serde_json::Map::new();
        params.insert("threadSource".to_string(), Value::Object(source));
        let modded = route_thread_params(&mut params);
        assert!(modded);
        assert_eq!(params.get("modelProvider").unwrap(), "9router");
        assert_eq!(params.get("model_provider").unwrap(), "9router");
    }

    #[test]
    fn test_route_agent_nickname() {
        let mut params = serde_json::Map::new();
        params.insert(
            "agentNickname".to_string(),
            Value::String("helpful-falcon".to_string()),
        );
        let modded = route_thread_params(&mut params);
        assert!(modded);
        assert_eq!(params.get("modelProvider").unwrap(), "9router");
        assert_eq!(params.get("model_provider").unwrap(), "9router");
    }

    #[test]
    fn test_route_worker_with_chatgpt_inherited_model() {
        let mut params = serde_json::Map::new();
        params.insert("agentRole".to_string(), Value::String("worker".to_string()));
        params.insert("model".to_string(), Value::String("gpt-6-luna".to_string()));
        let modded = route_thread_params(&mut params);
        assert!(modded);
        assert_ne!(params.get("model").unwrap(), "gpt-6-luna");
        assert_eq!(params.get("modelProvider").unwrap(), "9router");
        assert_eq!(params.get("model_provider").unwrap(), "9router");
    }

    #[test]
    fn test_sanitize_rate_limits() {
        let mut val = serde_json::json!({
            "ordinaryUsageAllowed": false,
            "rateLimitUpsell": "upsell_info",
            "rateLimits": {
                "rateLimitReachedType": "spend",
                "spendControlReached": true,
                "primary": { "usedPercent": 100 },
                "secondary": { "usedPercent": 95 }
            }
        });
        let modded = sanitize_rate_limits(&mut val);
        assert!(modded);
        assert_eq!(val["ordinaryUsageAllowed"], true);
        assert_eq!(val["rateLimitUpsell"], Value::Null);
        assert_eq!(val["rateLimits"]["rateLimitReachedType"], Value::Null);
        assert_eq!(val["rateLimits"]["spendControlReached"], false);
        assert_eq!(val["rateLimits"]["primary"]["usedPercent"], 0);
        assert_eq!(val["rateLimits"]["secondary"]["usedPercent"], 0);
    }

    #[test]
    fn test_is_subagent_model_name() {
        assert!(is_subagent_model_name("9router-subagent"));
        assert!(is_subagent_model_name("9router/claude-3-5-sonnet"));
        assert!(is_subagent_model_name("implement"));
        assert!(is_subagent_model_name("explore"));
        assert!(is_subagent_model_name("review"));
        assert!(!is_subagent_model_name("gpt-6-luna"));
        assert!(!is_subagent_model_name("gpt-4o"));
        assert!(!is_subagent_model_name("o1-preview"));
    }

    #[test]
    fn test_is_responses_path() {
        assert!(is_responses_path("/backend-api/codex/responses"));
        assert!(is_responses_path("/backend-api/responses"));
        assert!(is_responses_path("/codex/responses"));
        assert!(is_responses_path("/backend-api/codex/responses?client=desktop"));
        assert!(!is_responses_path("/backend-api/me"));
        assert!(!is_responses_path("/backend-api/accounts/check/v4-2023-04-27"));
        assert!(!is_responses_path("/backend-api/conversations"));
    }

    #[test]
    fn test_inspect_and_route_explicit_subagent_model() {
        let body = serde_json::json!({
            "model": "implement",
            "messages": [{"role": "user", "content": "Write unit tests"}]
        });
        let body_bytes = serde_json::to_vec(&body).unwrap();
        let (is_subagent, routed_body) =
            inspect_and_route_http_request("/backend-api/codex/responses", &body_bytes);
        assert!(is_subagent);
        let parsed: Value = serde_json::from_slice(&routed_body).unwrap();
        assert_eq!(parsed["model"], "implement");
    }

    #[test]
    fn test_inspect_and_route_worker_role_rewrites_to_implement() {
        let body = serde_json::json!({
            "model": "gpt-6-luna",
            "agent_role": "worker",
            "messages": [{"role": "user", "content": "Refactor codebase"}]
        });
        let body_bytes = serde_json::to_vec(&body).unwrap();
        let (is_subagent, routed_body) =
            inspect_and_route_http_request("/backend-api/codex/responses", &body_bytes);
        assert!(is_subagent);
        let parsed: Value = serde_json::from_slice(&routed_body).unwrap();
        let expected = map_role_to_model(Some("worker"));
        assert_eq!(parsed["model"], expected);
        assert_ne!(parsed["model"], "gpt-6-luna");
    }

    #[test]
    fn test_inspect_and_route_explorer_role_rewrites_to_explore() {
        let body = serde_json::json!({
            "model": "gpt-6-luna",
            "role": "explorer",
            "messages": [{"role": "user", "content": "Search files"}]
        });
        let body_bytes = serde_json::to_vec(&body).unwrap();
        let (is_subagent, routed_body) =
            inspect_and_route_http_request("/backend-api/codex/responses", &body_bytes);
        assert!(is_subagent);
        let parsed: Value = serde_json::from_slice(&routed_body).unwrap();
        let expected = map_role_to_model(Some("explorer"));
        assert_eq!(parsed["model"], expected);
        assert_ne!(parsed["model"], "gpt-6-luna");
    }

    #[test]
    fn test_inspect_and_route_reviewer_role_rewrites_to_review() {
        let body = serde_json::json!({
            "agentRole": "reviewer",
            "messages": [{"role": "user", "content": "Review diff"}]
        });
        let body_bytes = serde_json::to_vec(&body).unwrap();
        let (is_subagent, routed_body) =
            inspect_and_route_http_request("/backend-api/responses", &body_bytes);
        assert!(is_subagent);
        let parsed: Value = serde_json::from_slice(&routed_body).unwrap();
        let expected = map_role_to_model(Some("reviewer"));
        assert_eq!(parsed["model"], expected);
    }

    #[test]
    fn test_inspect_and_route_parent_turn_untouched() {
        let body = serde_json::json!({
            "model": "gpt-6-luna",
            "messages": [{"role": "user", "content": "Hello world"}]
        });
        let body_bytes = serde_json::to_vec(&body).unwrap();
        let (is_subagent, routed_body) =
            inspect_and_route_http_request("/backend-api/codex/responses", &body_bytes);
        assert!(!is_subagent);
        let parsed: Value = serde_json::from_slice(&routed_body).unwrap();
        assert_eq!(parsed["model"], "gpt-6-luna");
    }

    #[test]
    fn test_inspect_and_route_non_responses_untouched() {
        let body = serde_json::json!({
            "account_id": "12345"
        });
        let body_bytes = serde_json::to_vec(&body).unwrap();
        let (is_subagent, routed_body) =
            inspect_and_route_http_request("/backend-api/me", &body_bytes);
        assert!(!is_subagent);
        assert_eq!(body_bytes, routed_body);
    }

    #[test]
    fn test_decompress_zstd_and_route_subagent() {
        let original_json = serde_json::json!({
            "model": "9router-subagent",
            "input": [{"role": "user", "content": "ping"}]
        });
        let raw_bytes = serde_json::to_vec(&original_json).unwrap();
        let compressed = zstd::encode_all(&raw_bytes[..], 3).unwrap();

        let mut headers = HeaderMap::new();
        headers.insert(header::CONTENT_ENCODING, HeaderValue::from_static("zstd"));

        let (decompressed, was_compressed) = decompress_if_needed(&compressed, &headers);
        assert!(was_compressed);
        assert_eq!(decompressed, raw_bytes);

        let (is_subagent, routed) =
            inspect_and_route_http_request("/backend-api/codex/responses", &decompressed);
        assert!(is_subagent);
        let parsed: Value = serde_json::from_slice(&routed).unwrap();
        assert_eq!(parsed["model"], "9router-subagent");
    }

    #[test]
    fn test_build_forward_headers_strips_zstd_for_subagent() {
        let mut incoming = HeaderMap::new();
        incoming.insert(header::HOST, HeaderValue::from_static("127.0.0.1:20129"));
        incoming.insert(header::CONTENT_ENCODING, HeaderValue::from_static("zstd"));
        incoming.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        );

        let subagent_headers = build_forward_headers(&incoming, true);
        assert!(!subagent_headers.contains_key(header::HOST));
        assert!(!subagent_headers.contains_key(header::CONTENT_ENCODING));
        assert!(subagent_headers.contains_key(header::AUTHORIZATION));
        assert_eq!(
            subagent_headers.get(header::CONTENT_TYPE).unwrap(),
            "application/json"
        );

        let parent_headers = build_forward_headers(&incoming, false);
        assert!(!parent_headers.contains_key(header::HOST));
        assert_eq!(
            parent_headers.get(header::CONTENT_ENCODING).unwrap(),
            "zstd"
        );
    }

    #[test]
    fn test_resolve_forward_url() {
        assert_eq!(
            resolve_forward_url("/backend-api/codex/responses", true),
            get_subagent_responses_url()
        );
        assert_eq!(
            resolve_forward_url("/backend-api/me", false),
            "https://chatgpt.com/backend-api/me"
        );
        assert_eq!(
            resolve_forward_url("/backend-api/codex/responses", false),
            "https://chatgpt.com/backend-api/codex/responses"
        );
        assert_eq!(
            resolve_forward_url_with_query(
                "/backend-api/models",
                Some("client_version=0.159"),
                false
            ),
            "https://chatgpt.com/backend-api/models?client_version=0.159"
        );
    }

    #[test]
    fn test_inject_loopback_base_url() {
        let args = vec![
            "exec".to_string(),
            "-c".to_string(),
            "chatgpt_base_url=\"http://old\"".to_string(),
            "-m".to_string(),
            "9router-subagent".to_string(),
        ];
        let updated = inject_loopback_base_url(&args, "20129");
        assert_eq!(
            updated,
            vec![
                "exec".to_string(),
                "-m".to_string(),
                "9router-subagent".to_string(),
                "-c".to_string(),
                "chatgpt_base_url=\"https://127.0.0.1:20129/backend-api/\"".to_string(),
            ]
        );
    }

    #[tokio::test]
    async fn test_ws_responses_returns_426_upgrade_required() {
        use tower::ServiceExt;
        let state = Arc::new(ProxyAppState {
            http_client: reqwest::Client::new(),
        });
        let app = create_router(state);

        let req = axum::extract::Request::builder()
            .uri("/backend-api/codex/responses")
            .method("GET")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::UPGRADE_REQUIRED);
    }

    #[tokio::test]
    async fn test_health_endpoint_returns_ok() {
        use tower::ServiceExt;
        let state = Arc::new(ProxyAppState {
            http_client: reqwest::Client::new(),
        });
        let app = create_router(state);

        let req = axum::extract::Request::builder()
            .uri("/health")
            .method("GET")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[test]
    fn test_rewrite_body_model_to_fallback() {
        let body = serde_json::to_vec(&serde_json::json!({
            "model": "implement",
            "input": "hello"
        }))
        .unwrap();
        let fallback = rewrite_body_model_to_fallback(&body).unwrap();
        let parsed: Value = serde_json::from_slice(&fallback).unwrap();
        assert_eq!(parsed["model"], map_role_to_model(Some("default")));

        // If already using default model, returns None
        let already_default = rewrite_body_model_to_fallback(&fallback);
        assert!(already_default.is_none());
    }
}
