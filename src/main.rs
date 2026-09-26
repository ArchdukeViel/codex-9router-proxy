use serde_json::Value;
use std::env;
use std::fs;
use std::io::{self, BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::str::FromStr;
use std::sync::Arc;
use std::thread;
use std::time::Duration;
use tiny_http::{Header, Method, Request, Response, Server, StatusCode};

/// Check if a given role name indicates a subagent.
pub fn is_subagent_role_name(r: &str) -> bool {
    let lower = r.to_lowercase();
    match lower.as_str() {
        "worker" | "explorer" | "default" | "subagent" | "sub-agent" | "sub_agent" | "reviewer" => true,
        "user" | "assistant" | "system" | "main" | "primary" => false,
        _ => true,
    }
}

/// Locate the active Codex config.toml path.
pub fn get_config_path() -> Option<PathBuf> {
    if let Ok(codex_home) = env::var("CODEX_HOME") {
        let p = PathBuf::from(codex_home).join("config.toml");
        if p.is_file() {
            return Some(p);
        }
    }
    if let Ok(profile) = env::var("USERPROFILE") {
        let p = PathBuf::from(profile).join(".codex").join("config.toml");
        if p.is_file() {
            return Some(p);
        }
    }
    if let Ok(home) = env::var("HOME") {
        let p = PathBuf::from(home).join(".codex").join("config.toml");
        if p.is_file() {
            return Some(p);
        }
    }
    None
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

/// Read model configuration for a specific role from ~/.codex/config.toml.
pub fn read_model_from_config(role: &str) -> Option<String> {
    let config_path = get_config_path()?;
    let content = fs::read_to_string(&config_path).ok()?;
    parse_model_from_toml(&content, role)
}

/// Map an agent role to a specialized model name with multi-tier resolution:
/// 1. Environment variable override: `CODEX_<ROLE>_MODEL`
/// 2. Generic environment variable override: `CODEX_SUBAGENT_MODEL`
/// 3. Active ~/.codex/config.toml: `[subagent_models.<role>]` or `[agents].default_subagent_model`
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

    // 3. Read from config.toml ([subagent_models.<role>] or [agents].default_subagent_model)
    if let Some(m) = read_model_from_config(&role_lower) {
        return m;
    }

    // 4. Built-in smart fallback defaults
    match role_lower.as_str() {
        "worker" => "implement".to_string(),
        "explorer" => "explore".to_string(),
        "reviewer" => "review".to_string(),
        _ => "9router-subagent".to_string(),
    }
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

    // 2. Check if codex.orig.exe exists in the directory of the current executable
    if let Ok(current_exe) = env::current_exe() {
        if let Some(parent) = current_exe.parent() {
            let orig = parent.join("codex.orig.exe");
            if orig.is_file() {
                return orig;
            }
        }
    }

    // 3. Scan bin/<hash> candidates in LOCALAPPDATA
    if let Ok(local_app_data) = env::var("LOCALAPPDATA") {
        let bin_dir = Path::new(&local_app_data).join("OpenAI").join("Codex").join("bin");
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
                        // Only add if it's the real large binary (> 10 MB) to avoid proxy loops
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
        if let Some(rls) = obj.get_mut("rateLimitsByLimitId").and_then(|r| r.as_object_mut()) {
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
                    if !m.is_empty() && (m == "implement" || m == "explore" || m == "review" || m == "9router-subagent") {
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

    let is_subagent = role_is_subagent || has_agent_nickname || source_is_subagent || nested_subagent;

    let model_str = params
        .get("model")
        .and_then(|m| m.as_str())
        .map(|s| s.to_string());

    let is_9router_model = if let Some(ref m) = model_str {
        m == "9router-subagent" || m.starts_with("9router") || m == "implement" || m == "explore" || m == "review"
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
                    || (is_subagent && (s.starts_with("gpt-") || s.starts_with("o1") || s.starts_with("o3") || s.starts_with("chatgpt")))
            }
            _ => false,
        };

        if needs_model {
            params.insert("model".to_string(), Value::String(mapped_model));
            modified = true;
        }
        if params.get("modelProvider").and_then(|v| v.as_str()) != Some(&target_provider) {
            params.insert("modelProvider".to_string(), Value::String(target_provider.clone()));
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
                println!("[FAIL] Endpoint socket unreachable: {} (error: {})", target, last_err);
            }
        }
        Err(e) => println!("[FAIL] Invalid endpoint address  : {} ({})", target, e),
    }
}

/// Check if a given model string corresponds to a 9Router or subagent model.
pub fn is_subagent_model_name(m: &str) -> bool {
    let lower = m.to_lowercase();
    lower == "9router-subagent"
        || lower.starts_with("9router")
        || lower == "implement"
        || lower == "explore"
        || lower == "review"
}

/// Check if a request path is an LLM responses endpoint.
pub fn is_responses_path(path: &str) -> bool {
    let clean_path = path.split('?').next().unwrap_or(path);
    clean_path == "/backend-api/codex/responses"
        || clean_path == "/backend-api/responses"
        || clean_path == "/codex/responses"
        || clean_path == "/responses"
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

    let is_subagent = model_is_subagent || has_subagent_marker || role_is_subagent || has_agent_nickname;

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

/// Convert a tiny_http Method into an HTTP verb string.
fn method_to_str(m: &Method) -> &str {
    match m {
        Method::Get => "GET",
        Method::Post => "POST",
        Method::Put => "PUT",
        Method::Delete => "DELETE",
        Method::Head => "HEAD",
        Method::Patch => "PATCH",
        Method::Options => "OPTIONS",
        Method::Connect => "CONNECT",
        Method::Trace => "TRACE",
        Method::NonStandard(s) => s.as_str(),
    }
}

/// Forward an upstream ureq::Response back to the tiny_http client.
fn send_upstream_response(request: Request, upstream_resp: ureq::Response) {
    let status = upstream_resp.status();
    let mut response_headers = Vec::new();

    for name in upstream_resp.headers_names() {
        let name_lower = name.to_lowercase();
        if name_lower == "transfer-encoding"
            || name_lower == "content-length"
            || name_lower == "content-encoding"
            || name_lower == "connection"
        {
            continue;
        }
        if let Some(val) = upstream_resp.header(&name) {
            let line = format!("{}: {}", name, val);
            if let Ok(h) = Header::from_str(&line) {
                response_headers.push(h);
            }
        }
    }

    let reader = upstream_resp.into_reader();
    let resp = Response::new(
        StatusCode(status),
        response_headers,
        reader,
        None,
        None,
    );
    let _ = request.respond(resp);
}

/// Process a single incoming HTTP request on the loopback reverse proxy.
pub fn handle_reverse_proxy_request(mut request: Request) {
    if request.method() == &Method::Options {
        let mut resp = Response::empty(200);
        if let Ok(h) = Header::from_str("Access-Control-Allow-Origin: *") { resp.add_header(h); }
        if let Ok(h) = Header::from_str("Access-Control-Allow-Methods: GET, POST, PUT, DELETE, OPTIONS, PATCH") { resp.add_header(h); }
        if let Ok(h) = Header::from_str("Access-Control-Allow-Headers: *") { resp.add_header(h); }
        let _ = request.respond(resp);
        return;
    }

    let path = request.url().to_string();
    let mut body_bytes = Vec::new();
    if let Some(len) = request.body_length() {
        body_bytes.reserve(len);
    }
    let _ = request.as_reader().read_to_end(&mut body_bytes);

    let (is_subagent, final_body) = inspect_and_route_http_request(&path, &body_bytes);
    let target_url = resolve_forward_url(&path, is_subagent);

    let mut forward_headers: Vec<(String, String)> = Vec::new();
    let mut client_auth: Option<String> = None;

    for h in request.headers() {
        let name = h.field.as_str().to_string();
        let val = h.value.as_str().to_string();
        let lower = name.to_lowercase();
        if lower == "host" || lower == "content-length" || lower == "connection" {
            continue;
        }
        if lower == "authorization" {
            client_auth = Some(val.clone());
        }
        forward_headers.push((name, val));
    }

    if is_subagent {
        let auth = get_subagent_auth_header()
            .or(client_auth)
            .unwrap_or_else(|| "Bearer dummy-9router-key".to_string());
        forward_headers.retain(|(k, _)| !k.eq_ignore_ascii_case("authorization"));
        forward_headers.push(("Authorization".to_string(), auth));
    }

    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(15))
        .build();

    let method_str = method_to_str(request.method());
    let mut ureq_req = agent.request(method_str, &target_url);

    for (k, v) in forward_headers {
        ureq_req = ureq_req.set(&k, &v);
    }

    let result = if final_body.is_empty() && (method_str == "GET" || method_str == "HEAD" || method_str == "OPTIONS" || method_str == "DELETE") {
        ureq_req.call()
    } else {
        ureq_req.send_bytes(&final_body)
    };

    match result {
        Ok(upstream_resp) => {
            send_upstream_response(request, upstream_resp);
        }
        Err(ureq::Error::Status(_status_code, upstream_resp)) => {
            send_upstream_response(request, upstream_resp);
        }
        Err(ureq::Error::Transport(err)) => {
            let err_body = serde_json::json!({
                "error": {
                    "message": format!("Codex 9Router Proxy upstream connection error to {}: {}", target_url, err),
                    "type": "proxy_gateway_error",
                    "code": 502
                }
            }).to_string();
            let mut resp = Response::from_string(err_body).with_status_code(502);
            if let Ok(h) = Header::from_str("Content-Type: application/json") {
                resp.add_header(h);
            }
            let _ = request.respond(resp);
        }
    }
}

/// Run the background request listener loop.
pub fn run_reverse_proxy_server(server: Arc<Server>) {
    loop {
        match server.recv() {
            Ok(request) => {
                thread::spawn(move || {
                    handle_reverse_proxy_request(request);
                });
            }
            Err(e) => {
                eprintln!("[codex-9router-proxy] Reverse proxy listener error: {}", e);
                break;
            }
        }
    }
}

/// Bind and spawn the embedded HTTP reverse proxy in a background thread.
pub fn spawn_embedded_reverse_proxy(bind_addr: &str) -> io::Result<Arc<Server>> {
    let server = Server::http(bind_addr)
        .map_err(|e| io::Error::new(io::ErrorKind::AddrInUse, e.to_string()))?;
    let server = Arc::new(server);
    let server_clone = Arc::clone(&server);
    thread::spawn(move || {
        run_reverse_proxy_server(server_clone);
    });
    Ok(server)
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
        println!("[OK] Official engine    : {} ({} bytes)", real_codex.display(), size);
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

    let proxy_port = get_proxy_port();
    let proxy_addr = format!("127.0.0.1:{}", proxy_port);
    let proxy_status = match TcpListener::bind(&proxy_addr) {
        Ok(_) => "Port available / ready to bind",
        Err(_) => "Port active / in use",
    };
    println!("[OK] Reverse Proxy Port : {} ({})", proxy_addr, proxy_status);
    println!("     - Loopback URL     : http://{}/backend-api/", proxy_addr);
    let subagent_url = get_subagent_responses_url();
    println!("     - Subagent Route   : Strict -> {}", subagent_url);
    let upstream_base = get_chatgpt_upstream_base();
    println!("     - Parent Route     : Upstream -> {}/backend-api/", upstream_base);

    if let Some(cfg) = get_config_path() {
        println!("[OK] Codex Config TOML  : {}", cfg.display());
        if let Ok(content) = fs::read_to_string(&cfg) {
            let def_m = parse_model_from_toml(&content, "default").unwrap_or_else(|| "9router-subagent".to_string());
            let worker_m = parse_model_from_toml(&content, "worker").unwrap_or_else(|| "implement".to_string());
            let explorer_m = parse_model_from_toml(&content, "explorer").unwrap_or_else(|| "explore".to_string());
            let reviewer_m = parse_model_from_toml(&content, "reviewer").unwrap_or_else(|| "review".to_string());
            println!("     - Default Model    : {}", def_m);
            println!("     - Worker Model     : {}", worker_m);
            println!("     - Explorer Model   : {}", explorer_m);
            println!("     - Reviewer Model   : {}", reviewer_m);
        }
    } else {
        println!("[WARN] Config TOML      : Not found in ~/.codex/config.toml");
    }

    if let Ok(profile) = env::var("USERPROFILE") {
        let agents_dir = PathBuf::from(profile).join(".codex").join("agents");
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

fn main() -> io::Result<()> {
    let args: Vec<String> = env::args().skip(1).collect();

    if args.iter().any(|a| a.eq_ignore_ascii_case("--doctor") || a.eq_ignore_ascii_case("doctor") || a.eq_ignore_ascii_case("-doctor")) {
        run_doctor();
        return Ok(());
    }

    let real_codex = find_real_codex();

    let is_daemon_subcommand = args.iter().any(|a| {
        a == "daemon"
            || a == "status"
            || a == "stop"
            || a == "restart"
            || a == "start"
            || a == "help"
            || a == "-h"
            || a == "--help"
    });
    let is_app_server = args.iter().any(|a| a == "app-server") && !is_daemon_subcommand;

    if !is_app_server {
        let mut forward_args = args.clone();
        let target_provider = get_target_model_provider();

        let uses_9router = forward_args.windows(2).any(|w| {
            (w[0] == "-m" || w[0] == "--model") && (w[1].contains("9router") || w[1] == "implement" || w[1] == "explore" || w[1] == "review")
        }) || forward_args.iter().any(|a| {
            a.starts_with("--model=") && (a.contains("9router") || a.contains("implement") || a.contains("explore") || a.contains("review"))
                || a.starts_with("-m=") && (a.contains("9router") || a.contains("implement") || a.contains("explore") || a.contains("review"))
                || (a.contains("model=") && (a.contains("9router") || a.contains("implement") || a.contains("explore") || a.contains("review")))
        });

        let has_provider_config = forward_args.iter().any(|a| a.contains("model_provider="));

        if uses_9router && !has_provider_config {
            forward_args.push("-c".to_string());
            forward_args.push(format!("model_provider=\"{}\"", target_provider));
        }

        let status = Command::new(&real_codex)
            .args(&forward_args)
            .status()?;
        std::process::exit(status.code().unwrap_or(1));
    }

    let port = get_proxy_port();
    let bind_addr = format!("127.0.0.1:{}", port);
    match spawn_embedded_reverse_proxy(&bind_addr) {
        Ok(_) => {
            eprintln!("[codex-9router-proxy] Embedded loopback reverse proxy listening on http://{}/backend-api/", bind_addr);
        }
        Err(e) => {
            eprintln!("[codex-9router-proxy] Reverse proxy note: Port {} in use or active: {}", bind_addr, e);
        }
    }

    let mut child_args = Vec::new();
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
        child_args.push(a.clone());
    }
    child_args.push("-c".to_string());
    child_args.push(format!("chatgpt_base_url=\"http://127.0.0.1:{}/backend-api/\"", port));

    let mut child = Command::new(&real_codex)
        .args(&child_args)
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
                        "thread/start" | "thread/fork" | "thread/resume" | "thread/settings/update" | "turn/start" => {
                            if let Some(params) = json.get_mut("params").and_then(|p| p.as_object_mut()) {
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

            if child_stdin.write_all(processed.as_bytes()).is_err() || child_stdin.flush().is_err() {
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

            if stdout_lock.write_all(processed.as_bytes()).is_err() || stdout_lock.flush().is_err() {
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
        assert_eq!(parse_model_from_toml(toml, "worker"), Some("my-worker".to_string()));
        assert_eq!(parse_model_from_toml(toml, "explorer"), Some("my-explorer".to_string()));
        assert_eq!(parse_model_from_toml(toml, "reviewer"), Some("my-reviewer".to_string()));
        assert_eq!(parse_model_from_toml(toml, "default"), Some("my-default".to_string()));
        assert_eq!(parse_model_from_toml(toml, "unknown_role"), Some("fallback-subagent".to_string()));
    }

    #[test]
    fn test_parse_model_from_toml_fallback_agents() {
        let toml = r#"
[agents]
default_subagent_model = "9router-subagent"
"#;
        assert_eq!(parse_model_from_toml(toml, "worker"), Some("9router-subagent".to_string()));
        assert_eq!(parse_model_from_toml(toml, "explorer"), Some("9router-subagent".to_string()));
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
        params.insert("agent_type".to_string(), Value::String("default".to_string()));
        let modded = route_thread_params(&mut params);
        assert!(modded);
        assert_eq!(params.get("modelProvider").unwrap(), "9router");
        assert_eq!(params.get("model_provider").unwrap(), "9router");
    }

    #[test]
    fn test_route_9router_model_explicit() {
        let mut params = serde_json::Map::new();
        params.insert("model".to_string(), Value::String("9router-subagent".to_string()));
        params.insert("modelProvider".to_string(), Value::String("openai".to_string()));
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
        params.insert("threadSource".to_string(), Value::String("subAgent".to_string()));
        let modded = route_thread_params(&mut params);
        assert!(modded);
        assert_eq!(params.get("modelProvider").unwrap(), "9router");
        assert_eq!(params.get("model_provider").unwrap(), "9router");
    }

    #[test]
    fn test_route_thread_source_object() {
        let mut spawn = serde_json::Map::new();
        spawn.insert("agent_role".to_string(), Value::String("worker".to_string()));
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
        params.insert("agentNickname".to_string(), Value::String("helpful-falcon".to_string()));
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
        let (is_subagent, routed_body) = inspect_and_route_http_request("/backend-api/codex/responses", &body_bytes);
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
        let (is_subagent, routed_body) = inspect_and_route_http_request("/backend-api/codex/responses", &body_bytes);
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
        let (is_subagent, routed_body) = inspect_and_route_http_request("/backend-api/codex/responses", &body_bytes);
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
        let (is_subagent, routed_body) = inspect_and_route_http_request("/backend-api/responses", &body_bytes);
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
        let (is_subagent, routed_body) = inspect_and_route_http_request("/backend-api/codex/responses", &body_bytes);
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
        let (is_subagent, routed_body) = inspect_and_route_http_request("/backend-api/me", &body_bytes);
        assert!(!is_subagent);
        assert_eq!(body_bytes, routed_body);
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
    }

    #[test]
    fn test_subagent_responses_url_default() {
        assert_eq!(get_subagent_responses_url(), "http://127.0.0.1:20128/v1/responses");
    }
}
