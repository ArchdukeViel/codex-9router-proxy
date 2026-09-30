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
/// Strictly excludes OpenAI message roles (`developer`, `user`, `assistant`, `system`, `tool`, `function`)
/// and root/main thread names (`main`, `primary`, `root`, `/root`).
pub fn is_subagent_role_name(r: &str) -> bool {
    let lower = r.trim().to_lowercase();
    if lower.is_empty() {
        return false;
    }
    match lower.as_str() {
        "user" | "assistant" | "system" | "developer" | "tool" | "function" | "main"
        | "primary" | "root" | "/root" => false,
        "worker" | "explorer" | "default" | "subagent" | "sub-agent" | "sub_agent" | "reviewer"
        | "collab_spawn" | "review" => true,
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

/// Check if a model identifier is a stock OpenAI / ChatGPT parent model
/// (`gpt-*`, `o1*`..`o9*`, `chatgpt*`, `codex-*`, `computer-use*`).
pub fn is_parent_chatgpt_model(model: &str) -> bool {
    let lower = model.trim().to_ascii_lowercase();
    let is_o_series = lower.starts_with('o')
        && lower
            .as_bytes()
            .get(1)
            .is_some_and(|b| b.is_ascii_digit());
    lower.starts_with("gpt-")
        || is_o_series
        || lower.starts_with("chatgpt")
        || lower.starts_with("codex-")
        || lower.starts_with("computer-use")
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

/// Parse top-level `model_provider = "..."` from a role manifest file (`~/.codex/agents/<role>.toml`).
pub fn parse_provider_from_role_toml(content: &str) -> Option<String> {
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('#') || trimmed.is_empty() {
            continue;
        }
        if trimmed.starts_with('[') {
            break;
        }
        if let Some((k, v)) = parse_toml_key_value(trimmed) {
            if k.eq_ignore_ascii_case("model_provider") {
                return Some(v);
            }
        }
    }
    None
}

/// Parse provider name from `[model_providers.<provider>]` in `~/.codex/config.toml`.
pub fn parse_provider_from_config_toml(content: &str) -> Option<String> {
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            let section = trimmed.trim_matches(|c| c == '[' || c == ']').trim();
            if let Some(rest) = section.strip_prefix("model_providers.") {
                let prov = rest.trim().trim_matches(|c| c == '"' || c == '\'');
                if !prov.is_empty() && !prov.eq_ignore_ascii_case("openai") {
                    return Some(prov.to_string());
                }
            }
        }
    }
    None
}

/// Parse `base_url = "..."` strictly from `[model_providers.<provider>]` in `~/.codex/config.toml`.
pub fn parse_provider_base_url_from_config_toml(content: &str, provider: &str) -> Option<String> {
    let prov_trimmed = provider.trim();
    if prov_trimmed.is_empty() {
        return None;
    }
    let target_section = format!("model_providers.{}", prov_trimmed);
    let mut in_target_provider = false;

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('#') || trimmed.is_empty() {
            continue;
        }
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            let section = trimmed.trim_matches(|c| c == '[' || c == ']').trim();
            let normalized_section = section.replace(['"', '\''], "");
            in_target_provider = normalized_section.eq_ignore_ascii_case(&target_section);
            continue;
        }
        if in_target_provider {
            if let Some((k, v)) = parse_toml_key_value(trimmed) {
                if k.eq_ignore_ascii_case("base_url") {
                    return Some(v);
                }
            }
        }
    }
    None
}

/// Parse `env_key = "..."` strictly from `[model_providers.<provider>]` in `~/.codex/config.toml`.
pub fn parse_provider_env_key_from_config_toml(content: &str, provider: &str) -> Option<String> {
    let prov_trimmed = provider.trim();
    if prov_trimmed.is_empty() {
        return None;
    }
    let target_section = format!("model_providers.{}", prov_trimmed);
    let mut in_target_provider = false;

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('#') || trimmed.is_empty() {
            continue;
        }
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            let section = trimmed.trim_matches(|c| c == '[' || c == ']').trim();
            let normalized_section = section.replace(['"', '\''], "");
            in_target_provider = normalized_section.eq_ignore_ascii_case(&target_section);
            continue;
        }
        if in_target_provider {
            if let Some((k, v)) = parse_toml_key_value(trimmed) {
                if k.eq_ignore_ascii_case("env_key") {
                    return Some(v);
                }
            }
        }
    }
    None
}

/// Read target subagent model provider for an optional specific role from `<codex_home>/agents/*.toml` or `<codex_home>/config.toml`.
pub fn read_provider_for_role_from_config_in_dir(
    codex_home: &Path,
    role: Option<&str>,
) -> Option<String> {
    if let Some(r) = role {
        let role_lower = normalize_subagent_role_token(r).unwrap_or_else(|| r.trim().to_lowercase());
        let role_file = codex_home.join("agents").join(format!("{}.toml", role_lower));
        if role_file.is_file() {
            if let Ok(content) = fs::read_to_string(&role_file) {
                if let Some(p) = parse_provider_from_role_toml(&content) {
                    if !p.is_empty() && !p.eq_ignore_ascii_case("openai") {
                        return Some(p);
                    }
                }
            }
        }
    }

    let mut first_role_provider: Option<String> = None;
    for r in ["default", "worker", "explorer", "reviewer"] {
        let role_file = codex_home.join("agents").join(format!("{}.toml", r));
        if role_file.is_file() {
            if let Ok(content) = fs::read_to_string(&role_file) {
                if let Some(p) = parse_provider_from_role_toml(&content) {
                    if !p.is_empty() && !p.eq_ignore_ascii_case("openai") {
                        if !p.eq_ignore_ascii_case("9router") {
                            return Some(p);
                        }
                        if first_role_provider.is_none() {
                            first_role_provider = Some(p);
                        }
                    }
                }
            }
        }
    }

    let agents_dir = codex_home.join("agents");
    if let Ok(entries) = fs::read_dir(&agents_dir) {
        let mut custom_files: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().and_then(|s| s.to_str()).is_some_and(|ext| ext.eq_ignore_ascii_case("toml")))
            .collect();
        custom_files.sort();
        for role_file in custom_files {
            if let Ok(content) = fs::read_to_string(&role_file) {
                if let Some(p) = parse_provider_from_role_toml(&content) {
                    if !p.is_empty() && !p.eq_ignore_ascii_case("openai") {
                        if !p.eq_ignore_ascii_case("9router") {
                            return Some(p);
                        }
                        if first_role_provider.is_none() {
                            first_role_provider = Some(p);
                        }
                    }
                }
            }
        }
    }

    let config_path = codex_home.join("config.toml");
    if let Ok(content) = fs::read_to_string(&config_path) {
        if let Some(cfg_prov) = parse_provider_from_config_toml(&content) {
            if !cfg_prov.eq_ignore_ascii_case("9router") || first_role_provider.is_none() {
                return Some(cfg_prov);
            }
        }
    }

    first_role_provider
}

/// Read target subagent model provider from `<codex_home>/agents/*.toml` or `<codex_home>/config.toml`.
pub fn read_provider_from_config_in_dir(codex_home: &Path) -> Option<String> {
    read_provider_for_role_from_config_in_dir(codex_home, None)
}

/// Read target subagent model provider from `~/.codex/agents/*.toml` or `~/.codex/config.toml`.
pub fn read_provider_from_config() -> Option<String> {
    let codex_home = get_codex_home_dir()?;
    read_provider_from_config_in_dir(&codex_home)
}

/// Read the top-level primary parent model (`model = "..."`) from `<codex_home>/config.toml`.
pub fn read_primary_model_from_config_in_dir(codex_home: &Path) -> Option<String> {
    let config_path = codex_home.join("config.toml");
    let content = fs::read_to_string(&config_path).ok()?;
    let m = parse_model_from_role_toml(&content)?;
    let trimmed = m.trim();
    if !trimmed.is_empty() && is_parent_chatgpt_model(trimmed) {
        Some(trimmed.to_string())
    } else {
        None
    }
}

/// Read the top-level primary parent model (`model = "..."`) from `~/.codex/config.toml`.
pub fn read_primary_model_from_config() -> Option<String> {
    let codex_home = get_codex_home_dir()?;
    read_primary_model_from_config_in_dir(&codex_home)
}

/// Parse all subagent model values from `[subagent_models]` and `[agents].default_subagent_model` in `config.toml`.
pub fn parse_all_subagent_models_from_config_toml(content: &str) -> Vec<String> {
    let mut in_subagent_models = false;
    let mut in_agents = false;
    let mut models = Vec::new();

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
            if let Some((_k, v)) = parse_toml_key_value(trimmed) {
                let m = v.trim();
                if !m.is_empty()
                    && !is_parent_chatgpt_model(m)
                    && !models.iter().any(|existing: &String| existing.eq_ignore_ascii_case(m))
                {
                    models.push(m.to_string());
                }
            }
        } else if in_agents {
            if let Some((k, v)) = parse_toml_key_value(trimmed) {
                if k.eq_ignore_ascii_case("default_subagent_model") {
                    let m = v.trim();
                    if !m.is_empty()
                        && !is_parent_chatgpt_model(m)
                        && !models.iter().any(|existing: &String| existing.eq_ignore_ascii_case(m))
                    {
                        models.push(m.to_string());
                    }
                }
            }
        }
    }

    models
}

/// Collect all configured subagent models across standard roles, custom `<codex_home>/agents/*.toml` manifests,
/// and `<codex_home>/config.toml` (`[subagent_models]` & `default_subagent_model`).
pub fn collect_configured_subagent_models_in_dir(codex_home: Option<&Path>) -> Vec<String> {
    let mut models: Vec<String> = Vec::new();
    let mut push_unique = |m: String| {
        let trimmed = m.trim();
        if !trimmed.is_empty()
            && !is_parent_chatgpt_model(trimmed)
            && !models
                .iter()
                .any(|existing| existing.eq_ignore_ascii_case(trimmed))
        {
            models.push(trimmed.to_string());
        }
    };

    for role in ["default", "worker", "explorer", "reviewer"] {
        if let Some(dir) = codex_home {
            if let Some(m) = read_model_from_config_in_dir(dir, role) {
                push_unique(m);
            }
        }
        push_unique(map_role_to_model(Some(role)));
    }

    if let Some(dir) = codex_home {
        let agents_dir = dir.join("agents");
        if let Ok(entries) = fs::read_dir(&agents_dir) {
            let mut toml_files: Vec<PathBuf> = entries
                .flatten()
                .map(|e| e.path())
                .filter(|p| {
                    p.extension()
                        .and_then(|s| s.to_str())
                        .is_some_and(|ext| ext.eq_ignore_ascii_case("toml"))
                })
                .collect();
            toml_files.sort();
            for path in toml_files {
                if let Ok(content) = fs::read_to_string(&path) {
                    if let Some(m) = parse_model_from_role_toml(&content) {
                        push_unique(m);
                    }
                }
            }
        }
        let config_path = dir.join("config.toml");
        if let Ok(content) = fs::read_to_string(&config_path) {
            for m in parse_all_subagent_models_from_config_toml(&content) {
                push_unique(m);
            }
        }
    }

    models
}

/// Find the subagent role name (`"worker"`, `"explorer"`, `"reviewer"`, `"default"`, or custom role in `agents/*.toml`
/// or `[subagent_models]`) whose configured model matches `model_name`.
pub fn find_role_for_model_in_dir(codex_home: Option<&Path>, model_name: &str) -> Option<String> {
    let target = model_name.trim();
    if target.is_empty() || is_parent_chatgpt_model(target) {
        return None;
    }
    if let Some(dir) = codex_home {
        for role in ["worker", "explorer", "reviewer", "default"] {
            if let Some(m) = read_model_from_config_in_dir(dir, role) {
                if m.trim().eq_ignore_ascii_case(target) {
                    return Some(role.to_string());
                }
            }
        }
        let agents_dir = dir.join("agents");
        if let Ok(entries) = fs::read_dir(&agents_dir) {
            let mut toml_files: Vec<PathBuf> = entries
                .flatten()
                .map(|e| e.path())
                .filter(|p| {
                    p.extension()
                        .and_then(|s| s.to_str())
                        .is_some_and(|ext| ext.eq_ignore_ascii_case("toml"))
                })
                .collect();
            toml_files.sort();
            for path in toml_files {
                if let Ok(content) = fs::read_to_string(&path) {
                    if let Some(m) = parse_model_from_role_toml(&content) {
                        if m.trim().eq_ignore_ascii_case(target) {
                            if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                                return Some(stem.to_lowercase());
                            }
                        }
                    }
                }
            }
        }
        let config_path = dir.join("config.toml");
        if let Ok(content) = fs::read_to_string(&config_path) {
            let mut in_subagent_models = false;
            for line in content.lines() {
                let trimmed = line.trim();
                if trimmed.starts_with('#') || trimmed.is_empty() {
                    continue;
                }
                if trimmed.starts_with('[') && trimmed.ends_with(']') {
                    let section = trimmed.trim_matches(|c| c == '[' || c == ']').trim();
                    in_subagent_models = section.eq_ignore_ascii_case("subagent_models");
                    continue;
                }
                if in_subagent_models {
                    if let Some((k, v)) = parse_toml_key_value(trimmed) {
                        if v.trim().eq_ignore_ascii_case(target) {
                            return normalize_subagent_role_token(&k).or_else(|| Some(k.to_lowercase()));
                        }
                    }
                }
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

/// Read model configuration for a specific role from `<codex_home>/agents/<role>.toml` or `<codex_home>/config.toml`.
pub fn read_model_from_config_in_dir(codex_home: &Path, role: &str) -> Option<String> {
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
    let config_path = codex_home.join("config.toml");
    if let Ok(content) = fs::read_to_string(&config_path) {
        if let Some(m) = parse_model_from_toml(&content, role) {
            return Some(m);
        }
    }
    if !role.eq_ignore_ascii_case("default") {
        let default_file = codex_home.join("agents").join("default.toml");
        if default_file.is_file() {
            if let Ok(content) = fs::read_to_string(&default_file) {
                if let Some(m) = parse_model_from_role_toml(&content) {
                    return Some(m);
                }
            }
        }
    }
    None
}

/// Read model configuration for a specific role from `~/.codex/agents/<role>.toml` or `~/.codex/config.toml`.
pub fn read_model_from_config(role: &str) -> Option<String> {
    let codex_home = get_codex_home_dir()?;
    read_model_from_config_in_dir(&codex_home, role)
}

/// Built-in default model name for a given role without consulting env/disk.
/// All roles default to `"9router-subagent"` unless overridden by env or config.
pub fn builtin_role_model(_role: &str) -> &'static str {
    "9router-subagent"
}

/// Map an agent role to a specialized model name with multi-tier resolution:
/// 1. Explicit process environment variable override: `CODEX_<ROLE>_MODEL` or `CODEX_SUBAGENT_MODEL`
///    (when set in process env and not merely inherited unchanged from `HKCU\Environment`)
/// 2. Active `~/.codex/agents/<role>.toml` or `~/.codex/config.toml` (`read_model_from_config`)
/// 3. Persisted Windows User Registry / environment variable (`CODEX_<ROLE>_MODEL`, `CODEX_DEFAULT_MODEL`)
/// 4. Built-in default: `9router-subagent`
pub fn map_role_to_model(role: Option<&str>) -> String {
    let role_str = role.unwrap_or("default");
    let role_lower =
        normalize_subagent_role_token(role_str).unwrap_or_else(|| role_str.trim().to_lowercase());

    let env_role = format!("CODEX_{}_MODEL", role_lower.to_uppercase());
    let proc_role_val = env::var(&env_role)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let mut reg_role_val: Option<String> = None;

    // 1. Explicit process-level role override (when not merely inherited unchanged from HKCU\Environment)
    if let Some(ref m) = proc_role_val {
        reg_role_val = get_user_env_var(&env_role)
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        if reg_role_val.as_deref() != Some(m.as_str()) {
            return m.clone();
        }
    }

    // 2. Generic subagent model process env var: CODEX_SUBAGENT_MODEL
    let proc_generic_val = env::var("CODEX_SUBAGENT_MODEL")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let mut reg_generic_val: Option<String> = None;
    if let Some(ref m) = proc_generic_val {
        reg_generic_val = get_user_env_var("CODEX_SUBAGENT_MODEL")
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        if reg_generic_val.as_deref() != Some(m.as_str()) {
            return m.clone();
        }
    }

    // 3. Read from ~/.codex/agents/<role>.toml or ~/.codex/config.toml
    if let Some(m) = read_model_from_config(&role_lower) {
        return m;
    }

    // 4. Persisted environment / Windows User Registry fallback
    if let Some(m) = proc_role_val.or_else(|| {
        reg_role_val.or_else(|| {
            get_user_env_var(&env_role)
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
        })
    }) {
        return m;
    }
    if let Some(m) = proc_generic_val.or_else(|| {
        reg_generic_val.or_else(|| {
            get_user_env_var("CODEX_SUBAGENT_MODEL")
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
        })
    }) {
        return m;
    }
    if let Some(m) = env::var("CODEX_DEFAULT_MODEL")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .or_else(|| {
            get_user_env_var("CODEX_DEFAULT_MODEL")
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
        })
    {
        return m;
    }

    // 5. Built-in default
    builtin_role_model(&role_lower).to_string()
}

/// Decode standard or URL-safe base64 into a UTF-8 string (with or without `=` padding).
pub fn decode_base64_utf8(input: &str) -> Option<String> {
    let trimmed = input.trim();
    if trimmed.is_empty() || trimmed.starts_with('{') || trimmed.starts_with('[') {
        return None;
    }
    let mut buf = Vec::with_capacity(trimmed.len() * 3 / 4 + 3);
    let mut acc: u32 = 0;
    let mut bits: u8 = 0;
    for b in trimmed.bytes() {
        if b == b'=' || b.is_ascii_whitespace() {
            continue;
        }
        let val: u32 = match b {
            b'A'..=b'Z' => (b - b'A') as u32,
            b'a'..=b'z' => (b - b'a' + 26) as u32,
            b'0'..=b'9' => (b - b'0' + 52) as u32,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            _ => return None,
        };
        acc = (acc << 6) | val;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            buf.push(((acc >> bits) & 0xFF) as u8);
        }
    }
    if buf.is_empty() {
        return None;
    }
    String::from_utf8(buf).ok()
}

/// Normalize a raw subagent role or `agent_name` path (e.g. `"/root/explorer"`, `"review"`, `"collab_spawn"`)
/// into a canonical role name (`"worker"`, `"explorer"`, `"reviewer"`, `"default"`, or custom role).
pub fn normalize_subagent_role_token(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let lower = trimmed.to_lowercase();
    if matches!(
        lower.as_str(),
        "user" | "assistant" | "system" | "developer" | "tool" | "function" | "main"
            | "primary" | "root" | "/root"
    ) {
        return None;
    }
    let segment = lower
        .strip_prefix("/root/")
        .or_else(|| lower.strip_prefix("root/"))
        .or_else(|| lower.strip_prefix('/'))
        .unwrap_or(lower.as_str())
        .split('/')
        .next_back()
        .unwrap_or("")
        .trim();
    if segment.is_empty()
        || matches!(
            segment,
            "user" | "assistant" | "system" | "developer" | "tool" | "function" | "main"
                | "primary" | "root"
        )
    {
        return None;
    }
    let base_segment = segment
        .trim_end_matches(|c: char| c.is_ascii_digit())
        .trim_end_matches(['_', '-']);
    let effective = if matches!(
        base_segment,
        "review" | "reviewer" | "worker" | "implement" | "explorer" | "explore" | "default"
    ) {
        base_segment
    } else {
        segment
    };
    match effective {
        "review" | "reviewer" => Some("reviewer".to_string()),
        "worker" | "implement" => Some("worker".to_string()),
        "explorer" | "explore" => Some("explorer".to_string()),
        "collab_spawn" | "thread_spawn" | "subagent" | "sub-agent" | "sub_agent" | "compact"
        | "memory_consolidation" | "default" => Some("default".to_string()),
        other if is_subagent_role_name(other) => Some(other.to_string()),
        _ => None,
    }
}

fn extract_role_from_turn_metadata_value(parsed: &Value) -> Option<String> {
    let obj = parsed.as_object()?;
    for key in [
        "agent_role",
        "agentRole",
        "agent_type",
        "agentType",
        "subagent_role",
        "subagentRole",
        "agent_name",
        "agentName",
    ] {
        if let Some(s) = obj.get(key).and_then(|v| v.as_str()) {
            if let Some(norm) = normalize_subagent_role_token(s) {
                return Some(norm);
            }
        }
    }
    if let Some(s) = obj
        .get("subagent_kind")
        .or_else(|| obj.get("subagentKind"))
        .and_then(|v| v.as_str())
    {
        if let Some(norm) = normalize_subagent_role_token(s) {
            return Some(norm);
        }
    }
    None
}

fn extract_role_from_turn_metadata_str(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Ok(parsed) = serde_json::from_str::<Value>(trimmed) {
        if let Some(role) = extract_role_from_turn_metadata_value(&parsed) {
            return Some(role);
        }
    } else if let Some(decoded) = decode_base64_utf8(trimmed) {
        if let Ok(parsed) = serde_json::from_str::<Value>(decoded.trim()) {
            if let Some(role) = extract_role_from_turn_metadata_value(&parsed) {
                return Some(role);
            }
        }
    }
    None
}

/// Extract the most specific subagent role from HTTP headers (`x-codex-turn-metadata` and `x-openai-subagent`).
pub fn extract_role_from_http_headers(headers: &HeaderMap) -> Option<String> {
    let mut fallback_default = false;
    for val in headers.get_all("x-codex-turn-metadata") {
        if let Ok(turn_meta) = val.to_str() {
            if let Some(role) = extract_role_from_turn_metadata_str(turn_meta) {
                if role != "default" {
                    return Some(role);
                }
                fallback_default = true;
            }
        }
    }
    if let Some(sub_hdr) = headers.get("x-openai-subagent").and_then(|v| v.to_str().ok()) {
        if let Some(role) = normalize_subagent_role_token(sub_hdr) {
            if role != "default" {
                return Some(role);
            }
            fallback_default = true;
        }
    }
    if fallback_default {
        Some("default".to_string())
    } else {
        None
    }
}

fn is_subagent_turn_metadata_value(parsed: &Value) -> bool {
    let Some(obj) = parsed.as_object() else {
        return false;
    };
    if let Some(src) = obj.get("thread_source").or_else(|| obj.get("threadSource")) {
        if src
            .as_str()
            .is_some_and(|s| s.eq_ignore_ascii_case("subagent"))
            || contains_subagent_source(src)
        {
            return true;
        }
    }
    if let Some(kind) = obj.get("subagent_kind").or_else(|| obj.get("subagentKind")) {
        match kind {
            Value::String(s) if !s.trim().is_empty() => return true,
            Value::Object(m) if !m.is_empty() => return true,
            _ => {}
        }
    }
    for key in [
        "parent_thread_id",
        "parentThreadId",
        "parent_turn_id",
        "parentTurnId",
        "forked_from_thread_id",
        "forkedFromThreadId",
        "x-openai-subagent",
        "x-codex-parent-thread-id",
    ] {
        if obj
            .get(key)
            .and_then(|v| v.as_str())
            .is_some_and(|s| !s.trim().is_empty())
        {
            return true;
        }
    }
    if extract_role_from_turn_metadata_value(parsed).is_some() {
        return true;
    }
    false
}

fn is_subagent_turn_metadata_str(turn_meta: &str) -> bool {
    let trimmed = turn_meta.trim();
    if trimmed.is_empty() {
        return false;
    }
    if let Ok(parsed) = serde_json::from_str::<Value>(trimmed) {
        return is_subagent_turn_metadata_value(&parsed);
    }
    if let Some(decoded) = decode_base64_utf8(trimmed) {
        if is_subagent_turn_metadata_str(&decoded) {
            return true;
        }
    }
    let lower = trimmed.to_ascii_lowercase();
    lower.contains("\"thread_source\":\"subagent\"")
        || lower.contains("\"thread_source\": \"subagent\"")
        || lower.contains("\"thread_source\":\"thread_spawn\"")
        || lower.contains("\"thread_source\": \"thread_spawn\"")
        || lower.contains("\"subagent_kind\":\"")
        || lower.contains("\"subagent_kind\": \"")
        || lower.contains("\"subagent_kind\":{")
        || lower.contains("\"subagent_kind\": {")
        || lower.contains("\"parent_thread_id\":\"")
        || lower.contains("\"parent_thread_id\": \"")
        || lower.contains("\"parent_turn_id\":\"")
        || lower.contains("\"parent_turn_id\": \"")
        || lower.contains("\"forked_from_thread_id\":\"")
        || lower.contains("\"forked_from_thread_id\": \"")
}

/// Check if HTTP headers indicate a spawned subagent request (`x-openai-subagent`, `x-codex-parent-thread-id`,
/// or `x-codex-turn-metadata` carrying subagent metadata in raw JSON or base64).
pub fn is_subagent_http_headers(headers: &HeaderMap) -> bool {
    for key in ["x-openai-subagent", "x-codex-parent-thread-id"] {
        if let Some(val) = headers.get(key).and_then(|v| v.to_str().ok()) {
            if !val.trim().is_empty() {
                return true;
            }
        }
    }
    for val in headers.get_all("x-codex-turn-metadata") {
        if let Ok(turn_meta) = val.to_str() {
            if is_subagent_turn_metadata_str(turn_meta) {
                return true;
            }
        }
    }
    false
}

/// Check if a JSON value contains markers indicating a subagent thread source or subagent `client_metadata`.
pub fn contains_subagent_source(v: &Value) -> bool {
    match v {
        Value::String(s) => {
            let lower = s.to_lowercase();
            lower.contains("subagent")
                || lower.contains("sub_agent")
                || lower.contains("sub-agent")
                || lower.contains("thread_spawn")
                || lower.contains("collab_spawn")
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
            for k in [
                "x-openai-subagent",
                "x-codex-parent-thread-id",
                "parent_turn_id",
                "parentTurnId",
                "parent_thread_id",
                "parentThreadId",
                "forked_from_thread_id",
                "forkedFromThreadId",
            ] {
                if let Some(s) = map.get(k).and_then(|x| x.as_str()) {
                    if !s.trim().is_empty() {
                        return true;
                    }
                }
            }
            if let Some(turn_meta) = map.get("x-codex-turn-metadata").and_then(|x| x.as_str()) {
                if is_subagent_turn_metadata_str(turn_meta) {
                    return true;
                }
            }
            if let Some(r) = map
                .get("agent_role")
                .or_else(|| map.get("agentRole"))
                .or_else(|| map.get("agent_type"))
                .or_else(|| map.get("agentType"))
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
            if let Some(cm) = map.get("client_metadata").or_else(|| map.get("clientMetadata")) {
                if contains_subagent_source(cm) {
                    return true;
                }
            }
            false
        }
        _ => false,
    }
}

/// Retrieve the configured target model provider for an optional subagent role inside an explicit `<codex_home>` (defaults to "9router").
pub fn get_target_model_provider_for_role_in_dir(
    codex_home: Option<&Path>,
    role: Option<&str>,
) -> String {
    let proc_val = env::var("CODEX_SUBAGENT_PROVIDER")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let mut reg_val: Option<String> = None;

    if let Some(ref p) = proc_val {
        reg_val = get_user_env_var("CODEX_SUBAGENT_PROVIDER")
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        if reg_val.as_deref() != Some(p.as_str()) {
            return p.clone();
        }
    }

    if let Some(dir) = codex_home {
        if let Some(cfg_prov) = read_provider_for_role_from_config_in_dir(dir, role) {
            return cfg_prov;
        }
    }

    if let Some(p) = proc_val.or_else(|| {
        reg_val.or_else(|| {
            get_user_env_var("CODEX_SUBAGENT_PROVIDER")
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
        })
    }) {
        return p;
    }

    "9router".to_string()
}

/// Retrieve the configured target model provider for an optional subagent role (defaults to "9router").
pub fn get_target_model_provider_for_role(role: Option<&str>) -> String {
    let codex_home = get_codex_home_dir();
    get_target_model_provider_for_role_in_dir(codex_home.as_deref(), role)
}

/// Retrieve the configured target model provider (defaults to "9router").
pub fn get_target_model_provider() -> String {
    get_target_model_provider_for_role(None)
}

/// Parse the value of a registry key from `reg query HKCU\Environment /v <name>` output.
pub fn parse_reg_query_value(output: &str, name: &str) -> Option<String> {
    for line in output.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with("HKEY_") {
            continue;
        }
        let mut parts = trimmed.split_whitespace();
        let key_name = parts.next()?;
        if !key_name.eq_ignore_ascii_case(name) {
            continue;
        }
        let reg_type = parts.next()?;
        if !reg_type.to_uppercase().starts_with("REG_") {
            continue;
        }
        if let Some(type_pos) = trimmed.find(reg_type) {
            let val = trimmed[type_pos + reg_type.len()..].trim();
            if !val.is_empty() {
                return Some(val.to_string());
            }
        }
    }
    None
}

#[cfg(windows)]
fn query_hkcu_string_value_win32(subkey_path: &str, value_name: &str) -> Option<String> {
    use std::ffi::{OsStr, OsString};
    use std::os::windows::ffi::{OsStrExt, OsStringExt};

    #[link(name = "advapi32")]
    extern "system" {
        fn RegOpenKeyExW(
            h_key: isize,
            lp_sub_key: *const u16,
            ul_options: u32,
            sam_desired: u32,
            phk_result: *mut isize,
        ) -> i32;
        fn RegQueryValueExW(
            h_key: isize,
            lp_value_name: *const u16,
            lp_reserved: *const u32,
            lp_type: *mut u32,
            lp_data: *mut u8,
            lpcb_data: *mut u32,
        ) -> i32;
        fn RegCloseKey(h_key: isize) -> i32;
    }

    const HKEY_CURRENT_USER: isize = -2147483647i32 as isize; // 0x80000001
    const KEY_READ: u32 = 0x20019;
    const REG_SZ: u32 = 1;
    const REG_EXPAND_SZ: u32 = 2;

    let subkey: Vec<u16> = OsStr::new(subkey_path)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let val_name: Vec<u16> = OsStr::new(value_name)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();

    unsafe {
        let mut hkey: isize = 0;
        if RegOpenKeyExW(HKEY_CURRENT_USER, subkey.as_ptr(), 0, KEY_READ, &mut hkey) != 0
            || hkey == 0
        {
            return None;
        }
        let mut val_type: u32 = 0;
        let mut data_len: u32 = 0;
        let status = RegQueryValueExW(
            hkey,
            val_name.as_ptr(),
            std::ptr::null(),
            &mut val_type,
            std::ptr::null_mut(),
            &mut data_len,
        );
        if status != 0 || (val_type != REG_SZ && val_type != REG_EXPAND_SZ) || data_len < 2 {
            let _ = RegCloseKey(hkey);
            return None;
        }
        let mut buf = vec![0u16; (data_len as usize).div_ceil(2)];
        let mut actual_len = (buf.len() * 2) as u32;
        let status2 = RegQueryValueExW(
            hkey,
            val_name.as_ptr(),
            std::ptr::null(),
            &mut val_type,
            buf.as_mut_ptr() as *mut u8,
            &mut actual_len,
        );
        let _ = RegCloseKey(hkey);
        if status2 != 0 {
            return None;
        }
        let mut u16_count = actual_len as usize / 2;
        while u16_count > 0 && buf[u16_count - 1] == 0 {
            u16_count -= 1;
        }
        let s = OsString::from_wide(&buf[..u16_count])
            .to_string_lossy()
            .trim()
            .to_string();
        if s.is_empty() {
            None
        } else {
            Some(s)
        }
    }
}

#[cfg(windows)]
fn query_hkcu_environment_win32(name: &str) -> Option<String> {
    query_hkcu_string_value_win32("Environment", name)
}

/// Query a Windows User Environment variable from `HKCU\Environment`.
pub fn get_user_env_var(name: &str) -> Option<String> {
    #[cfg(windows)]
    {
        query_hkcu_environment_win32(name)
    }
    #[cfg(not(windows))]
    {
        let _ = name;
        None
    }
}

/// Extract a Windows Package Family Name (`<Name>_<PublisherId>`) from a Package Full Name
/// (e.g. `"OpenAI.Codex_26.924.2738.0_x64__2p2nqsd0c76g0"` -> `"OpenAI.Codex_2p2nqsd0c76g0"`).
pub fn extract_package_family_name_from_full_name(full_name: &str) -> Option<String> {
    let trimmed = full_name.trim();
    if trimmed.is_empty() {
        return None;
    }
    let parts: Vec<&str> = trimmed.split('_').collect();
    if parts.len() < 2 {
        return None;
    }
    let pkg_name = parts.first()?.trim();
    let publisher_id = parts.last()?.trim();
    if pkg_name.is_empty() || publisher_id.is_empty() {
        return None;
    }
    Some(format!("{}_{}", pkg_name, publisher_id))
}

/// Parse a `u64` version tuple `(major, minor, build, revision)` from a Package Full Name
/// (e.g. `"OpenAI.Codex_26.924.2738.0_x64__2p2nqsd0c76g0"` -> `(26, 924, 2738, 0)`).
pub fn parse_package_version_tuple(full_name: &str) -> (u64, u64, u64, u64) {
    let parts: Vec<&str> = full_name.trim().split('_').collect();
    let ver_str = if parts.len() >= 2 { parts[1] } else { full_name.trim() };
    let mut nums = ver_str.split('.').map(|s| s.parse::<u64>().unwrap_or(0));
    (
        nums.next().unwrap_or(0),
        nums.next().unwrap_or(0),
        nums.next().unwrap_or(0),
        nums.next().unwrap_or(0),
    )
}

#[cfg(windows)]
fn query_hkcu_codex_appmodel_packages() -> Vec<(String, PathBuf)> {
    use std::ffi::{OsStr, OsString};
    use std::os::windows::ffi::{OsStrExt, OsStringExt};

    #[link(name = "advapi32")]
    extern "system" {
        fn RegOpenKeyExW(
            h_key: isize,
            lp_sub_key: *const u16,
            ul_options: u32,
            sam_desired: u32,
            phk_result: *mut isize,
        ) -> i32;
        fn RegEnumKeyExW(
            h_key: isize,
            dw_index: u32,
            lp_name: *mut u16,
            lpcch_name: *mut u32,
            lp_reserved: *const u32,
            lp_class: *mut u16,
            lpcch_class: *mut u32,
            lpft_last_write_time: *mut u64,
        ) -> i32;
        fn RegCloseKey(h_key: isize) -> i32;
    }

    const HKEY_CURRENT_USER: isize = -2147483647i32 as isize;
    const KEY_READ: u32 = 0x20019;
    const BASE_SUBKEY: &str =
        "Software\\Classes\\Local Settings\\Software\\Microsoft\\Windows\\CurrentVersion\\AppModel\\Repository\\Packages";

    let subkey_wide: Vec<u16> = OsStr::new(BASE_SUBKEY)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();

    let mut matching_names = Vec::new();
    unsafe {
        let mut hkey: isize = 0;
        if RegOpenKeyExW(
            HKEY_CURRENT_USER,
            subkey_wide.as_ptr(),
            0,
            KEY_READ,
            &mut hkey,
        ) == 0
            && hkey != 0
        {
            let mut idx: u32 = 0;
            loop {
                let mut name_buf = [0u16; 260];
                let mut name_len = name_buf.len() as u32;
                let mut last_write: u64 = 0;
                let status = RegEnumKeyExW(
                    hkey,
                    idx,
                    name_buf.as_mut_ptr(),
                    &mut name_len,
                    std::ptr::null(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    &mut last_write,
                );
                if status != 0 {
                    break;
                }
                let name = OsString::from_wide(&name_buf[..name_len as usize])
                    .to_string_lossy()
                    .into_owned();
                if name.to_ascii_lowercase().starts_with("openai.codex") {
                    matching_names.push(name);
                }
                idx += 1;
            }
            let _ = RegCloseKey(hkey);
        }
    }

    // Sort by parsed version descending so newest package comes first
    matching_names.sort_by_key(|a| std::cmp::Reverse(parse_package_version_tuple(a)));

    let mut results = Vec::new();
    for pkg_name in matching_names {
        let full_subkey = format!("{}\\{}", BASE_SUBKEY, pkg_name);
        if let Some(root_str) = query_hkcu_string_value_win32(&full_subkey, "PackageRootFolder") {
            let root_path = PathBuf::from(root_str);
            if root_path.is_dir() {
                results.push((pkg_name, root_path));
            }
        }
    }
    results
}

/// Resolve the active `OpenAI.Codex` Package Family Name dynamically from `HKCU` AppModel registry
/// or `%LOCALAPPDATA%\Packages`, falling back to `"OpenAI.Codex_2p2nqsd0c76g0"`.
pub fn resolve_codex_package_family_name() -> String {
    #[cfg(windows)]
    {
        for (pkg_full_name, _) in query_hkcu_codex_appmodel_packages() {
            let lower = pkg_full_name.to_ascii_lowercase();
            if lower.starts_with("openai.codex_") {
                if let Some(pfn) = extract_package_family_name_from_full_name(&pkg_full_name) {
                    return pfn;
                }
            }
        }
    }
    if let Ok(local_appdata) = env::var("LOCALAPPDATA") {
        let packages_dir = PathBuf::from(local_appdata).join("Packages");
        if let Ok(entries) = fs::read_dir(packages_dir) {
            for entry in entries.flatten() {
                if let Some(name) = entry.file_name().to_str() {
                    let lower = name.to_ascii_lowercase();
                    if lower.starts_with("openai.codex_") {
                        return name.to_string();
                    }
                }
            }
        }
    }
    "OpenAI.Codex_2p2nqsd0c76g0".to_string()
}

/// Discover all candidate directories that may contain official stock `codex.exe` (`> 10 MB`)
/// and companion helper binaries (`rg.exe`, `codex-command-runner.exe`, `codex-code-mode-host.exe`,
/// `codex-windows-sandbox-setup.exe`, `codex-windows-sandbox-service.exe`).
pub fn discover_codex_binary_candidate_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    let mut push_dir = |p: PathBuf| {
        if p.is_dir() && !dirs.contains(&p) {
            dirs.push(p);
        }
    };

    #[cfg(windows)]
    {
        for (_pkg_name, pkg_root) in query_hkcu_codex_appmodel_packages() {
            push_dir(pkg_root.join("app").join("resources"));
            push_dir(pkg_root);
        }
        let win_apps = Path::new("C:\\Program Files\\WindowsApps");
        if let Ok(entries) = fs::read_dir(win_apps) {
            let mut wa_dirs: Vec<PathBuf> = entries
                .flatten()
                .filter(|e| {
                    e.file_name()
                        .to_str()
                        .is_some_and(|n| n.to_ascii_lowercase().starts_with("openai.codex"))
                })
                .map(|e| e.path())
                .collect();
            wa_dirs.sort_by(|a, b| {
                let na = a.file_name().and_then(|s| s.to_str()).unwrap_or("");
                let nb = b.file_name().and_then(|s| s.to_str()).unwrap_or("");
                parse_package_version_tuple(nb).cmp(&parse_package_version_tuple(na))
            });
            for d in wa_dirs {
                push_dir(d.join("app").join("resources"));
                push_dir(d);
            }
        }
    }

    if let Ok(local_app_data) = env::var("LOCALAPPDATA") {
        let bin_dir = Path::new(&local_app_data)
            .join("OpenAI")
            .join("Codex")
            .join("bin");
        if let Ok(entries) = fs::read_dir(&bin_dir) {
            let mut hash_dirs: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
            hash_dirs.sort_by(|a, b| {
                let ma = a.metadata().and_then(|m| m.modified()).ok();
                let mb = b.metadata().and_then(|m| m.modified()).ok();
                mb.cmp(&ma)
            });
            for d in hash_dirs {
                push_dir(d);
            }
        }

        push_dir(
            Path::new(&local_app_data)
                .join("Programs")
                .join("OpenAI")
                .join("Codex")
                .join("bin"),
        );
    }

    if let Ok(userprofile) = env::var("USERPROFILE") {
        let releases_dir = Path::new(&userprofile)
            .join(".codex")
            .join("packages")
            .join("app-server-daemon")
            .join("releases");
        if let Ok(entries) = fs::read_dir(&releases_dir) {
            for entry in entries.flatten() {
                push_dir(entry.path().join("bin"));
            }
        }
        for ext_root in IDE_EXTENSION_ROOTS {
            let ext_dir = Path::new(&userprofile).join(ext_root);
            if let Ok(entries) = fs::read_dir(&ext_dir) {
                for entry in entries.flatten() {
                    if entry
                        .file_name()
                        .to_str()
                        .is_some_and(|n| n.to_ascii_lowercase().starts_with("openai.chatgpt-"))
                    {
                        push_dir(entry.path().join("bin").join("windows-x86_64"));
                        push_dir(entry.path().join("bin").join("windows-aarch64"));
                        push_dir(entry.path().join("bin").join("windows-arm64"));
                    }
                }
            }
        }
    }

    dirs
}

/// IDE extension roots under `%USERPROFILE%` that may host `openai.chatgpt-*` extensions.
pub const IDE_EXTENSION_ROOTS: [&str; 6] = [
    ".vscode\\extensions",
    ".vscode-insiders\\extensions",
    ".cursor\\extensions",
    ".windsurf\\extensions",
    ".antigravity\\extensions",
    ".antigravity-ide\\extensions",
];

/// Copy `src` to `dst`, renaming `dst` to `<filename>.old.<pid>` first if `dst` is locked in use.
pub fn safe_copy_or_rename_locked(src: &Path, dst: &Path) -> io::Result<()> {
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent)?;
    }
    if fs::copy(src, dst).is_ok() {
        return Ok(());
    }
    if dst.exists() {
        let file_name = dst
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("binary.exe");
        let pid = std::process::id();
        let old_path = dst.with_file_name(format!("{}.old.{}", file_name, pid));
        let _ = fs::remove_file(&old_path);
        if fs::rename(dst, &old_path).is_err() {
            let alt_old = dst.with_file_name(format!(
                "{}.old.{}.{}",
                file_name,
                pid,
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.subsec_nanos())
                    .unwrap_or(1)
            ));
            let _ = fs::rename(dst, &alt_old);
        }
    }
    fs::copy(src, dst).map(|_| ())
}

/// Synchronize `custom\codex-9router-subagents.orig.exe`, `custom\codex.orig.exe`, and companion helper binaries
/// (`rg.exe`, `codex-command-runner.exe`, `codex-code-mode-host.exe`, `codex-windows-sandbox-setup.exe`,
/// `codex-windows-sandbox-service.exe`) inside `custom_dir` from `candidate_dirs` whenever a newer stock `codex.exe`
/// (`> 10 MB`) or helper is installed. Returns the number of files refreshed.
pub fn sync_custom_codex_binaries_from_candidates(
    custom_dir: &Path,
    candidate_dirs: &[PathBuf],
) -> usize {
    sync_custom_codex_binaries_from_candidates_with_min_size(custom_dir, candidate_dirs, 10_000_000)
}

/// Synchronize `custom\codex-9router-subagents.orig.exe`, `custom\codex.orig.exe`, and companion helper binaries
/// inside `custom_dir` from `candidate_dirs` using `min_stock_bytes` as the minimum stock `codex.exe` size threshold.
pub fn sync_custom_codex_binaries_from_candidates_with_min_size(
    custom_dir: &Path,
    candidate_dirs: &[PathBuf],
    min_stock_bytes: u64,
) -> usize {
    if !custom_dir.is_dir() {
        return 0;
    }

    // Best-effort cleanup of unlocked .old.* files in custom_dir
    if let Ok(entries) = fs::read_dir(custom_dir) {
        for entry in entries.flatten() {
            if entry
                .file_name()
                .to_str()
                .is_some_and(|n| n.contains(".old."))
            {
                let _ = fs::remove_file(entry.path());
            }
        }
    }

    let cur_canon = env::current_exe().ok().and_then(|p| p.canonicalize().ok());
    let custom_canon = custom_dir.canonicalize().ok();

    // 1. Find newest stock codex binary (> min_stock_bytes) across candidate_dirs
    let mut best_stock: Option<(PathBuf, u64, Option<std::time::SystemTime>)> = None;
    for dir in candidate_dirs {
        if let (Some(ref cc), Ok(dc)) = (&custom_canon, dir.canonicalize()) {
            if cc == &dc {
                continue;
            }
        }
        for name in ["codex.orig.exe", "codex.exe"] {
            let candidate = dir.join(name);
            if !candidate.is_file() {
                continue;
            }
            if let (Some(ref cur), Ok(cand_c)) = (&cur_canon, candidate.canonicalize()) {
                if cur == &cand_c {
                    continue;
                }
            }
            let Ok(meta) = candidate.metadata() else {
                continue;
            };
            let len = meta.len();
            if len <= min_stock_bytes {
                continue;
            }
            let modified = meta.modified().ok();
            let is_newer = match &best_stock {
                None => true,
                Some((_, _, best_mod)) => modified > *best_mod,
            };
            if is_newer {
                best_stock = Some((candidate, len, modified));
            }
        }
    }

    let mut refreshed_count = 0;

    if let Some((ref src_codex, src_len, src_mod)) = best_stock {
        for orig_name in ["codex-9router-subagents.orig.exe", "codex.orig.exe"] {
            let dst = custom_dir.join(orig_name);
            let needs_refresh = match dst.metadata() {
                Err(_) => true,
                Ok(dst_meta) => {
                    let dst_len = dst_meta.len();
                    let dst_mod = dst_meta.modified().ok();
                    dst_len <= min_stock_bytes
                        || dst_len != src_len
                        || (src_mod.is_some() && dst_mod < src_mod)
                }
            };
            if needs_refresh && safe_copy_or_rename_locked(src_codex, &dst).is_ok() {
                refreshed_count += 1;
            }
        }
    }

    // 2. Refresh companion helper binaries from candidate_dirs
    const HELPERS: [&str; 5] = [
        "rg.exe",
        "codex-command-runner.exe",
        "codex-code-mode-host.exe",
        "codex-windows-sandbox-setup.exe",
        "codex-windows-sandbox-service.exe",
    ];

    for helper in HELPERS {
        let mut best_helper: Option<(PathBuf, u64, Option<std::time::SystemTime>)> = None;
        for dir in candidate_dirs {
            if let (Some(ref cc), Ok(dc)) = (&custom_canon, dir.canonicalize()) {
                if cc == &dc {
                    continue;
                }
            }
            let h_path = dir.join(helper);
            let Ok(meta) = h_path.metadata() else {
                continue;
            };
            if !meta.is_file() || meta.len() == 0 {
                continue;
            }
            let modified = meta.modified().ok();
            let is_newer = match &best_helper {
                None => true,
                Some((_, _, best_mod)) => modified > *best_mod,
            };
            if is_newer {
                best_helper = Some((h_path, meta.len(), modified));
            }
        }

        if let Some((ref src_h, src_len, src_mod)) = best_helper {
            let dst_h = custom_dir.join(helper);
            let needs_refresh = match dst_h.metadata() {
                Err(_) => true,
                Ok(dst_meta) => {
                    let dst_len = dst_meta.len();
                    let dst_mod = dst_meta.modified().ok();
                    dst_len == 0
                        || dst_len != src_len
                        || (src_mod.is_some() && dst_mod < src_mod)
                }
            };
            if needs_refresh && safe_copy_or_rename_locked(src_h, &dst_h).is_ok() {
                refreshed_count += 1;
            }
        }
    }

    refreshed_count
}

/// Auto-hook standalone CLI (`%LOCALAPPDATA%\Programs\OpenAI\Codex\bin\codex.exe`) and IDE extensions
/// (`openai.chatgpt-*\bin\windows-*\codex.exe`) by backing up stock `> min_stock_bytes` binaries to
/// `codex.orig.exe`, replacing `codex.exe` with `proxy_shim`, and cleaning up unlocked `.old.*` files.
pub fn sync_hooked_surface_binaries_with_min_size(
    proxy_shim: &Path,
    hook_dirs: &[PathBuf],
    min_stock_bytes: u64,
) -> usize {
    let Ok(shim_meta) = proxy_shim.metadata() else {
        return 0;
    };
    if !shim_meta.is_file() || shim_meta.len() == 0 || shim_meta.len() > min_stock_bytes {
        return 0;
    }
    let shim_len = shim_meta.len();
    let mut hooked = 0;

    for hdir in hook_dirs {
        if let Ok(entries) = fs::read_dir(hdir) {
            for entry in entries.flatten() {
                if entry
                    .file_name()
                    .to_str()
                    .is_some_and(|n| n.contains(".old."))
                {
                    let _ = fs::remove_file(entry.path());
                }
            }
        }

        let codex_exe = hdir.join("codex.exe");
        let codex_orig = hdir.join("codex.orig.exe");
        let Ok(exe_meta) = codex_exe.metadata() else {
            continue;
        };
        if !exe_meta.is_file() {
            continue;
        }
        if exe_meta.len() > min_stock_bytes {
            let needs_backup = match codex_orig.metadata() {
                Err(_) => true,
                Ok(orig_meta) => {
                    orig_meta.len() != exe_meta.len()
                        || orig_meta.modified().ok() != exe_meta.modified().ok()
                }
            };
            if (!needs_backup || safe_copy_or_rename_locked(&codex_exe, &codex_orig).is_ok())
                && safe_copy_or_rename_locked(proxy_shim, &codex_exe).is_ok()
            {
                hooked += 1;
            }
        } else if codex_orig
            .metadata()
            .is_ok_and(|m| m.is_file() && m.len() > min_stock_bytes)
            && (exe_meta.len() != shim_len
                || exe_meta.modified().ok() != shim_meta.modified().ok())
            && safe_copy_or_rename_locked(proxy_shim, &codex_exe).is_ok()
        {
            hooked += 1;
        }
    }

    hooked
}

/// Perform in-process self-healing of `%LOCALAPPDATA%\OpenAI\Codex\custom` binaries and helpers,
/// and hook/re-hook standalone CLI (`%LOCALAPPDATA%\Programs\OpenAI\Codex\bin\codex.exe`) or IDE extensions
/// (`openai.chatgpt-*`) if an external update installed or overwrote `codex.exe` with a `> 10 MB` stock binary.
pub fn sync_custom_codex_binaries() -> usize {
    let Ok(local_app_data) = env::var("LOCALAPPDATA") else {
        return 0;
    };
    let custom_dir = Path::new(&local_app_data)
        .join("OpenAI")
        .join("Codex")
        .join("custom");
    if !custom_dir.is_dir() {
        return 0;
    }
    let candidate_dirs = discover_codex_binary_candidate_dirs();
    let mut count = sync_custom_codex_binaries_from_candidates(&custom_dir, &candidate_dirs);

    let proxy_shim = custom_dir.join("codex-9router-subagents.exe");
    let mut hook_dirs = vec![Path::new(&local_app_data)
        .join("Programs")
        .join("OpenAI")
        .join("Codex")
        .join("bin")];
    if let Ok(userprofile) = env::var("USERPROFILE") {
        for ext_root in IDE_EXTENSION_ROOTS {
            let ext_dir = Path::new(&userprofile).join(ext_root);
            if let Ok(entries) = fs::read_dir(&ext_dir) {
                for entry in entries.flatten() {
                    if entry
                        .file_name()
                        .to_str()
                        .is_some_and(|n| n.to_ascii_lowercase().starts_with("openai.chatgpt-"))
                    {
                        hook_dirs.push(entry.path().join("bin").join("windows-x86_64"));
                        hook_dirs.push(entry.path().join("bin").join("windows-aarch64"));
                        hook_dirs.push(entry.path().join("bin").join("windows-arm64"));
                    }
                }
            }
        }
    }
    count += sync_hooked_surface_binaries_with_min_size(&proxy_shim, &hook_dirs, 10_000_000);

    count
}

/// Select the best official `.orig.exe` candidate inside `dir`, preferring files > 10 MB
/// (stock Codex binary) and sorting by most recent modification time.
pub fn pick_best_orig_candidate(dir: &Path) -> Option<PathBuf> {
    let cur_canon = env::current_exe().ok().and_then(|p| p.canonicalize().ok());
    let mut valid_candidates: Vec<(PathBuf, u64, Option<std::time::SystemTime>)> = Vec::new();

    for candidate_name in ["codex-9router-subagents.orig.exe", "codex.orig.exe"] {
        let orig = dir.join(candidate_name);
        if !orig.is_file() {
            continue;
        }
        if let (Some(ref c1), Ok(c2)) = (&cur_canon, orig.canonicalize()) {
            if c1 == &c2 {
                continue;
            }
        }
        let meta = orig.metadata().ok();
        let len = meta.as_ref().map_or(0, |m| m.len());
        if len == 0 {
            continue;
        }
        let modified = meta.and_then(|m| m.modified().ok());
        valid_candidates.push((orig, len, modified));
    }

    if valid_candidates.is_empty() {
        return None;
    }

    let has_full_stock = valid_candidates.iter().any(|(_, len, _)| *len > 10_000_000);
    if has_full_stock {
        valid_candidates.retain(|(_, len, _)| *len > 10_000_000);
    }

    valid_candidates.sort_by_key(|a| std::cmp::Reverse(a.2));
    valid_candidates.into_iter().next().map(|(p, _, _)| p)
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

    // 2. Check if codex-9router-subagents.orig.exe or codex.orig.exe exists in the directory of the current executable
    if let Ok(current_exe) = env::current_exe() {
        if let Some(parent) = current_exe.parent() {
            if let Some(orig) = pick_best_orig_candidate(parent) {
                return orig;
            }
        }
    }

    // 3. Check custom directory and discovered Store / bin/<hash> / Programs / extension candidates
    if let Ok(local_app_data) = env::var("LOCALAPPDATA") {
        let custom_dir = Path::new(&local_app_data)
            .join("OpenAI")
            .join("Codex")
            .join("custom");
        if let Some(orig) = pick_best_orig_candidate(&custom_dir) {
            return orig;
        }
    }

    let cur_canon = env::current_exe().ok().and_then(|p| p.canonicalize().ok());
    let mut stock_candidates: Vec<(PathBuf, Option<std::time::SystemTime>)> = Vec::new();
    for dir in discover_codex_binary_candidate_dirs() {
        for name in ["codex.orig.exe", "codex.exe"] {
            let p = dir.join(name);
            if !p.is_file() {
                continue;
            }
            if let (Some(ref c1), Ok(c2)) = (&cur_canon, p.canonicalize()) {
                if c1 == &c2 {
                    continue;
                }
            }
            if let Ok(meta) = p.metadata() {
                if meta.len() > 10_000_000 {
                    stock_candidates.push((p, meta.modified().ok()));
                }
            }
        }
    }
    stock_candidates.sort_by_key(|a| std::cmp::Reverse(a.1));
    if let Some((first, _)) = stock_candidates.into_iter().next() {
        return first;
    }

    PathBuf::from("codex.orig.exe")
}

/// Reset any rate limit errors or spend control blocks in local UI responses,
/// and rewrite loopback `backendOrigin` back to `https://chatgpt.com` so the Electron GUI
/// (`AuthService` / `electron.net.fetch`) connects directly to ChatGPT with valid public TLS.
pub fn sanitize_rate_limits(val: &mut Value) -> bool {
    let mut modified = false;
    if let Some(obj) = val.as_object_mut() {
        for key in ["backendOrigin", "backend_origin"] {
            if let Some(Value::String(origin)) = obj.get_mut(key) {
                if origin.starts_with("https://127.0.0.1")
                    || origin.starts_with("http://127.0.0.1")
                    || origin.starts_with("https://localhost")
                    || origin.starts_with("http://localhost")
                {
                    let upstream = get_chatgpt_upstream_base();
                    let clean_origin = upstream
                        .trim_end_matches('/')
                        .trim_end_matches("/backend-api")
                        .to_string();
                    *origin = clean_origin;
                    modified = true;
                }
            }
        }
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
/// Never inspects chat message `"role"` fields or `"input"` / `"messages"` / `"instructions"` / `"tools"` arrays.
pub fn find_agent_role(v: &Value) -> Option<String> {
    match v {
        Value::Object(map) => {
            let mut fallback_role: Option<String> = None;
            for key in [
                "agentRole",
                "agent_role",
                "agentType",
                "agent_type",
                "subagent_role",
                "subagentRole",
                "agentName",
                "agent_name",
            ] {
                if let Some(r) = map.get(key).and_then(|x| x.as_str()) {
                    if let Some(norm) = normalize_subagent_role_token(r) {
                        if norm != "default" {
                            return Some(norm);
                        }
                        fallback_role.get_or_insert(norm);
                    }
                }
            }
            for meta_key in ["x-codex-turn-metadata", "turn_metadata", "turnMetadata"] {
                if let Some(meta_val) = map.get(meta_key) {
                    let meta_role = match meta_val {
                        Value::String(s) => extract_role_from_turn_metadata_str(s),
                        other => extract_role_from_turn_metadata_value(other),
                    };
                    if let Some(norm) = meta_role {
                        if norm != "default" {
                            return Some(norm);
                        }
                        fallback_role.get_or_insert(norm);
                    }
                }
            }
            if let Some(r) = map.get("x-openai-subagent").and_then(|x| x.as_str()) {
                let trimmed = r.trim();
                if let Some(norm) = normalize_subagent_role_token(trimmed) {
                    if norm != "default" {
                        return Some(norm);
                    }
                    fallback_role.get_or_insert(norm);
                } else if !trimmed.is_empty() {
                    fallback_role.get_or_insert_with(|| "default".to_string());
                }
            }
            // In JSON-RPC thread/start or turn/start params (where "input"/"messages" is not the parent),
            // allow top-level "role" only if it is explicitly one of the known subagent roles.
            if let Some(r) = map.get("role").and_then(|x| x.as_str()) {
                let lower = r.trim().to_lowercase();
                if matches!(lower.as_str(), "worker" | "explorer" | "reviewer") {
                    return Some(lower);
                }
                if matches!(lower.as_str(), "default" | "subagent") {
                    fallback_role.get_or_insert(lower);
                }
            }
            for (k, child) in map.iter() {
                if matches!(
                    k.as_str(),
                    "input"
                        | "messages"
                        | "instructions"
                        | "tools"
                        | "functions"
                        | "input_schema"
                        | "parameters"
                ) {
                    continue;
                }
                if let Some(r) = find_agent_role(child) {
                    if r != "default" && r != "subagent" {
                        return Some(r);
                    }
                    fallback_role.get_or_insert(r);
                }
            }
            fallback_role
        }
        Value::Array(arr) => {
            let mut fallback_role: Option<String> = None;
            for child in arr {
                if let Some(r) = find_agent_role(child) {
                    if r != "default" && r != "subagent" {
                        return Some(r);
                    }
                    fallback_role.get_or_insert(r);
                }
            }
            fallback_role
        }
        _ => None,
    }
}

/// Inspect and rewrite thread/turn JSON-RPC parameters to route subagents to 9Router.
pub fn route_thread_params(params: &mut serde_json::Map<String, Value>) -> bool {
    let mut modified = false;
    let mut nested_subagent = false;

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
        if !params.contains_key("model") || params.get("model").is_none_or(|v| v.is_null()) {
            params.insert("model".to_string(), Value::String(m));
            modified = true;
        }
    }

    // Check role / agent type across current map and nested trees
    let detected_role = find_agent_role(&Value::Object(params.clone()));
    let model_str = params
        .get("model")
        .and_then(|m| m.as_str())
        .map(|s| s.to_string());
    let inferred_role = match detected_role.as_deref() {
        None | Some("default" | "subagent") => model_str
            .as_deref()
            .and_then(|m| find_role_for_model_in_dir(get_codex_home_dir().as_deref(), m))
            .or(detected_role.clone()),
        _ => detected_role.clone(),
    };
    let role = inferred_role.as_deref();
    let target_provider = get_target_model_provider_for_role(role);

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
        .is_some_and(|s| !s.is_empty());

    // Check threadSource
    let source_is_subagent = params
        .get("threadSource")
        .or_else(|| params.get("thread_source"))
        .is_some_and(contains_subagent_source);

    let is_subagent =
        role_is_subagent || has_agent_nickname || source_is_subagent || nested_subagent;

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
            Some(Value::String(s)) => s.is_empty() || (is_subagent && is_parent_chatgpt_model(s)),
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

/// Check if a given model string corresponds to a 9Router or subagent model within a specific `<codex_home>` directory.
pub fn is_subagent_model_name_in_dir(m: &str, codex_home: Option<&Path>) -> bool {
    let trimmed = m.trim();
    if trimmed.is_empty() || is_parent_chatgpt_model(trimmed) {
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

    // Match if user configured a custom model for any standard or custom subagent role
    for configured in collect_configured_subagent_models_in_dir(codex_home) {
        if !configured.is_empty()
            && !is_parent_chatgpt_model(&configured)
            && lower == configured.to_lowercase()
        {
            return true;
        }
    }

    false
}

/// Check if a given model string corresponds to a 9Router or subagent model.
pub fn is_subagent_model_name(m: &str) -> bool {
    let codex_home = get_codex_home_dir();
    is_subagent_model_name_in_dir(m, codex_home.as_deref())
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

/// Check if a request path is the models catalog endpoint (`GET /backend-api/codex/models` or `GET /backend-api/models`).
pub fn is_models_path(path: &str) -> bool {
    let clean_path = path.split('?').next().unwrap_or(path).trim_end_matches('/');
    clean_path == "/backend-api/codex/models"
        || clean_path == "/backend-api/models"
        || clean_path == "/codex/models"
        || clean_path == "/models"
        || clean_path.ends_with("/codex/models")
        || clean_path.ends_with("/backend-api/models")
        || clean_path.ends_with("/models")
}

/// Retrieve the configured loopback proxy port (default: 20129).
pub fn get_proxy_port() -> String {
    if let Ok(p) = env::var("CODEX_PROXY_PORT") {
        let trimmed = p.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    if let Some(p) = get_user_env_var("CODEX_PROXY_PORT") {
        let trimmed = p.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    "20129".to_string()
}

/// Retrieve the official ChatGPT upstream base URL (default: <https://chatgpt.com>).
pub fn get_chatgpt_upstream_base() -> String {
    env::var("CODEX_CHATGPT_UPSTREAM_URL").unwrap_or_else(|_| "https://chatgpt.com".to_string())
}

/// Normalize a raw base URL or endpoint into a `/v1/responses` (or `/responses`) endpoint URL.
pub fn normalize_subagent_responses_endpoint(ep: &str) -> Option<String> {
    let trimmed = ep.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return None;
    }
    if trimmed.ends_with("/responses") {
        return Some(trimmed.to_string());
    }
    if trimmed.ends_with("/v1") {
        return Some(format!("{}/responses", trimmed));
    }
    Some(format!("{}/v1/responses", trimmed))
}

/// Read subagent provider `base_url` from `<codex_home>/config.toml`.
pub fn read_subagent_endpoint_from_config_in_dir(
    codex_home: &Path,
    provider: &str,
) -> Option<String> {
    let config_path = codex_home.join("config.toml");
    let content = fs::read_to_string(&config_path).ok()?;
    let base_url = parse_provider_base_url_from_config_toml(&content, provider)?;
    normalize_subagent_responses_endpoint(&base_url)
}

/// Read subagent provider `base_url` from `~/.codex/config.toml`.
pub fn read_subagent_endpoint_from_config(provider: &str) -> Option<String> {
    let codex_home = get_codex_home_dir()?;
    read_subagent_endpoint_from_config_in_dir(&codex_home, provider)
}

/// Retrieve the subagent responses target endpoint for an optional role within `<codex_home>`.
pub fn get_subagent_responses_url_for_role_in_dir(
    role: Option<&str>,
    codex_home: Option<&Path>,
) -> String {
    let proc_val = env::var("CODEX_SUBAGENT_ENDPOINT")
        .ok()
        .and_then(|s| normalize_subagent_responses_endpoint(&s));
    let mut reg_val: Option<String> = None;

    if let Some(ref ep) = proc_val {
        reg_val = get_user_env_var("CODEX_SUBAGENT_ENDPOINT")
            .and_then(|s| normalize_subagent_responses_endpoint(&s));
        if reg_val.as_deref() != Some(ep.as_str()) {
            return ep.clone();
        }
    }

    let provider = get_target_model_provider_for_role_in_dir(codex_home, role);
    if let Some(home) = codex_home {
        if let Some(cfg_ep) = read_subagent_endpoint_from_config_in_dir(home, &provider) {
            return cfg_ep;
        }
    }

    if let Some(ep) = proc_val.or_else(|| {
        reg_val.or_else(|| {
            get_user_env_var("CODEX_SUBAGENT_ENDPOINT")
                .and_then(|s| normalize_subagent_responses_endpoint(&s))
        })
    }) {
        return ep;
    }

    "http://127.0.0.1:20128/v1/responses".to_string()
}

/// Retrieve the subagent responses target endpoint for an optional role.
pub fn get_subagent_responses_url_for_role(role: Option<&str>) -> String {
    let codex_home = get_codex_home_dir();
    get_subagent_responses_url_for_role_in_dir(role, codex_home.as_deref())
}

/// Retrieve the 9Router / subagent responses target endpoint.
pub fn get_subagent_responses_url() -> String {
    get_subagent_responses_url_for_role(None)
}

/// Format an API key or bearer token into a single `Bearer <token>` header value without duplicating `Bearer `.
pub fn format_bearer_header_value(raw_key: &str) -> Option<String> {
    let mut token = raw_key.trim();
    while let Some(rest) = token
        .strip_prefix("Bearer ")
        .or_else(|| token.strip_prefix("bearer "))
        .or_else(|| token.strip_prefix("BEARER "))
    {
        token = rest.trim();
    }
    if token.is_empty() {
        None
    } else {
        Some(format!("Bearer {}", token))
    }
}

/// Retrieve authorization header for a subagent role within `<codex_home>`, prioritizing the active
/// provider's configured `env_key` in `config.toml` before falling back to `NINEROUTER_KEY`.
pub fn get_subagent_auth_header_for_role_in_dir(
    role: Option<&str>,
    codex_home: Option<&Path>,
) -> Option<String> {
    if let Some(home) = codex_home {
        let provider = get_target_model_provider_for_role_in_dir(Some(home), role);
        if let Ok(content) = fs::read_to_string(home.join("config.toml")) {
            if let Some(env_key_name) = parse_provider_env_key_from_config_toml(&content, &provider)
            {
                let trimmed_name = env_key_name.trim();
                if !trimmed_name.is_empty() {
                    if let Ok(k) = env::var(trimmed_name) {
                        if let Some(formatted) = format_bearer_header_value(&k) {
                            return Some(formatted);
                        }
                    }
                    if let Some(k) = get_user_env_var(trimmed_name) {
                        if let Some(formatted) = format_bearer_header_value(&k) {
                            return Some(formatted);
                        }
                    }
                }
            }
        }
    }
    if let Ok(key) = env::var("NINEROUTER_KEY") {
        if let Some(formatted) = format_bearer_header_value(&key) {
            return Some(formatted);
        }
    }
    if let Some(key) = get_user_env_var("NINEROUTER_KEY") {
        if let Some(formatted) = format_bearer_header_value(&key) {
            return Some(formatted);
        }
    }
    None
}

/// Retrieve authorization header for a subagent role.
pub fn get_subagent_auth_header_for_role(role: Option<&str>) -> Option<String> {
    let codex_home = get_codex_home_dir();
    get_subagent_auth_header_for_role_in_dir(role, codex_home.as_deref())
}

/// Retrieve authorization header for 9Router subagents.
pub fn get_subagent_auth_header() -> Option<String> {
    get_subagent_auth_header_for_role(None)
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

fn extract_compaction_item_text(item_obj: &serde_json::Map<String, Value>) -> String {
    if let Some(enc) = item_obj.get("encrypted_content").and_then(|v| v.as_str()) {
        let trimmed = enc.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    if let Some(sum_val) = item_obj.get("summary") {
        match sum_val {
            Value::String(s) => {
                let trimmed = s.trim();
                if !trimmed.is_empty() {
                    return trimmed.to_string();
                }
            }
            Value::Array(arr) => {
                let mut parts = Vec::new();
                for part in arr {
                    if let Some(t) = part
                        .get("text")
                        .or_else(|| part.get("summary_text"))
                        .and_then(|v| v.as_str())
                        .or_else(|| part.as_str())
                    {
                        let trimmed = t.trim();
                        if !trimmed.is_empty() {
                            parts.push(trimmed.to_string());
                        }
                    }
                }
                if !parts.is_empty() {
                    return parts.join("\n");
                }
            }
            _ => {}
        }
    }
    if let Some(content_val) = item_obj.get("content") {
        if let Some(text) = extract_text_from_content_value(content_val) {
            return text;
        }
    }
    if let Some(text) = item_obj.get("text").and_then(|v| v.as_str()) {
        let trimmed = text.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    String::new()
}

/// Sanitize incompatible OpenAI tool schemas (`"type": "namespace"`, `"type": "web_search"`,
/// `"type": "custom"` for `apply_patch`, and `"type": "additional_tools"` / `"compaction_trigger"` /
/// `"configuration_update"` in `"input"`), rehydrate `"type": "compaction"` / `"context_compaction"`
/// items into standard `"type": "message"` (`"role": "user"`) items, convert forked
/// `"custom_tool_call"` / `"custom_tool_call_output"` / `"tool_search_call"` / `"tool_search_output"`
/// items into standard `"type": "message"` items, and normalize Multi-Agents V2 `"type": "agent_message"`
/// items and `"role": "developer"` messages before forwarding a subagent request to 9Router.
pub fn sanitize_subagent_request_for_9router(json: &mut Value) {
    let Some(obj) = json.as_object_mut() else {
        return;
    };

    // 1. In "input":
    //    - Strip internal marker items ("additional_tools", "compaction_trigger", "configuration_update").
    //    - Rehydrate any "type": "compaction" / "compaction_summary" / "context_compaction" items into
    //      standard "type": "message" ("role": "user") items with "[Compacted Conversation Summary]\n<text>".
    //    - Convert forked "custom_tool_call" / "custom_tool_call_output" / "tool_search_call" /
    //      "tool_search_output" items into standard "type": "message" items so 9Router never fails on
    //      parent gpt-6-luna freeform/code_mode history items.
    //    - Normalize Multi-Agents V2 "type": "agent_message" items into standard
    //      "type": "message" with "role": "user" (stripping V2-only metadata fields
    //      such as "author", "recipient", and "internal_chat_message_metadata_passthrough").
    //    - Normalize "role": "developer" items into "role": "system" so 9Router and
    //      downstream translators (Gemini / Claude / OpenAI) preserve developer prompts.
    if let Some(input_arr) = obj.get_mut("input").and_then(|v| v.as_array_mut()) {
        input_arr.retain(|item| {
            let Some(t) = item.get("type").and_then(|t| t.as_str()) else {
                return true;
            };
            !t.eq_ignore_ascii_case("additional_tools")
                && !t.eq_ignore_ascii_case("compaction_trigger")
                && !t.eq_ignore_ascii_case("configuration_update")
        });
        for item in input_arr.iter_mut() {
            let Some(item_obj) = item.as_object_mut() else {
                continue;
            };
            let item_type = item_obj
                .get("type")
                .and_then(|t| t.as_str())
                .unwrap_or("")
                .to_ascii_lowercase();

            if matches!(
                item_type.as_str(),
                "compaction" | "compaction_summary" | "context_compaction"
            ) {
                let raw_summary = extract_compaction_item_text(item_obj);
                let summary_text = if raw_summary.is_empty() {
                    "[Compacted Conversation Summary]".to_string()
                } else if raw_summary.starts_with("[Compacted Conversation Summary]") {
                    raw_summary
                } else {
                    format!("[Compacted Conversation Summary]\n{}", raw_summary)
                };
                item_obj.clear();
                item_obj.insert("type".to_string(), Value::String("message".to_string()));
                item_obj.insert("role".to_string(), Value::String("user".to_string()));
                item_obj.insert(
                    "content".to_string(),
                    serde_json::json!([
                        {
                            "type": "input_text",
                            "text": summary_text
                        }
                    ]),
                );
                continue;
            }

            if item_type == "custom_tool_call" {
                let name = item_obj
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("custom_tool")
                    .to_string();
                let call_input = item_obj
                    .get("input")
                    .or_else(|| item_obj.get("arguments"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let text = if call_input.trim().is_empty() {
                    format!("[Tool Call: {}]", name)
                } else {
                    format!("[Tool Call: {}]\n{}", name, call_input)
                };
                item_obj.clear();
                item_obj.insert("type".to_string(), Value::String("message".to_string()));
                item_obj.insert("role".to_string(), Value::String("assistant".to_string()));
                item_obj.insert(
                    "content".to_string(),
                    serde_json::json!([
                        {
                            "type": "output_text",
                            "text": text
                        }
                    ]),
                );
                continue;
            }

            if item_type == "custom_tool_call_output" || item_type == "tool_search_output" {
                let out_text = item_obj
                    .get("output")
                    .and_then(extract_text_from_content_value)
                    .unwrap_or_default();
                let text = if out_text.trim().is_empty() {
                    "[Tool Output]".to_string()
                } else {
                    format!("[Tool Output]\n{}", out_text)
                };
                item_obj.clear();
                item_obj.insert("type".to_string(), Value::String("message".to_string()));
                item_obj.insert("role".to_string(), Value::String("user".to_string()));
                item_obj.insert(
                    "content".to_string(),
                    serde_json::json!([
                        {
                            "type": "input_text",
                            "text": text
                        }
                    ]),
                );
                continue;
            }

            if item_type == "tool_search_call" {
                let query_text = item_obj
                    .get("arguments")
                    .or_else(|| item_obj.get("query"))
                    .and_then(extract_text_from_content_value)
                    .unwrap_or_default();
                let text = if query_text.trim().is_empty() {
                    "[Tool Search]".to_string()
                } else {
                    format!("[Tool Search]\n{}", query_text)
                };
                item_obj.clear();
                item_obj.insert("type".to_string(), Value::String("message".to_string()));
                item_obj.insert("role".to_string(), Value::String("assistant".to_string()));
                item_obj.insert(
                    "content".to_string(),
                    serde_json::json!([
                        {
                            "type": "output_text",
                            "text": text
                        }
                    ]),
                );
                continue;
            }

            if item_type == "image_generation_call" {
                let prompt_text = item_obj
                    .get("revised_prompt")
                    .or_else(|| item_obj.get("prompt"))
                    .and_then(extract_text_from_content_value)
                    .unwrap_or_default();
                let text = if prompt_text.trim().is_empty() {
                    "[Image Generation Call]".to_string()
                } else {
                    format!("[Image Generation Call]\n{}", prompt_text)
                };
                item_obj.clear();
                item_obj.insert("type".to_string(), Value::String("message".to_string()));
                item_obj.insert("role".to_string(), Value::String("assistant".to_string()));
                item_obj.insert(
                    "content".to_string(),
                    serde_json::json!([
                        {
                            "type": "output_text",
                            "text": text
                        }
                    ]),
                );
                continue;
            }

            if item_type == "agent_message" {
                item_obj.insert("type".to_string(), Value::String("message".to_string()));
                let normalized_role = match item_obj
                    .get("role")
                    .and_then(|r| r.as_str())
                    .map(|r| r.trim())
                {
                    None | Some("") => "user".to_string(),
                    Some(r) if r.eq_ignore_ascii_case("agent") => "user".to_string(),
                    Some(r) if r.eq_ignore_ascii_case("developer") => "system".to_string(),
                    Some(r) => r.to_lowercase(),
                };
                item_obj.insert("role".to_string(), Value::String(normalized_role));
                item_obj.remove("author");
                item_obj.remove("recipient");
            } else if item_obj
                .get("role")
                .and_then(|r| r.as_str())
                .is_some_and(|r| r.eq_ignore_ascii_case("developer"))
            {
                item_obj.insert("role".to_string(), Value::String("system".to_string()));
            }
            item_obj.remove("internal_chat_message_metadata_passthrough");
        }
    }

    if let Some(messages_arr) = obj.get_mut("messages").and_then(|v| v.as_array_mut()) {
        for msg in messages_arr.iter_mut() {
            if let Some(msg_obj) = msg.as_object_mut() {
                if msg_obj
                    .get("role")
                    .and_then(|r| r.as_str())
                    .is_some_and(|r| r.eq_ignore_ascii_case("developer"))
                {
                    msg_obj.insert("role".to_string(), Value::String("system".to_string()));
                }
            }
        }
    }

    // 2. Sanitize top-level "tools" array:
    //    - Filter out "type": "namespace", "type": "web_search", "type": "web_search_preview",
    //      "type": "tool_search" (or "name": "tool_search"), and "type": "custom" (Freeform tools
    //      require custom_tool_call SSE responses which 9Router does not emit; when codex.orig.exe
    //      uses our injected model metadata with apply_patch_tool_type = "function", it sends
    //      "type": "function", "name": "apply_patch" which is preserved here and matches ToolPayload::Function).
    let mut tools_became_empty = false;
    if let Some(tools_arr) = obj.get_mut("tools").and_then(|v| v.as_array_mut()) {
        tools_arr.retain(|tool| {
            let tool_type = tool
                .get("type")
                .and_then(|t| t.as_str())
                .unwrap_or("");
            let tool_name = tool
                .get("name")
                .or_else(|| tool.get("function").and_then(|f| f.get("name")))
                .and_then(|n| n.as_str())
                .unwrap_or("");
            !matches!(
                tool_type,
                "namespace" | "web_search" | "web_search_preview" | "custom" | "tool_search"
            ) && !tool_name.eq_ignore_ascii_case("tool_search")
        });
        if tools_arr.is_empty() {
            tools_became_empty = true;
        }
    }
    if tools_became_empty {
        obj.remove("tools");
        obj.remove("tool_choice");
    }

    // 3. Strip ChatGPT-internal top-level fields that 9Router / downstream providers do not accept.
    obj.remove("access_programs");
    obj.remove("codex_output_schema");
}

/// Inspect and determine routing policy for an HTTP request to the loopback proxy
/// using both HTTP headers and JSON body.
/// Returns `(is_subagent, modified_or_original_body)`.
pub fn inspect_and_route_http_request_with_headers(
    path: &str,
    headers: &HeaderMap,
    body: &[u8],
) -> (bool, Vec<u8>) {
    if !is_responses_path(path) || body.is_empty() {
        return (false, body.to_vec());
    }

    let Ok(mut json) = serde_json::from_slice::<Value>(body) else {
        return (false, body.to_vec());
    };

    let header_is_subagent = is_subagent_http_headers(headers);
    let current_model = json
        .get("model")
        .and_then(|m| m.as_str())
        .map(|s| s.to_string());
    let model_is_subagent = current_model
        .as_deref()
        .is_some_and(is_subagent_model_name);
    let has_subagent_marker = contains_subagent_source(&json);
    let detected_role = find_agent_role(&json);
    let role_is_subagent = detected_role
        .as_deref()
        .is_some_and(is_subagent_role_name);
    let has_agent_nickname = json
        .get("agentNickname")
        .or_else(|| json.get("agent_nickname"))
        .and_then(|v| v.as_str())
        .is_some_and(|s| !s.is_empty());

    let is_subagent = header_is_subagent
        || model_is_subagent
        || has_subagent_marker
        || role_is_subagent
        || has_agent_nickname;

    if !is_subagent {
        return (false, body.to_vec());
    }

    let needs_model_rewrite = match current_model.as_deref() {
        None => true,
        Some(m) => m.is_empty() || is_parent_chatgpt_model(m),
    };

    if needs_model_rewrite {
        let header_role = extract_role_from_http_headers(headers);
        let effective_role = match (detected_role.as_deref(), header_role.as_deref()) {
            (Some("default" | "subagent"), Some(hr)) => Some(hr),
            (Some(dr), _) => Some(dr),
            (None, hr) => hr,
        };
        let mapped_model = map_role_to_model(effective_role);
        if let Some(obj) = json.as_object_mut() {
            obj.insert("model".to_string(), Value::String(mapped_model));
        }
    }

    sanitize_subagent_request_for_9router(&mut json);

    let out_body = serde_json::to_vec(&json).unwrap_or_else(|_| body.to_vec());
    (true, out_body)
}

/// Convenience wrapper for inspecting an HTTP request body without custom headers.
pub fn inspect_and_route_http_request(path: &str, body: &[u8]) -> (bool, Vec<u8>) {
    inspect_and_route_http_request_with_headers(path, &HeaderMap::new(), body)
}

/// Resolve the subagent context window given an optional string override or the parent template
/// (defaulting to `872000`).
pub fn resolve_subagent_context_window_with_override(
    template: Option<&Value>,
    env_override: Option<&str>,
) -> u64 {
    if let Some(raw) = env_override {
        if let Ok(parsed) = raw.trim().parse::<u64>() {
            if parsed > 0 {
                return parsed;
            }
        }
    }
    let tmpl_max = template
        .and_then(|t| {
            t.get("max_context_window")
                .or_else(|| t.get("context_window"))
        })
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    tmpl_max.max(872_000)
}

/// Resolve the subagent context window from `CODEX_SUBAGENT_CONTEXT_WINDOW` or the parent template
/// (defaulting to `872000`).
pub fn resolve_subagent_context_window(template: Option<&Value>) -> u64 {
    let env_override = env::var("CODEX_SUBAGENT_CONTEXT_WINDOW")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| {
            get_user_env_var("CODEX_SUBAGENT_CONTEXT_WINDOW").filter(|s| !s.trim().is_empty())
        });
    resolve_subagent_context_window_with_override(template, env_override.as_deref())
}

/// Resolve the subagent `comp_hash` from the matched parent template (defaulting to `"3000"`).
pub fn resolve_subagent_comp_hash(template: Option<&Value>) -> String {
    template
        .and_then(|t| t.get("comp_hash"))
        .and_then(|v| v.as_str())
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .unwrap_or("3000")
        .to_string()
}

fn apply_subagent_model_metadata_fields(
    entry_obj: &mut serde_json::Map<String, Value>,
    slug: &str,
    template: Option<&Value>,
) {
    let context_window = resolve_subagent_context_window(template);
    let comp_hash = resolve_subagent_comp_hash(template);

    entry_obj.insert("slug".to_string(), Value::String(slug.to_string()));
    if entry_obj.contains_key("id") {
        entry_obj.insert("id".to_string(), Value::String(slug.to_string()));
    }
    entry_obj.insert(
        "display_name".to_string(),
        Value::String(format!("{} (9Router)", slug)),
    );
    entry_obj.insert(
        "description".to_string(),
        Value::String("Subagent model routed via Codex 9Router Proxy".to_string()),
    );
    entry_obj.insert("prefer_websockets".to_string(), Value::Bool(false));
    entry_obj.insert(
        "context_window".to_string(),
        serde_json::json!(context_window),
    );
    entry_obj.insert(
        "max_context_window".to_string(),
        serde_json::json!(context_window),
    );
    entry_obj.insert("auto_compact_token_limit".to_string(), Value::Null);
    entry_obj.insert(
        "effective_context_window_percent".to_string(),
        serde_json::json!(95),
    );
    entry_obj.insert("comp_hash".to_string(), Value::String(comp_hash));
    entry_obj.insert(
        "apply_patch_tool_type".to_string(),
        Value::String("function".to_string()),
    );
    entry_obj.insert("visibility".to_string(), Value::String("list".to_string()));
    entry_obj.insert("supported_in_api".to_string(), Value::Bool(true));
    entry_obj.insert("supports_search_tool".to_string(), Value::Bool(false));
    entry_obj.insert(
        "supports_experimental_context".to_string(),
        Value::Bool(false),
    );
    entry_obj.insert("use_responses_lite".to_string(), Value::Bool(true));
    entry_obj.remove("tool_mode");
    // Ensure code_mode is not forced on 9Router subagent models
    entry_obj.insert(
        "experimental_supported_tools".to_string(),
        serde_json::json!([]),
    );

    // codex.orig.exe (model-provider/src/models_endpoint.rs) rejects any ModelInfo entry missing
    // both `base_instructions` and `model_messages.instructions_template`.
    let has_base_instructions = entry_obj
        .get("base_instructions")
        .and_then(|v| v.as_str())
        .is_some_and(|s| !s.trim().is_empty());
    let has_instructions_template = entry_obj
        .get("model_messages")
        .and_then(|v| v.get("instructions_template"))
        .and_then(|v| v.as_str())
        .is_some_and(|s| !s.trim().is_empty());
    if !has_base_instructions && !has_instructions_template {
        let default_instructions =
            "You are Codex, an AI coding subagent. Complete your assigned task accurately and concisely.";
        entry_obj.insert(
            "base_instructions".to_string(),
            Value::String(default_instructions.to_string()),
        );
        entry_obj.insert(
            "model_messages".to_string(),
            serde_json::json!({
                "instructions_template": default_instructions
            }),
        );
    }
}

/// Inject subagent model metadata descriptors (`9router-subagent`, `implement`, `explore`, `review`,
/// and any configured role models) into the `/backend-api/models` or `models_cache.json` JSON payload
/// so `codex.orig.exe` never logs `Model metadata for ... not found`, aligns subagent context metadata
/// with the active parent model template (preserving its `comp_hash` or defaulting to `"3000"`, with
/// `context_window: 872000`, `max_context_window: 872000`, `effective_context_window_percent: 95`),
/// and configures function `apply_patch`.
pub fn inject_subagent_models_metadata(body: &[u8]) -> Vec<u8> {
    let primary = read_primary_model_from_config();
    inject_subagent_models_metadata_with_primary(body, primary.as_deref())
}

/// Inject subagent model metadata descriptors with an optional configured primary parent model preference
/// and an optional `<codex_home>` directory for discovering custom role manifests.
pub fn inject_subagent_models_metadata_in_dir(
    body: &[u8],
    primary_model: Option<&str>,
    codex_home: Option<&Path>,
) -> Vec<u8> {
    let Ok(mut json) = serde_json::from_slice::<Value>(body) else {
        return body.to_vec();
    };

    let Some(obj) = json.as_object_mut() else {
        return body.to_vec();
    };

    let models_key = if obj.contains_key("models") {
        "models"
    } else if obj.contains_key("data") {
        "data"
    } else {
        return body.to_vec();
    };

    let Some(models_arr) = obj.get_mut(models_key).and_then(|v| v.as_array_mut()) else {
        return body.to_vec();
    };

    // Prefer the user's configured primary parent model from config.toml (if present in models_arr),
    // then "gpt-6-luna", then the first non-subagent parent model. Never select a subagent model as template.
    let primary_clean = primary_model
        .map(|s| s.trim())
        .filter(|s| !s.is_empty() && !is_subagent_model_name_in_dir(s, codex_home));
    let template = primary_clean
        .and_then(|prim| {
            models_arr.iter().find(|item| {
                item.get("slug")
                    .or_else(|| item.get("id"))
                    .and_then(|s| s.as_str())
                    .is_some_and(|s| s.eq_ignore_ascii_case(prim))
            })
        })
        .or_else(|| {
            models_arr.iter().find(|item| {
                item.get("slug")
                    .or_else(|| item.get("id"))
                    .and_then(|s| s.as_str())
                    .is_some_and(|s| s.eq_ignore_ascii_case("gpt-6-luna"))
            })
        })
        .or_else(|| {
            models_arr.iter().find(|item| {
                item.get("slug")
                    .or_else(|| item.get("id"))
                    .and_then(|s| s.as_str())
                    .is_some_and(|s| !is_subagent_model_name_in_dir(s, codex_home))
            })
        })
        .cloned();

    let mut slugs_to_ensure = vec![
        "9router-subagent".to_string(),
        "implement".to_string(),
        "explore".to_string(),
        "review".to_string(),
    ];
    for m in collect_configured_subagent_models_in_dir(codex_home) {
        if !m.is_empty()
            && !is_parent_chatgpt_model(&m)
            && !slugs_to_ensure
                .iter()
                .any(|existing| existing.eq_ignore_ascii_case(&m))
        {
            slugs_to_ensure.push(m);
        }
    }

    for slug in slugs_to_ensure {
        if let Some(existing_item) = models_arr.iter_mut().find(|item| {
            item.get("slug")
                .or_else(|| item.get("id"))
                .and_then(|s| s.as_str())
                .is_some_and(|s| s.eq_ignore_ascii_case(&slug))
        }) {
            if let Some(entry_obj) = existing_item.as_object_mut() {
                if let Some(ref tmpl) = template {
                    if let Some(tmpl_obj) = tmpl.as_object() {
                        for (k, v) in tmpl_obj {
                            entry_obj.entry(k.clone()).or_insert_with(|| v.clone());
                        }
                    }
                }
                apply_subagent_model_metadata_fields(entry_obj, &slug, template.as_ref());
            }
            continue;
        }

        let mut entry = if let Some(ref tmpl) = template {
            tmpl.clone()
        } else {
            serde_json::json!({
                "slug": slug,
                "display_name": format!("{} (9Router)", slug),
                "description": "Subagent model routed via Codex 9Router Proxy",
                "prefer_websockets": false,
                "context_window": 872000,
                "max_context_window": 872000,
                "auto_compact_token_limit": null,
                "effective_context_window_percent": 95,
                "comp_hash": "3000",
                "max_output_tokens": 64000,
                "default_reasoning_level": "high",
                "supported_reasoning_levels": [
                    {"effort": "low", "description": "Fast responses"},
                    {"effort": "medium", "description": "Balanced reasoning"},
                    {"effort": "high", "description": "Deep reasoning"},
                    {"effort": "xhigh", "description": "Extra high reasoning depth"},
                    {"effort": "max", "description": "Maximum reasoning depth"}
                ],
                "shell_type": "shell_command",
                "visibility": "list",
                "supported_in_api": true,
                "priority": 99,
                "apply_patch_tool_type": "function",
                "truncation_policy": {
                    "mode": "tokens",
                    "limit": 10000
                },
                "supports_parallel_tool_calls": true,
                "supports_reasoning_summaries": true,
                "supports_search_tool": false,
                "supports_experimental_context": false,
                "use_responses_lite": true,
                "experimental_supported_tools": [],
                "input_modalities": ["text", "image"]
            })
        };

        if let Some(entry_obj) = entry.as_object_mut() {
            apply_subagent_model_metadata_fields(entry_obj, &slug, template.as_ref());
        }

        models_arr.push(entry);
    }

    serde_json::to_vec(&json).unwrap_or_else(|_| body.to_vec())
}

/// Inject subagent model metadata descriptors with an optional configured primary parent model preference.
pub fn inject_subagent_models_metadata_with_primary(
    body: &[u8],
    primary_model: Option<&str>,
) -> Vec<u8> {
    let codex_home = get_codex_home_dir();
    inject_subagent_models_metadata_in_dir(body, primary_model, codex_home.as_deref())
}

/// Apply `inject_subagent_models_metadata` to a `models_cache.json` file on disk if present.
/// Uses an atomic temporary file + rename (with direct write fallback) so concurrent processes never read partial JSON.
/// Never logs or exposes file contents.
pub fn sync_models_cache_file(cache_path: &Path) -> io::Result<bool> {
    if !cache_path.is_file() {
        return Ok(false);
    }
    let raw = fs::read(cache_path)?;
    let enriched = inject_subagent_models_metadata(&raw);
    if enriched != raw {
        let tmp_path = cache_path.with_file_name(format!(
            "{}.tmp.{}",
            cache_path
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("models_cache.json"),
            std::process::id()
        ));
        if fs::write(&tmp_path, &enriched).is_ok() {
            if fs::rename(&tmp_path, cache_path).is_err() {
                let _ = fs::remove_file(&tmp_path);
                fs::write(cache_path, &enriched)?;
            }
        } else {
            fs::write(cache_path, &enriched)?;
        }
    }
    Ok(true)
}

/// Synchronize `~/.codex/models_cache.json` in place if present on disk so `codex.orig.exe`
/// never loads stale `200000` or `272000` subagent metadata on a cache hit.
pub fn sync_codex_models_cache() -> Option<PathBuf> {
    let codex_home = get_codex_home_dir()?;
    let cache_path = codex_home.join("models_cache.json");
    if sync_models_cache_file(&cache_path).ok()? {
        Some(cache_path)
    } else {
        None
    }
}

/// Instruction appended to `"input"` when transforming a subagent `"generate": false`
/// or `x-codex-turn-metadata` remote-compaction request into a 9Router summarization request.
pub const COMPACTION_SUMMARIZATION_PROMPT: &str = "Summarize the conversation state, key findings, file changes, and remaining tasks concisely so the agent can continue seamlessly after context compaction.";

fn is_compaction_turn_metadata_substring(s: &str) -> bool {
    let lower = s.to_ascii_lowercase();
    lower.contains("\"request_kind\":\"compaction\"")
        || lower.contains("\"request_kind\": \"compaction\"")
        || lower.contains("\"requestkind\":\"compaction\"")
        || lower.contains("\"requestkind\": \"compaction\"")
        || lower.contains("\"request_kind\":\"compact\"")
        || lower.contains("\"request_kind\": \"compact\"")
        || lower.contains("\"requestkind\":\"compact\"")
        || lower.contains("\"requestkind\": \"compact\"")
        || lower.contains("\"subagent_kind\":\"compact\"")
        || lower.contains("\"subagent_kind\": \"compact\"")
        || lower.contains("\"subagentkind\":\"compact\"")
        || lower.contains("\"subagentkind\": \"compact\"")
        || lower.contains("\"compaction\":{")
        || lower.contains("\"compaction\": {")
        || lower.contains("\"compaction\":true")
        || lower.contains("\"compaction\": true")
}

fn is_compaction_turn_metadata_str(raw: &str) -> bool {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return false;
    }
    if trimmed.eq_ignore_ascii_case("compaction") || trimmed.eq_ignore_ascii_case("compact") {
        return true;
    }
    if let Ok(val) = serde_json::from_str::<Value>(trimmed) {
        if val.is_object() && is_compaction_metadata_value(&val) {
            return true;
        }
    }
    if let Some(decoded) = decode_base64_utf8(trimmed) {
        let dec_trimmed = decoded.trim();
        if dec_trimmed.eq_ignore_ascii_case("compaction")
            || dec_trimmed.eq_ignore_ascii_case("compact")
        {
            return true;
        }
        if let Ok(val) = serde_json::from_str::<Value>(dec_trimmed) {
            if val.is_object() && is_compaction_metadata_value(&val) {
                return true;
            }
        }
        if is_compaction_turn_metadata_substring(dec_trimmed) {
            return true;
        }
    }
    is_compaction_turn_metadata_substring(trimmed)
}

fn is_compaction_metadata_value(val: &Value) -> bool {
    match val {
        Value::String(s) => is_compaction_turn_metadata_str(s),
        Value::Object(map) => {
            if map
                .get("request_kind")
                .or_else(|| map.get("requestKind"))
                .or_else(|| map.get("subagent_kind"))
                .or_else(|| map.get("subagentKind"))
                .or_else(|| map.get("mode"))
                .and_then(|v| v.as_str())
                .is_some_and(|s| {
                    let t = s.trim();
                    t.eq_ignore_ascii_case("compaction") || t.eq_ignore_ascii_case("compact")
                })
            {
                return true;
            }
            if let Some(comp) = map.get("compaction") {
                match comp {
                    Value::Object(_) => return true,
                    Value::Bool(true) => return true,
                    Value::String(s) if !s.trim().is_empty() => return true,
                    _ => {}
                }
            }
            if let Some(turn_meta) = map
                .get("x-codex-turn-metadata")
                .or_else(|| map.get("turn_metadata"))
                .or_else(|| map.get("turnMetadata"))
            {
                if is_compaction_metadata_value(turn_meta) {
                    return true;
                }
            }
            false
        }
        _ => false,
    }
}

/// Detect whether HTTP headers indicate a remote-compaction request
/// (e.g. `x-codex-turn-metadata` containing `"request_kind":"compaction"` or `"compaction":{...}`,
/// including base64-encoded header values and `"compact"` enum variants).
pub fn is_compaction_http_headers(headers: &HeaderMap) -> bool {
    for val in headers.get_all("x-codex-turn-metadata") {
        if let Ok(s) = val.to_str() {
            if is_compaction_turn_metadata_str(s) {
                return true;
            }
        }
    }
    for key in ["x-codex-request-kind", "x-openai-request-kind"] {
        for val in headers.get_all(key) {
            if let Ok(s) = val.to_str() {
                let t = s.trim();
                if t.eq_ignore_ascii_case("compaction") || t.eq_ignore_ascii_case("compact") {
                    return true;
                }
            }
        }
    }
    false
}

/// Detect whether a `/responses` JSON request payload is a remote-compaction request
/// (`"generate": false`, `"request_kind": "compaction"`, `"compaction": {...}`, or `"client_metadata"` / `"clientMetadata"`).
pub fn is_compaction_request(body: &[u8]) -> bool {
    let Ok(val) = serde_json::from_slice::<Value>(body) else {
        return false;
    };
    if val.get("generate").and_then(|g| g.as_bool()) == Some(false) {
        return true;
    }
    if is_compaction_metadata_value(&val) {
        return true;
    }
    if val
        .get("client_metadata")
        .or_else(|| val.get("clientMetadata"))
        .is_some_and(is_compaction_metadata_value)
    {
        return true;
    }
    false
}

/// Detect whether an HTTP `/responses` request is a remote-compaction request
/// using both HTTP headers (`x-codex-turn-metadata`) and the JSON request body.
pub fn is_compaction_request_with_headers(headers: &HeaderMap, body: &[u8]) -> bool {
    is_compaction_http_headers(headers) || is_compaction_request(body)
}

/// Transform a subagent remote-compaction request (`"generate": false` or `x-codex-turn-metadata` compaction)
/// into a 9Router summarization request:
/// - Ensures subagent model rewrite and input sanitization (`sanitize_subagent_request_for_9router`)
/// - Removes `"generate"`, `"request_kind"`, `"compaction"`, `"client_metadata"`, `"tools"`, `"tool_choice"`, `"parallel_tool_calls"`, `"include"`, `"service_tier"`, `"previous_response_id"`, `"prompt_cache_key"`, `"access_programs"`, `"codex_output_schema"`, `"store"`, and `"stream_options"`
/// - Converts tool-call/output and reasoning history items in `"input"` into plain `"type": "message"` items
///   so tool-less summarization requests are never rejected by downstream providers
/// - Appends a summarization instruction user message to `"input"` unless a compaction prompt is already present
pub fn build_9router_compaction_request_body(body: &[u8]) -> Option<Vec<u8>> {
    let mut json = serde_json::from_slice::<Value>(body).ok()?;
    sanitize_subagent_request_for_9router(&mut json);

    let obj = json.as_object_mut()?;
    let needs_model_rewrite = match obj.get("model").and_then(|v| v.as_str()) {
        None => true,
        Some(m) => m.is_empty() || is_parent_chatgpt_model(m),
    };
    if needs_model_rewrite {
        let detected_role = find_agent_role(&Value::Object(obj.clone()));
        let mapped = map_role_to_model(detected_role.as_deref());
        obj.insert("model".to_string(), Value::String(mapped));
    }

    for key in [
        "generate",
        "request_kind",
        "requestKind",
        "compaction",
        "client_metadata",
        "clientMetadata",
        "tools",
        "tool_choice",
        "parallel_tool_calls",
        "include",
        "service_tier",
        "previous_response_id",
        "prompt_cache_key",
        "access_programs",
        "codex_output_schema",
        "store",
        "stream_options",
    ] {
        obj.remove(key);
    }

    let instruction_item = serde_json::json!({
        "type": "message",
        "role": "user",
        "content": [
            {
                "type": "input_text",
                "text": COMPACTION_SUMMARIZATION_PROMPT
            }
        ]
    });

    match obj.get_mut("input") {
        Some(Value::Array(arr)) => {
            let mut normalized_items = Vec::with_capacity(arr.len() + 1);
            for item in arr.drain(..) {
                let Some(mut item_obj) = item.as_object().cloned() else {
                    normalized_items.push(item);
                    continue;
                };
                let item_type = item_obj
                    .get("type")
                    .and_then(|t| t.as_str())
                    .unwrap_or("message")
                    .to_ascii_lowercase();
                match item_type.as_str() {
                    "function_call" | "local_shell_call" | "web_search_call" => {
                        let name = item_obj
                            .get("name")
                            .and_then(|v| v.as_str())
                            .unwrap_or(item_type.as_str())
                            .to_string();
                        let args = item_obj
                            .get("arguments")
                            .or_else(|| item_obj.get("input"))
                            .and_then(extract_text_from_content_value)
                            .unwrap_or_default();
                        let text = if args.trim().is_empty() {
                            format!("[Tool Call: {}]", name)
                        } else {
                            format!("[Tool Call: {}]\n{}", name, args)
                        };
                        normalized_items.push(serde_json::json!({
                            "type": "message",
                            "role": "assistant",
                            "content": [{"type": "output_text", "text": text}]
                        }));
                    }
                    "function_call_output"
                    | "local_shell_call_output"
                    | "web_search_call_output" => {
                        let out_text = item_obj
                            .get("output")
                            .and_then(extract_text_from_content_value)
                            .unwrap_or_default();
                        let text = if out_text.trim().is_empty() {
                            "[Tool Output]".to_string()
                        } else {
                            format!("[Tool Output]\n{}", out_text)
                        };
                        normalized_items.push(serde_json::json!({
                            "type": "message",
                            "role": "user",
                            "content": [{"type": "input_text", "text": text}]
                        }));
                    }
                    "reasoning" => {
                        if let Some(summary_val) = item_obj.get("summary") {
                            if let Some(summary_text) = extract_text_from_content_value(summary_val)
                            {
                                if !summary_text.trim().is_empty() {
                                    normalized_items.push(serde_json::json!({
                                        "type": "message",
                                        "role": "assistant",
                                        "content": [{"type": "output_text", "text": summary_text}]
                                    }));
                                }
                            }
                        }
                    }
                    _ => {
                        item_obj.remove("internal_chat_message_metadata_passthrough");
                        normalized_items.push(Value::Object(item_obj));
                    }
                }
            }
            let already_has_compaction_prompt = normalized_items.iter().any(|item| {
                item.get("content")
                    .and_then(extract_text_from_content_value)
                    .is_some_and(|t| {
                        t.contains("CONTEXT CHECKPOINT COMPACTION")
                            || t.contains(COMPACTION_SUMMARIZATION_PROMPT)
                    })
            });
            if !already_has_compaction_prompt {
                normalized_items.push(instruction_item);
            }
            *arr = normalized_items;
        }
        Some(Value::String(s)) => {
            let prev = s.clone();
            let mut arr = Vec::new();
            let already_has_compaction_prompt = prev.contains("CONTEXT CHECKPOINT COMPACTION")
                || prev.contains(COMPACTION_SUMMARIZATION_PROMPT);
            if !prev.trim().is_empty() {
                arr.push(serde_json::json!({
                    "type": "message",
                    "role": "user",
                    "content": [{"type": "input_text", "text": prev}]
                }));
            }
            if !already_has_compaction_prompt {
                arr.push(instruction_item);
            }
            obj.insert("input".to_string(), Value::Array(arr));
        }
        _ => {
            obj.insert("input".to_string(), Value::Array(vec![instruction_item]));
        }
    }

    serde_json::to_vec(&json).ok()
}

fn extract_text_from_content_value(content: &Value) -> Option<String> {
    match content {
        Value::String(s) => {
            let trimmed = s.trim();
            if !trimmed.is_empty() {
                Some(trimmed.to_string())
            } else {
                None
            }
        }
        Value::Array(arr) => {
            let mut parts = Vec::new();
            for part in arr {
                if let Some(s) = part.as_str() {
                    let trimmed = s.trim();
                    if !trimmed.is_empty() {
                        parts.push(trimmed.to_string());
                    }
                } else if let Some(part_obj) = part.as_object() {
                    for key in ["text", "output_text", "input_text", "summary_text", "encrypted_content"] {
                        if let Some(t) = part_obj.get(key).and_then(|v| v.as_str()) {
                            let trimmed = t.trim();
                            if !trimmed.is_empty() {
                                parts.push(trimmed.to_string());
                                break;
                            }
                        }
                    }
                }
            }
            if parts.is_empty() {
                None
            } else {
                Some(parts.join("\n"))
            }
        }
        _ => None,
    }
}

fn extract_text_from_output_item(item: &Value) -> Option<String> {
    let obj = item.as_object()?;
    let item_type = obj.get("type").and_then(|v| v.as_str()).unwrap_or("message");
    if item_type.eq_ignore_ascii_case("compaction")
        || item_type.eq_ignore_ascii_case("compaction_summary")
    {
        let text = extract_compaction_item_text(obj);
        if !text.is_empty() {
            return Some(text);
        }
    }
    if item_type.eq_ignore_ascii_case("message") || item_type.eq_ignore_ascii_case("agent_message") {
        if let Some(content) = obj.get("content") {
            if let Some(t) = extract_text_from_content_value(content) {
                return Some(t);
            }
        }
        if let Some(t) = obj.get("text").and_then(|v| v.as_str()) {
            let trimmed = t.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
    }
    None
}

fn extract_text_from_response_json(val: &Value) -> Option<String> {
    let obj = val.as_object()?;
    if let Some(ot) = obj.get("output_text").and_then(|v| v.as_str()) {
        let trimmed = ot.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }
    if let Some(output_arr) = obj.get("output").and_then(|v| v.as_array()) {
        let mut texts = Vec::new();
        for item in output_arr {
            if let Some(t) = extract_text_from_output_item(item) {
                texts.push(t);
            }
        }
        if !texts.is_empty() {
            return Some(texts.join("\n"));
        }
    }
    if let Some(choices) = obj.get("choices").and_then(|v| v.as_array()) {
        for choice in choices {
            if let Some(msg) = choice.get("message") {
                if let Some(content) = msg.get("content") {
                    if let Some(t) = extract_text_from_content_value(content) {
                        return Some(t);
                    }
                }
            }
        }
    }
    if let Some(inner_resp) = obj.get("response") {
        if let Some(t) = extract_text_from_response_json(inner_resp) {
            return Some(t);
        }
    }
    None
}

/// Extract the assistant summary text from a 9Router `/v1/responses` response
/// (supports both JSON responses and SSE `text/event-stream` responses).
pub fn extract_summary_from_9router_response(resp_bytes: &[u8]) -> Option<String> {
    if resp_bytes.is_empty() {
        return None;
    }

    if let Ok(val) = serde_json::from_slice::<Value>(resp_bytes) {
        if let Some(t) = extract_text_from_response_json(&val) {
            return Some(t);
        }
    }

    let text = String::from_utf8_lossy(resp_bytes);
    let mut delta_acc = String::new();
    let mut completed_texts: Vec<String> = Vec::new();

    for line in text.lines() {
        let trimmed = line.trim();
        let Some(data_str) = trimmed.strip_prefix("data:") else {
            continue;
        };
        let payload = data_str.trim();
        if payload.is_empty() || payload == "[DONE]" {
            continue;
        }
        let Ok(ev) = serde_json::from_str::<Value>(payload) else {
            continue;
        };
        let ev_type = ev.get("type").and_then(|v| v.as_str()).unwrap_or("");
        match ev_type {
            "response.output_text.delta" => {
                if let Some(d) = ev.get("delta").and_then(|v| v.as_str()) {
                    delta_acc.push_str(d);
                }
            }
            "response.output_text.done" => {
                if let Some(t) = ev.get("text").and_then(|v| v.as_str()) {
                    let t_trim = t.trim();
                    if !t_trim.is_empty() {
                        completed_texts.push(t_trim.to_string());
                    }
                }
            }
            "response.output_item.done" => {
                if let Some(item) = ev.get("item") {
                    if let Some(t) = extract_text_from_output_item(item) {
                        completed_texts.push(t);
                    }
                }
            }
            "response.completed" | "response.done" => {
                if let Some(resp) = ev.get("response") {
                    if let Some(t) = extract_text_from_response_json(resp) {
                        completed_texts = vec![t];
                    }
                }
            }
            _ => {
                if let Some(choices) = ev.get("choices").and_then(|v| v.as_array()) {
                    for choice in choices {
                        if let Some(d) = choice
                            .get("delta")
                            .and_then(|d| d.get("content"))
                            .and_then(|c| c.as_str())
                        {
                            delta_acc.push_str(d);
                        }
                    }
                } else if let Some(t) = extract_text_from_response_json(&ev) {
                    completed_texts.push(t);
                }
            }
        }
    }

    if !completed_texts.is_empty() {
        let joined = completed_texts.join("\n");
        let trimmed = joined.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }

    let delta_trimmed = delta_acc.trim();
    if !delta_trimmed.is_empty() {
        return Some(delta_trimmed.to_string());
    }

    None
}

fn truncate_chars(s: &str, max_chars: usize) -> String {
    let mut chars = s.chars();
    let taken: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        format!("{}...", taken)
    } else {
        taken
    }
}

/// Construct a deterministic local summary from the request's `"input"` (or `"messages"`)
/// if 9Router returns an error or an empty summary during remote compaction.
pub fn build_deterministic_local_summary(req_json: &Value) -> String {
    let mut entries: Vec<String> = Vec::new();

    if let Some(input_arr) = req_json.get("input").and_then(|v| v.as_array()) {
        for item in input_arr {
            let Some(obj) = item.as_object() else {
                if let Some(s) = item.as_str() {
                    let trimmed = s.trim();
                    if !trimmed.is_empty() && trimmed != COMPACTION_SUMMARIZATION_PROMPT {
                        entries.push(format!("- user: {}", truncate_chars(trimmed, 600)));
                    }
                }
                continue;
            };
            let item_type = obj.get("type").and_then(|v| v.as_str()).unwrap_or("message");
            if item_type.eq_ignore_ascii_case("compaction")
                || item_type.eq_ignore_ascii_case("compaction_summary")
            {
                let c_text = extract_compaction_item_text(obj);
                if !c_text.is_empty() {
                    entries.push(format!("- prior_summary: {}", truncate_chars(&c_text, 800)));
                }
                continue;
            }
            if item_type.eq_ignore_ascii_case("function_call")
                || item_type.eq_ignore_ascii_case("custom_tool_call")
            {
                let name = obj.get("name").and_then(|v| v.as_str()).unwrap_or("tool");
                let args = obj
                    .get("arguments")
                    .or_else(|| obj.get("input"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                entries.push(format!("- tool_call({}): {}", name, truncate_chars(args.trim(), 300)));
                continue;
            }
            if item_type.eq_ignore_ascii_case("function_call_output")
                || item_type.eq_ignore_ascii_case("custom_tool_call_output")
            {
                if let Some(out_val) = obj.get("output") {
                    if let Some(out_text) = extract_text_from_content_value(out_val) {
                        entries.push(format!(
                            "- tool_output: {}",
                            truncate_chars(&out_text, 400)
                        ));
                    }
                }
                continue;
            }
            let role = obj
                .get("role")
                .and_then(|v| v.as_str())
                .unwrap_or("user")
                .to_lowercase();
            if let Some(content) = obj.get("content") {
                if let Some(text) = extract_text_from_content_value(content) {
                    if text.trim() == COMPACTION_SUMMARIZATION_PROMPT {
                        continue;
                    }
                    entries.push(format!("- {}: {}", role, truncate_chars(&text, 600)));
                }
            }
        }
    } else if let Some(input_str) = req_json.get("input").and_then(|v| v.as_str()) {
        let trimmed = input_str.trim();
        if !trimmed.is_empty() && trimmed != COMPACTION_SUMMARIZATION_PROMPT {
            entries.push(format!("- user: {}", truncate_chars(trimmed, 600)));
        }
    } else if let Some(messages_arr) = req_json.get("messages").and_then(|v| v.as_array()) {
        for msg in messages_arr {
            if let Some(obj) = msg.as_object() {
                let role = obj
                    .get("role")
                    .and_then(|v| v.as_str())
                    .unwrap_or("user")
                    .to_lowercase();
                if let Some(content) = obj.get("content") {
                    if let Some(text) = extract_text_from_content_value(content) {
                        entries.push(format!("- {}: {}", role, truncate_chars(&text, 600)));
                    }
                }
            }
        }
    }

    if entries.is_empty() {
        return "Context compacted locally by Codex 9Router Proxy. Proceed with the delegated subagent task.".to_string();
    }

    // Keep most recent entries if there are many turns
    let start_idx = entries.len().saturating_sub(20);
    let body = entries[start_idx..].join("\n");
    truncate_chars(&body, 6000)
}

/// Convenience helper to construct a deterministic local summary directly from raw JSON request bytes.
pub fn build_deterministic_local_summary_from_bytes(body: &[u8]) -> String {
    if let Ok(json) = serde_json::from_slice::<Value>(body) {
        build_deterministic_local_summary(&json)
    } else {
        "Context compacted locally by Codex 9Router Proxy. Proceed with the delegated subagent task.".to_string()
    }
}

/// Format a valid Responses API `text/event-stream` SSE payload containing `response.created`,
/// `response.output_item.done` (`"item": {"type": "compaction", "id": "cmp_...", "encrypted_content": "<summary>"}`),
/// and `response.completed`.
pub fn build_compaction_sse_stream(summary: &str, model: &str) -> String {
    let effective_summary = if summary.trim().is_empty() {
        "Context compacted locally by Codex 9Router Proxy."
    } else {
        summary.trim()
    };
    let effective_model = if model.trim().is_empty() {
        "9router-subagent"
    } else {
        model.trim()
    };
    let resp_id = "resp_9router_compact";
    let item_id = "cmp_9router_compact";

    let created = serde_json::json!({
        "type": "response.created",
        "response": {
            "id": resp_id,
            "object": "response",
            "status": "in_progress",
            "model": effective_model,
            "output": []
        }
    });
    let item_done = serde_json::json!({
        "type": "response.output_item.done",
        "output_index": 0,
        "item": {
            "type": "compaction",
            "id": item_id,
            "encrypted_content": effective_summary
        }
    });
    let completed = serde_json::json!({
        "type": "response.completed",
        "response": {
            "id": resp_id,
            "object": "response",
            "status": "completed",
            "model": effective_model,
            "output": [
                {
                    "type": "compaction",
                    "id": item_id,
                    "encrypted_content": effective_summary
                }
            ],
            "usage": {
                "input_tokens": 0,
                "input_tokens_details": { "cached_tokens": 0 },
                "output_tokens": 0,
                "output_tokens_details": { "reasoning_tokens": 0 },
                "total_tokens": 0
            }
        }
    });

    format!(
        "event: response.created\ndata: {}\n\nevent: response.output_item.done\ndata: {}\n\nevent: response.completed\ndata: {}\n\n",
        created, item_done, completed
    )
}

/// Build an Axum HTTP `200 OK` `text/event-stream` response for `compact_remote_v2_attempt`.
pub fn build_compaction_sse_response(summary: &str, model: &str) -> Response {
    let sse_payload = build_compaction_sse_stream(summary, model);
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/event-stream; charset=utf-8")
        .header(header::CACHE_CONTROL, "no-cache")
        .body(Body::from(sse_payload))
        .unwrap_or_else(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Internal error building compaction SSE response: {}", e),
            )
                .into_response()
        })
}

/// Handle a subagent remote-compaction request (`"generate": false` or `x-codex-turn-metadata` compaction)
/// strictly via 9Router with a deterministic local fallback summary if 9Router returns an error or empty summary.
pub async fn handle_subagent_compaction(
    client: &reqwest::Client,
    target_url: &str,
    mut forward_headers: HeaderMap,
    routed_body: &[u8],
) -> Response {
    forward_headers.remove("x-codex-turn-metadata");
    forward_headers.remove("x-codex-request-kind");
    forward_headers.remove("x-openai-request-kind");
    forward_headers.remove(header::ACCEPT_ENCODING);

    let model_name = serde_json::from_slice::<Value>(routed_body)
        .ok()
        .and_then(|v| v.get("model").and_then(|m| m.as_str()).map(|s| s.to_string()))
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| map_role_to_model(Some("default")));

    if let Some(sum_body) = build_9router_compaction_request_body(routed_body) {
        if let Ok((mut upstream_res, _)) = send_upstream_with_retry(
            client,
            Method::POST,
            target_url,
            forward_headers.clone(),
            Bytes::from(sum_body.clone()),
        )
        .await
        {
            if upstream_res.status().as_u16() >= 400 {
                if let Some(fallback_body) = rewrite_body_model_to_fallback(&sum_body) {
                    if let Ok((retry_res, _)) = send_upstream_with_retry(
                        client,
                        Method::POST,
                        target_url,
                        forward_headers,
                        Bytes::from(fallback_body),
                    )
                    .await
                    {
                        upstream_res = retry_res;
                    }
                }
            }

            if upstream_res.status() == reqwest::StatusCode::OK {
                if let Ok(resp_bytes) = upstream_res.bytes().await {
                    if let Some(summary) = extract_summary_from_9router_response(&resp_bytes) {
                        return build_compaction_sse_response(&summary, &model_name);
                    }
                }
            }
        }
    }

    let fallback_summary = build_deterministic_local_summary_from_bytes(routed_body);
    build_compaction_sse_response(&fallback_summary, &model_name)
}

/// Resolve the forward target URL for a given path, routing classification, and optional subagent role.
pub fn resolve_forward_url_for_role(
    path: &str,
    is_subagent: bool,
    role: Option<&str>,
) -> String {
    if is_subagent {
        get_subagent_responses_url_for_role(role)
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

/// Resolve the forward target URL for a given path and routing classification.
pub fn resolve_forward_url(path: &str, is_subagent: bool) -> String {
    resolve_forward_url_for_role(path, is_subagent, None)
}

/// Resolve the forward target URL preserving optional query parameters and optional subagent role.
pub fn resolve_forward_url_with_query_for_role(
    path: &str,
    query: Option<&str>,
    is_subagent: bool,
    role: Option<&str>,
) -> String {
    let base_url = resolve_forward_url_for_role(path, is_subagent, role);
    if let Some(q) = query {
        if !q.is_empty() && !base_url.contains('?') {
            return format!("{}?{}", base_url, q);
        }
    }
    base_url
}

/// Resolve the forward target URL preserving optional query parameters.
pub fn resolve_forward_url_with_query(
    path: &str,
    query: Option<&str>,
    is_subagent: bool,
) -> String {
    resolve_forward_url_with_query_for_role(path, query, is_subagent, None)
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

/// Build headers to forward upstream with an explicit optional subagent authorization header.
/// Never forwards the user's incoming ChatGPT OAuth `Authorization` header to subagent endpoints.
pub fn build_forward_headers_with_subagent_auth(
    incoming_headers: &HeaderMap,
    is_subagent: bool,
    subagent_auth: Option<&str>,
) -> HeaderMap {
    let mut out = HeaderMap::new();

    for (key, value) in incoming_headers.iter() {
        let key_str = key.as_str();
        if is_hop_by_hop_header(key_str) || key_str.eq_ignore_ascii_case("content-length") {
            continue;
        }
        if is_subagent && key_str.eq_ignore_ascii_case("authorization") {
            continue;
        }
        if is_subagent && key_str.eq_ignore_ascii_case("content-encoding") {
            // Subagent requests are forwarded as decompressed JSON
            continue;
        }
        out.insert(key.clone(), value.clone());
    }

    if is_subagent {
        let auth_val = subagent_auth
            .and_then(format_bearer_header_value)
            .and_then(|auth_str| HeaderValue::from_str(&auth_str).ok())
            .unwrap_or_else(|| HeaderValue::from_static("Bearer dummy-9router-key"));
        out.insert(header::AUTHORIZATION, auth_val);
    }

    out
}

/// Build headers to forward upstream to either 9Router or ChatGPT for an optional subagent role.
/// Never forwards the user's incoming ChatGPT OAuth `Authorization` header to subagent endpoints.
pub fn build_forward_headers_for_role(
    incoming_headers: &HeaderMap,
    is_subagent: bool,
    role: Option<&str>,
) -> HeaderMap {
    let subagent_auth = if is_subagent {
        get_subagent_auth_header_for_role(role)
    } else {
        None
    };
    build_forward_headers_with_subagent_auth(incoming_headers, is_subagent, subagent_auth.as_deref())
}

/// Build headers to forward upstream to either 9Router or ChatGPT.
/// Never forwards the user's incoming ChatGPT OAuth `Authorization` header to subagent endpoints.
pub fn build_forward_headers(incoming_headers: &HeaderMap, is_subagent: bool) -> HeaderMap {
    build_forward_headers_for_role(incoming_headers, is_subagent, None)
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

fn generate_and_write_tls_files(
    cert_path: &Path,
    key_path: &Path,
) -> Result<(String, String), Box<dyn std::error::Error + Send + Sync>> {
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

    generate_and_write_tls_files(cert_path, key_path)
}

fn try_build_tls_acceptor_from_disk(
    cert_path: &Path,
    key_path: &Path,
) -> Result<TlsAcceptor, Box<dyn std::error::Error + Send + Sync>> {
    let cert_pem = fs::read(cert_path)?;
    let key_pem = fs::read(key_path)?;

    let mut certs = Vec::new();
    for cert_result in rustls_pemfile::certs(&mut &cert_pem[..]) {
        certs.push(cert_result?);
    }
    if certs.is_empty() {
        return Err("No certificates found in cert file".into());
    }

    let key = rustls_pemfile::private_key(&mut &key_pem[..])?
        .ok_or("No private key found in key file")?;

    let mut server_config = ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)?;
    server_config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];

    Ok(TlsAcceptor::from(Arc::new(server_config)))
}

/// Build a `tokio_rustls::TlsAcceptor` supporting both HTTP/2 (`h2`) and `http/1.1`.
/// Automatically regenerates `bridge-cert.pem` and `bridge-key.pem` once if existing files are corrupted or mismatched.
pub fn create_tls_acceptor(
    cert_path: &Path,
    key_path: &Path,
) -> Result<TlsAcceptor, Box<dyn std::error::Error + Send + Sync>> {
    let _ = get_or_create_tls_files(cert_path, key_path)?;

    match try_build_tls_acceptor_from_disk(cert_path, key_path) {
        Ok(acceptor) => Ok(acceptor),
        Err(_) => {
            let _ = fs::remove_file(cert_path);
            let _ = fs::remove_file(key_path);
            let _ = generate_and_write_tls_files(cert_path, key_path)?;
            try_build_tls_acceptor_from_disk(cert_path, key_path)
        }
    }
}

#[derive(Clone)]
pub struct ProxyAppState {
    pub http_client: reqwest::Client,
}

/// Build the shared upstream `reqwest::Client` with a 10-second connect timeout,
/// 60-second TCP keepalive, and 90-second connection pool idle timeout.
pub fn build_upstream_http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .tcp_keepalive(Duration::from_secs(60))
        .pool_idle_timeout(Duration::from_secs(90))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
}

/// Walk the full `std::error::Error::source()` chain to expose all underlying causes
/// (such as hyper client errors, TLS handshake errors, or OS socket/DNS error codes like `os error 11001`).
pub fn format_error_chain(err: &dyn std::error::Error) -> String {
    let mut chain = vec![err.to_string()];
    let mut current = err.source();
    while let Some(source) = current {
        let msg = source.to_string();
        let trimmed = msg.trim();
        if !trimmed.is_empty() && !chain.iter().any(|prev| prev.contains(trimmed)) {
            chain.push(trimmed.to_string());
        }
        current = source.source();
    }
    chain.join(" -> ")
}

/// Format an upstream error chain with classification tags (`[connect]`, `[timeout]`, `[dns]`, `[request]`)
/// and diagnostic hints for `502 Bad Gateway` responses.
pub fn format_upstream_error_with_flags(
    target_url: &str,
    chain: &str,
    is_timeout: bool,
    is_connect: bool,
    is_request: bool,
) -> String {
    let lower = chain.to_lowercase();
    let mut categories = Vec::new();

    if is_timeout || lower.contains("timed out") || lower.contains("timeout") {
        categories.push("timeout");
    }
    if is_connect || lower.contains("client error (connect)") || lower.contains("connection refused") {
        categories.push("connect");
    }
    if lower.contains("dns")
        || lower.contains("11001")
        || lower.contains("no such host")
        || lower.contains("lookup")
        || lower.contains("getaddrinfo")
    {
        categories.push("dns");
    }
    if is_request && categories.is_empty() {
        categories.push("request");
    }

    let hint = if categories.contains(&"dns") {
        " [hint: DNS lookup failed (e.g. os error 11001); check local internet/DNS connectivity]"
    } else if categories.contains(&"timeout") {
        " [hint: upstream connection timed out; check network stability or firewall]"
    } else if categories.contains(&"connect") {
        " [hint: TCP/TLS connection failed; verify upstream endpoint is reachable]"
    } else if !categories.is_empty() {
        " [hint: transient upstream request/socket error; retried automatically]"
    } else {
        ""
    };

    let tag = if categories.is_empty() {
        String::new()
    } else {
        format!(" [{}]", categories.join("/"))
    };

    format!(
        "Codex 9Router Proxy upstream connection error to {}: {}{}{}",
        target_url, chain, tag, hint
    )
}

/// Format a `reqwest::Error` with its full `std::error::Error::source()` cause chain and
/// diagnostic connection/timeout/DNS hints for `502 Bad Gateway` responses.
pub fn format_reqwest_upstream_error(target_url: &str, err: &reqwest::Error) -> String {
    let chain = format_error_chain(err);
    format_upstream_error_with_flags(
        target_url,
        &chain,
        err.is_timeout(),
        err.is_connect(),
        err.is_request(),
    )
}

/// Send an HTTP request upstream with a single automatic retry when `send().await` fails
/// with a transient connection or request error (`err.is_connect() || err.is_request()`)
/// to recover from stale pooled HTTP/2 or TLS sockets after sleep/wake or network transitions.
pub async fn send_upstream_with_retry(
    client: &reqwest::Client,
    method: Method,
    target_url: &str,
    headers: HeaderMap,
    body: Bytes,
) -> Result<(reqwest::Response, usize), reqwest::Error> {
    let build_req = || {
        let mut req = client
            .request(method.clone(), target_url)
            .headers(headers.clone());
        if !body.is_empty() {
            req = req.body(body.clone());
        }
        req
    };

    match build_req().send().await {
        Ok(res) => Ok((res, 1)),
        Err(first_err) => {
            if first_err.is_connect() || first_err.is_request() {
                match build_req().send().await {
                    Ok(res) => Ok((res, 2)),
                    Err(retry_err) => Err(retry_err),
                }
            } else {
                Err(first_err)
            }
        }
    }
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

    let is_models_req = method == Method::GET && is_models_path(path);
    let (decompressed_body, _was_compressed) = decompress_if_needed(&body, &headers);
    let (is_subagent, routed_body) =
        inspect_and_route_http_request_with_headers(path, &headers, &decompressed_body);
    let effective_role = if is_subagent {
        let parsed_body = serde_json::from_slice::<Value>(&decompressed_body).ok();
        let body_role = parsed_body.as_ref().and_then(find_agent_role);
        let header_role = extract_role_from_http_headers(&headers);
        let primary_role = match (body_role, header_role) {
            (Some(r), Some(hr)) if r == "default" || r == "subagent" => Some(hr),
            (Some(r), _) => Some(r),
            (None, hr) => hr,
        };
        match primary_role.as_deref() {
            None | Some("default" | "subagent") => parsed_body
                .as_ref()
                .and_then(|v| v.get("model").and_then(|m| m.as_str()))
                .and_then(|m| find_role_for_model_in_dir(get_codex_home_dir().as_deref(), m))
                .or(primary_role),
            _ => primary_role,
        }
    } else {
        None
    };
    #[cfg(test)]
    let target_url = headers
        .get("x-codex-test-upstream")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
        .unwrap_or_else(|| {
            resolve_forward_url_with_query_for_role(
                path,
                query,
                is_subagent,
                effective_role.as_deref(),
            )
        });
    #[cfg(not(test))]
    let target_url = resolve_forward_url_with_query_for_role(
        path,
        query,
        is_subagent,
        effective_role.as_deref(),
    );
    let mut forward_headers =
        build_forward_headers_for_role(&headers, is_subagent, effective_role.as_deref());
    #[cfg(test)]
    forward_headers.remove("x-codex-test-upstream");

    // If this is a subagent remote-compaction request ("generate": false or x-codex-turn-metadata compaction),
    // handle it strictly via 9Router summarization + deterministic local fallback and return a compaction SSE stream.
    if is_subagent && is_compaction_request_with_headers(&headers, &routed_body) {
        return handle_subagent_compaction(
            &state.http_client,
            &target_url,
            forward_headers,
            &routed_body,
        )
        .await;
    }

    // For GET /backend-api/models, strip compression and conditional cache headers
    // so ChatGPT returns plain 200 OK JSON that we can enrich with 9Router subagent models.
    if is_models_req {
        forward_headers.remove(header::ACCEPT_ENCODING);
        forward_headers.remove(header::IF_NONE_MATCH);
        forward_headers.remove(header::IF_MODIFIED_SINCE);
    }

    let body_to_send = if is_subagent {
        Bytes::from(routed_body.clone())
    } else {
        body
    };

    let (mut upstream_res, _attempts) = match send_upstream_with_retry(
        &state.http_client,
        method.clone(),
        &target_url,
        forward_headers.clone(),
        body_to_send,
    )
    .await
    {
        Ok(res) => res,
        Err(err) => {
            return (
                StatusCode::BAD_GATEWAY,
                [(header::CONTENT_TYPE, "application/json")],
                serde_json::json!({
                    "error": {
                        "message": format_reqwest_upstream_error(&target_url, &err),
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
            if let Ok((retry_res, _)) = send_upstream_with_retry(
                &state.http_client,
                method,
                &target_url,
                forward_headers,
                Bytes::from(fallback_body),
            )
            .await
            {
                upstream_res = retry_res;
            }
        }
    }

    let status = StatusCode::from_u16(upstream_res.status().as_u16())
        .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    let mut clean_headers = filter_response_headers(upstream_res.headers());

    if is_models_req && status == StatusCode::OK {
        return match upstream_res.bytes().await {
            Ok(raw_bytes) => {
                let enriched = inject_subagent_models_metadata(&raw_bytes);
                let _ = sync_codex_models_cache();
                clean_headers.remove(header::CONTENT_ENCODING);
                clean_headers.remove(header::ETAG);
                let mut response = Response::builder().status(status);
                for (k, v) in clean_headers.iter() {
                    response = response.header(k, v);
                }
                response.body(Body::from(enriched)).unwrap_or_else(|e| {
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        format!("Internal error building models response: {}", e),
                    )
                        .into_response()
                })
            }
            Err(e) => (
                StatusCode::BAD_GATEWAY,
                [(header::CONTENT_TYPE, "application/json")],
                serde_json::json!({
                    "error": {
                        "message": format_reqwest_upstream_error(&target_url, &e),
                        "type": "proxy_gateway_error",
                        "code": 502
                    }
                })
                .to_string(),
            )
                .into_response(),
        };
    }

    let stream = upstream_res
        .bytes_stream()
        .map_err(std::io::Error::other);

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
        .route("/__codex_9router_proxy_healthz", get(health_handler))
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

/// Verify whether an existing listener on `bind_addr` is a healthy `codex-9router-proxy` TLS endpoint
/// trusted by the certificate at `cert_path`.
pub fn verify_existing_tls_proxy_listener(bind_addr: &str, cert_path: &Path) -> bool {
    let Ok(cert_bytes) = fs::read(cert_path) else {
        return false;
    };
    let Ok(cert) = reqwest::Certificate::from_pem(&cert_bytes) else {
        return false;
    };
    let url = format!("https://{}/health", bind_addr);
    thread::spawn(move || {
        let Ok(rt) = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        else {
            return false;
        };
        rt.block_on(async move {
            let Ok(client) = reqwest::Client::builder()
                .add_root_certificate(cert)
                .timeout(Duration::from_millis(800))
                .build()
            else {
                return false;
            };
            let Ok(resp) = client.get(&url).send().await else {
                return false;
            };
            if resp.status() != reqwest::StatusCode::OK {
                return false;
            }
            let Ok(body) = resp.text().await else {
                return false;
            };
            body.contains("codex-9router-proxy")
        })
    })
    .join()
    .unwrap_or(false)
}

/// Spawn a background Tokio runtime thread that accepts and serves dual-stack TLS + plain HTTP
/// requests on an already-bound `std::net::TcpListener`.
pub fn spawn_reverse_proxy_accept_thread(std_listener: TcpListener, tls_acceptor: TlsAcceptor) {
    let _ = std_listener.set_nonblocking(true);

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

            let http_client = build_upstream_http_client();

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
}

/// Spawn a lightweight background standby failover thread that polls `TcpListener::bind(&bind_addr)`
/// every `250ms` and immediately starts `spawn_reverse_proxy_accept_thread` if the primary listener exits.
pub fn spawn_reverse_proxy_standby_thread(bind_addr: String, tls_acceptor: TlsAcceptor) {
    thread::spawn(move || {
        loop {
            thread::sleep(Duration::from_millis(250));
            if let Ok(l) = TcpListener::bind(&bind_addr) {
                if l.set_nonblocking(true).is_ok() {
                    spawn_reverse_proxy_accept_thread(l, tls_acceptor);
                    break;
                }
            }
        }
    });
}

/// Bind and spawn the embedded dual-stack HTTP/HTTPS reverse proxy in a background thread,
/// returning `(cert_path, bound_port)`. If `bind_addr` is already in use, verifies that the
/// listener completes a TLS `/health` check trusted by `cert_path` and spawns an in-process
/// standby failover thread; otherwise binds a fallback port.
pub fn spawn_embedded_reverse_proxy_with_port(bind_addr: &str) -> io::Result<(PathBuf, String)> {
    let (cert_path, key_path) = default_cert_paths();
    let tls_acceptor = create_tls_acceptor(&cert_path, &key_path)
        .map_err(|e| io::Error::other(e.to_string()))?;

    let requested_port = bind_addr
        .rsplit(':')
        .next()
        .unwrap_or("20129")
        .to_string();

    let (std_listener, actual_port) = match TcpListener::bind(bind_addr) {
        Ok(l) => {
            let bound_port = l
                .local_addr()
                .map(|a| a.port().to_string())
                .unwrap_or_else(|_| requested_port.clone());
            (l, bound_port)
        }
        Err(e) if e.kind() == io::ErrorKind::AddrInUse => {
            if verify_existing_tls_proxy_listener(bind_addr, &cert_path) {
                spawn_reverse_proxy_standby_thread(bind_addr.to_string(), tls_acceptor);
                return Ok((cert_path, requested_port));
            }
            if let Ok(l) = TcpListener::bind(bind_addr) {
                let bound_port = l
                    .local_addr()
                    .map(|a| a.port().to_string())
                    .unwrap_or_else(|_| requested_port.clone());
                (l, bound_port)
            } else {
                let mut fallback_listener: Option<(TcpListener, String)> = None;
                for candidate_port in 20130..=20139u16 {
                    let candidate_addr = format!("127.0.0.1:{}", candidate_port);
                    if let Ok(l) = TcpListener::bind(&candidate_addr) {
                        fallback_listener = Some((l, candidate_port.to_string()));
                        break;
                    } else if verify_existing_tls_proxy_listener(&candidate_addr, &cert_path) {
                        spawn_reverse_proxy_standby_thread(candidate_addr, tls_acceptor);
                        return Ok((cert_path, candidate_port.to_string()));
                    }
                }
                if fallback_listener.is_none() {
                    if let Ok(l) = TcpListener::bind("127.0.0.1:0") {
                        if let Ok(addr) = l.local_addr() {
                            fallback_listener = Some((l, addr.port().to_string()));
                        }
                    }
                }
                match fallback_listener {
                    Some(pair) => pair,
                    None => return Err(e),
                }
            }
        }
        Err(e) => return Err(e),
    };
    std_listener.set_nonblocking(true)?;
    spawn_reverse_proxy_accept_thread(std_listener, tls_acceptor);

    Ok((cert_path, actual_port))
}

/// Bind and spawn the embedded dual-stack HTTP/HTTPS reverse proxy in a background thread.
pub fn spawn_embedded_reverse_proxy(bind_addr: &str) -> io::Result<PathBuf> {
    spawn_embedded_reverse_proxy_with_port(bind_addr).map(|(cert_path, _)| cert_path)
}

/// Compute the `--proxy-daemon` single-instance lockfile path inside `custom_dir` for `port`.
pub fn proxy_daemon_lockfile_path_in_dir(custom_dir: &Path, port: &str) -> PathBuf {
    let clean_port: String = port
        .trim()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .collect();
    let effective_port = if clean_port.is_empty() {
        "20129"
    } else {
        &clean_port
    };
    custom_dir.join(format!("proxy-daemon-{}.lock", effective_port))
}

/// Compute the default `--proxy-daemon` single-instance lockfile path (`%LOCALAPPDATA%\OpenAI\Codex\custom\proxy-daemon-<port>.lock`).
pub fn default_proxy_daemon_lockfile_path(port: &str) -> PathBuf {
    let (cert_path, _) = default_cert_paths();
    let custom_dir = cert_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(env::temp_dir);
    proxy_daemon_lockfile_path_in_dir(&custom_dir, port)
}

/// Check whether `lock_path` is currently held by a running `--proxy-daemon` instance
/// by probing with `share_mode(0)` without truncating or overwriting the file.
pub fn is_proxy_daemon_lock_held_at(lock_path: &Path) -> bool {
    if !lock_path.exists() {
        return false;
    }
    let mut opts = fs::OpenOptions::new();
    opts.read(true).write(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        opts.share_mode(0);
    }
    match opts.open(lock_path) {
        Ok(probe_handle) => {
            drop(probe_handle);
            false
        }
        Err(_) => true,
    }
}

/// Attempt to open `lock_path` with an exclusive OS lock (`share_mode(0)` on Windows).
/// Retries briefly to ride through sub-millisecond liveness probes, returning `Some(File)`
/// if acquired or `None` if another `--proxy-daemon` instance holds the lock.
pub fn try_acquire_proxy_daemon_lock_at(lock_path: &Path) -> Option<fs::File> {
    if let Some(parent) = lock_path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let mut opts = fs::OpenOptions::new();
    opts.read(true).write(true).create(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        opts.share_mode(0);
    }
    let mut file_opt = None;
    for attempt in 0..4 {
        match opts.open(lock_path) {
            Ok(f) => {
                file_opt = Some(f);
                break;
            }
            Err(_) => {
                if attempt + 1 < 4 {
                    thread::sleep(Duration::from_millis(10));
                }
            }
        }
    }
    let mut file = file_opt?;
    let _ = file.set_len(0);
    let _ = writeln!(file, "{}", std::process::id());
    let _ = file.flush();
    Some(file)
}

/// Attempt to acquire the default `--proxy-daemon` single-instance lockfile for `port`.
pub fn try_acquire_proxy_daemon_lock(port: &str) -> Option<fs::File> {
    let lock_path = default_proxy_daemon_lockfile_path(port);
    try_acquire_proxy_daemon_lock_at(&lock_path)
}

/// Resolve the executable path to use when spawning a detached `--proxy-daemon` background process,
/// preferring `%LOCALAPPDATA%\OpenAI\Codex\custom\codex-9router-subagents.exe` and falling back to
/// `env::current_exe()` only when it is not a renamed `.old.*` file.
pub fn resolve_proxy_daemon_exe_path() -> Option<PathBuf> {
    if let Ok(local_app_data) = env::var("LOCALAPPDATA") {
        let custom_shim = Path::new(&local_app_data)
            .join("OpenAI")
            .join("Codex")
            .join("custom")
            .join("codex-9router-subagents.exe");
        if custom_shim.is_file() {
            return Some(custom_shim);
        }
    }
    if let Ok(cur) = env::current_exe() {
        let is_old = cur
            .file_name()
            .and_then(|s| s.to_str())
            .is_some_and(|n| n.to_ascii_lowercase().contains(".old."));
        if !is_old && cur.is_file() {
            return Some(cur);
        }
    }
    None
}

/// Ensure a detached background `--proxy-daemon` process is running for `port`.
/// If `proxy-daemon-<port>.lock` is not currently held with `share_mode(0)`, spawns a detached `--proxy-daemon`.
pub fn ensure_background_proxy_daemon_running(port: &str) {
    let lock_path = default_proxy_daemon_lockfile_path(port);
    if is_proxy_daemon_lock_held_at(&lock_path) {
        return;
    }

    let Some(daemon_exe) = resolve_proxy_daemon_exe_path() else {
        return;
    };

    let mut cmd = Command::new(daemon_exe);
    cmd.arg("--proxy-daemon")
        .env("CODEX_PROXY_PORT", port)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        cmd.creation_flags(CREATE_NO_WINDOW | DETACHED_PROCESS);
    }
    let _ = cmd.spawn();
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatGptWindowEntry {
    pub desktop: String,
    pub pid: u32,
    pub hwnd: usize,
    pub class_name: String,
    pub visible: bool,
    pub rect: (i32, i32, i32, i32),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatGptDesktopClassification {
    pub default_visible_windows: Vec<ChatGptWindowEntry>,
    pub default_any_pids: Vec<u32>,
    pub hidden_desktop_pids: Vec<u32>,
    pub hidden_desktop_names: Vec<String>,
}

/// Classify enumerated `ChatGPT.exe` top-level windows across Win32 desktops.
/// Identifies visible `Chrome_WidgetWin_1` windows on `Default` vs `Chrome_WidgetWin_*`
/// windows stranded on hidden sandbox desktops (such as `exebox-*`), while ignoring
/// Chromium's internal `sbox_alternate_desktop*` renderer desktops.
pub fn classify_chatgpt_desktop_windows(
    entries: &[ChatGptWindowEntry],
) -> ChatGptDesktopClassification {
    let mut default_visible_windows = Vec::new();
    let mut default_any_pids = Vec::new();
    let mut hidden_candidates: Vec<(u32, String)> = Vec::new();

    for entry in entries {
        if !entry.class_name.starts_with("Chrome_WidgetWin_") {
            continue;
        }
        if entry.desktop.eq_ignore_ascii_case("Default") {
            if !default_any_pids.contains(&entry.pid) {
                default_any_pids.push(entry.pid);
            }
            if entry.class_name == "Chrome_WidgetWin_1" && entry.visible {
                default_visible_windows.push(entry.clone());
            }
        } else if !entry
            .desktop
            .to_ascii_lowercase()
            .starts_with("sbox_alternate_desktop")
        {
            hidden_candidates.push((entry.pid, entry.desktop.clone()));
        }
    }

    let mut hidden_desktop_pids = Vec::new();
    let mut hidden_desktop_names = Vec::new();
    for (pid, dname) in hidden_candidates {
        if !default_any_pids.contains(&pid) {
            if !hidden_desktop_pids.contains(&pid) {
                hidden_desktop_pids.push(pid);
            }
            if !hidden_desktop_names.contains(&dname) {
                hidden_desktop_names.push(dname);
            }
        }
    }

    ChatGptDesktopClassification {
        default_visible_windows,
        default_any_pids,
        hidden_desktop_pids,
        hidden_desktop_names,
    }
}

/// Candidate Electron singleton lockfile paths for unpackaged and MSIX-packaged Codex Desktop.
pub fn codex_singleton_lockfile_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Ok(appdata) = env::var("APPDATA") {
        paths.push(
            PathBuf::from(appdata)
                .join("Codex")
                .join("web")
                .join("Codex")
                .join("lockfile"),
        );
    }
    if let Ok(local_appdata) = env::var("LOCALAPPDATA") {
        let packages_dir = PathBuf::from(&local_appdata).join("Packages");
        if let Ok(entries) = fs::read_dir(&packages_dir) {
            for entry in entries.flatten() {
                if entry
                    .file_name()
                    .to_str()
                    .is_some_and(|n| n.to_ascii_lowercase().starts_with("openai.codex"))
                {
                    let lock = entry
                        .path()
                        .join("LocalCache")
                        .join("Roaming")
                        .join("Codex")
                        .join("web")
                        .join("Codex")
                        .join("lockfile");
                    if !paths.contains(&lock) {
                        paths.push(lock);
                    }
                }
            }
        }
        let default_pkg_lock = packages_dir
            .join("OpenAI.Codex_2p2nqsd0c76g0")
            .join("LocalCache")
            .join("Roaming")
            .join("Codex")
            .join("web")
            .join("Codex")
            .join("lockfile");
        if !paths.contains(&default_pkg_lock) {
            paths.push(default_pkg_lock);
        }
    }
    paths
}

#[cfg(windows)]
mod win_desktop {
    use super::*;
    use std::ffi::OsString;
    use std::os::windows::ffi::{OsStrExt, OsStringExt};

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    pub struct WinRect {
        pub left: i32,
        pub top: i32,
        pub right: i32,
        pub bottom: i32,
    }

    #[repr(C)]
    struct StartupInfoW {
        cb: u32,
        lp_reserved: *mut u16,
        lp_desktop: *mut u16,
        lp_title: *mut u16,
        dw_x: u32,
        dw_y: u32,
        dw_x_size: u32,
        dw_y_size: u32,
        dw_x_count_chars: u32,
        dw_y_count_chars: u32,
        dw_fill_attribute: u32,
        dw_flags: u32,
        w_show_window: u16,
        cb_reserved2: u16,
        lp_reserved2: *mut u8,
        h_std_input: isize,
        h_std_output: isize,
        h_std_error: isize,
    }

    #[repr(C)]
    struct ProcessInformation {
        h_process: isize,
        h_thread: isize,
        dw_process_id: u32,
        dw_thread_id: u32,
    }

    type EnumDesktopsProc = unsafe extern "system" fn(*const u16, isize) -> i32;
    type EnumWindowsProc = unsafe extern "system" fn(isize, isize) -> i32;

    #[link(name = "user32")]
    extern "system" {
        fn GetProcessWindowStation() -> isize;
        fn EnumDesktopsW(hwinsta: isize, lp_enum_func: EnumDesktopsProc, l_param: isize) -> i32;
        fn OpenDesktopW(
            lpsz_desktop: *const u16,
            dw_flags: u32,
            f_inherit: i32,
            dw_desired_access: u32,
        ) -> isize;
        fn CloseDesktop(h_desktop: isize) -> i32;
        fn EnumDesktopWindows(
            h_desktop: isize,
            lpfn: EnumWindowsProc,
            l_param: isize,
        ) -> i32;
        fn GetWindowThreadProcessId(h_wnd: isize, lpdw_process_id: *mut u32) -> u32;
        fn GetClassNameW(h_wnd: isize, lp_class_name: *mut u16, n_max_count: i32) -> i32;
        fn IsWindowVisible(h_wnd: isize) -> i32;
        fn GetWindowRect(h_wnd: isize, lp_rect: *mut WinRect) -> i32;
        fn GetThreadDesktop(dw_thread_id: u32) -> isize;
        fn GetUserObjectInformationW(
            h_obj: isize,
            n_index: i32,
            pv_info: *mut u8,
            n_length: u32,
            lpn_length_needed: *mut u32,
        ) -> i32;
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn GetCurrentThreadId() -> u32;
        fn OpenProcess(dw_desired_access: u32, b_inherit_handle: i32, dw_process_id: u32) -> isize;
        fn CloseHandle(h_object: isize) -> i32;
        fn QueryFullProcessImageNameW(
            h_process: isize,
            dw_flags: u32,
            lp_exe_name: *mut u16,
            lpdw_size: *mut u32,
        ) -> i32;
        fn CreateProcessW(
            lp_application_name: *const u16,
            lp_command_line: *mut u16,
            lp_process_attributes: *const u8,
            lp_thread_attributes: *const u8,
            b_inherit_handles: i32,
            dw_creation_flags: u32,
            lp_environment: *const u8,
            lp_current_directory: *const u16,
            lp_startup_info: *const StartupInfoW,
            lp_process_information: *mut ProcessInformation,
        ) -> i32;
    }

    unsafe extern "system" fn enum_desktops_cb(desktop_ptr: *const u16, l_param: isize) -> i32 {
        if desktop_ptr.is_null() || l_param == 0 {
            return 1;
        }
        let mut len = 0usize;
        while *desktop_ptr.add(len) != 0 {
            len += 1;
        }
        let slice = std::slice::from_raw_parts(desktop_ptr, len);
        let name = OsString::from_wide(slice).to_string_lossy().into_owned();
        let list = &mut *(l_param as *mut Vec<String>);
        list.push(name);
        1
    }

    struct EnumWinContext {
        desktop_name: String,
        entries: Vec<ChatGptWindowEntry>,
    }

    fn is_chatgpt_pid(pid: u32) -> bool {
        if pid == 0 {
            return false;
        }
        unsafe {
            let h_proc = OpenProcess(0x1000, 0, pid);
            if h_proc == 0 {
                return false;
            }
            let mut buf = [0u16; 512];
            let mut size = buf.len() as u32;
            let ok = QueryFullProcessImageNameW(h_proc, 0, buf.as_mut_ptr(), &mut size);
            CloseHandle(h_proc);
            if ok == 0 || size == 0 {
                return false;
            }
            let path_str = OsString::from_wide(&buf[..size as usize])
                .to_string_lossy()
                .to_ascii_lowercase();
            Path::new(&path_str)
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("chatgpt"))
        }
    }

    unsafe extern "system" fn enum_windows_cb(h_wnd: isize, l_param: isize) -> i32 {
        if l_param == 0 {
            return 1;
        }
        let mut cls_buf = [0u16; 256];
        let cls_len = GetClassNameW(h_wnd, cls_buf.as_mut_ptr(), cls_buf.len() as i32);
        if cls_len <= 0 {
            return 1;
        }
        let cls_name = OsString::from_wide(&cls_buf[..cls_len as usize])
            .to_string_lossy()
            .into_owned();
        if !cls_name.starts_with("Chrome_WidgetWin_") {
            return 1;
        }
        let mut pid: u32 = 0;
        GetWindowThreadProcessId(h_wnd, &mut pid);
        if !is_chatgpt_pid(pid) {
            return 1;
        }
        let visible = IsWindowVisible(h_wnd) != 0;
        let mut rc = WinRect::default();
        let _ = GetWindowRect(h_wnd, &mut rc);
        let ctx = &mut *(l_param as *mut EnumWinContext);
        ctx.entries.push(ChatGptWindowEntry {
            desktop: ctx.desktop_name.clone(),
            pid,
            hwnd: h_wnd as usize,
            class_name: cls_name,
            visible,
            rect: (rc.left, rc.top, rc.right, rc.bottom),
        });
        1
    }

    pub fn enumerate_chatgpt_desktop_windows() -> Vec<ChatGptWindowEntry> {
        let mut desktops: Vec<String> = Vec::new();
        unsafe {
            let hwinsta = GetProcessWindowStation();
            if hwinsta == 0 {
                return Vec::new();
            }
            let _ = EnumDesktopsW(
                hwinsta,
                enum_desktops_cb,
                (&mut desktops as *mut Vec<String>) as isize,
            );
        }

        let mut all_entries = Vec::new();
        for d_name in desktops {
            let wide: Vec<u16> = std::ffi::OsStr::new(&d_name)
                .encode_wide()
                .chain(std::iter::once(0))
                .collect();
            unsafe {
                let h_desk = OpenDesktopW(wide.as_ptr(), 0, 0, 0x01FF);
                if h_desk == 0 {
                    continue;
                }
                let mut ctx = EnumWinContext {
                    desktop_name: d_name,
                    entries: Vec::new(),
                };
                let _ = EnumDesktopWindows(
                    h_desk,
                    enum_windows_cb,
                    (&mut ctx as *mut EnumWinContext) as isize,
                );
                let _ = CloseDesktop(h_desk);
                all_entries.extend(ctx.entries);
            }
        }
        all_entries
    }

    pub fn current_thread_desktop_name() -> Option<String> {
        unsafe {
            let h_desk = GetThreadDesktop(GetCurrentThreadId());
            if h_desk == 0 {
                return None;
            }
            let mut buf = [0u8; 512];
            let mut needed: u32 = 0;
            // UOI_NAME = 2
            if GetUserObjectInformationW(h_desk, 2, buf.as_mut_ptr(), buf.len() as u32, &mut needed)
                == 0
            {
                return None;
            }
            let u16_len = (needed as usize / 2).saturating_sub(1);
            let wide = std::slice::from_raw_parts(buf.as_ptr() as *const u16, u16_len);
            Some(OsString::from_wide(wide).to_string_lossy().into_owned())
        }
    }

    pub fn launch_codex_on_default_desktop() -> bool {
        let mut desktop_wide: Vec<u16> = std::ffi::OsStr::new("WinSta0\\Default")
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let family_name = resolve_codex_package_family_name();
        let cmd_str = format!("explorer.exe shell:AppsFolder\\{}!App", family_name);
        let mut cmd_wide: Vec<u16> = std::ffi::OsStr::new(&cmd_str)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();

        unsafe {
            let mut si: StartupInfoW = std::mem::zeroed();
            si.cb = std::mem::size_of::<StartupInfoW>() as u32;
            si.lp_desktop = desktop_wide.as_mut_ptr();
            let mut pi: ProcessInformation = std::mem::zeroed();

            let ok = CreateProcessW(
                std::ptr::null(),
                cmd_wide.as_mut_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                0,
                0,
                std::ptr::null(),
                std::ptr::null(),
                &si,
                &mut pi,
            );
            if ok != 0 {
                if pi.h_process != 0 {
                    CloseHandle(pi.h_process);
                }
                if pi.h_thread != 0 {
                    CloseHandle(pi.h_thread);
                }
                true
            } else {
                false
            }
        }
    }

    pub fn heal_hidden_desktop_codex(
        relaunch_if_no_default: bool,
    ) -> (ChatGptDesktopClassification, bool) {
        let mut entries = enumerate_chatgpt_desktop_windows();
        let mut class = classify_chatgpt_desktop_windows(&entries);

        // If app-server was spawned from a hidden exebox-* desktop before ChatGPT.exe's window finished registering,
        // wait briefly and re-check once.
        if relaunch_if_no_default
            && class.hidden_desktop_pids.is_empty()
            && class.default_visible_windows.is_empty()
        {
            if let Some(cur_desk) = current_thread_desktop_name() {
                if !cur_desk.eq_ignore_ascii_case("Default")
                    && !cur_desk
                        .to_ascii_lowercase()
                        .starts_with("sbox_alternate_desktop")
                {
                    thread::sleep(Duration::from_millis(250));
                    entries = enumerate_chatgpt_desktop_windows();
                    class = classify_chatgpt_desktop_windows(&entries);
                }
            }
        }

        let mut healed = false;
        if !class.hidden_desktop_pids.is_empty() {
            healed = true;
            if class.default_visible_windows.is_empty() {
                let _ = Command::new("taskkill")
                    .args(["/F", "/IM", "ChatGPT.exe", "/T"])
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status();
                let _ = Command::new("taskkill")
                    .args(["/F", "/IM", "codex-computer-use-swift.exe", "/T"])
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status();
                thread::sleep(Duration::from_millis(400));
                for lockfile in codex_singleton_lockfile_paths() {
                    if lockfile.exists() {
                        let _ = fs::remove_file(&lockfile);
                    }
                }
                if relaunch_if_no_default {
                    let stamp_path = env::temp_dir().join("codex-9router-desktop-heal.stamp");
                    let allow_relaunch = stamp_path
                        .metadata()
                        .and_then(|m| m.modified())
                        .ok()
                        .and_then(|t| t.elapsed().ok())
                        .is_none_or(|elapsed| elapsed > Duration::from_secs(10));
                    if allow_relaunch {
                        let _ = fs::write(&stamp_path, b"healed");
                        let _ = launch_codex_on_default_desktop();
                    }
                    std::process::exit(0);
                }
            } else {
                for pid in &class.hidden_desktop_pids {
                    let _ = Command::new("taskkill")
                        .args(["/F", "/PID", &pid.to_string(), "/T"])
                        .stdout(Stdio::null())
                        .stderr(Stdio::null())
                        .status();
                }
            }
        } else if class.default_any_pids.is_empty() {
            // If no ChatGPT.exe owns any desktop window, remove stale lockfiles if they are unlocked.
            for lockfile in codex_singleton_lockfile_paths() {
                if lockfile.exists() {
                    let _ = fs::remove_file(&lockfile);
                }
            }
        }

        (class, healed)
    }
}

/// Run full system health diagnostics and print report.
pub fn run_doctor() {
    println!("===================================================================");
    println!("           Codex 9Router Proxy - Diagnostics Doctor 🩺             ");
    println!("===================================================================");
    println!();

    let _ = sync_custom_codex_binaries();

    #[cfg(windows)]
    {
        let (class, healed) = win_desktop::heal_hidden_desktop_codex(false);
        if healed {
            println!(
                "[WARN] Codex Desktop GUI : Terminated hidden-desktop ChatGPT.exe (PIDs {:?} on {:?})",
                class.hidden_desktop_pids, class.hidden_desktop_names
            );
        }
        let refreshed = classify_chatgpt_desktop_windows(&win_desktop::enumerate_chatgpt_desktop_windows());
        if let Some(win) = refreshed.default_visible_windows.first() {
            println!(
                "[OK] Codex Desktop GUI  : Visible on WinSta0\\Default (PID {}, HWND 0x{:X}, Rect {},{}-{},{})",
                win.pid, win.hwnd, win.rect.0, win.rect.1, win.rect.2, win.rect.3
            );
        } else {
            println!(
                "[INFO] Codex Desktop GUI : No active window on WinSta0\\Default (singleton lockfiles clean)"
            );
        }
    }

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

    if let Ok(local_app_data) = env::var("LOCALAPPDATA") {
        let custom_shim = Path::new(&local_app_data)
            .join("OpenAI")
            .join("Codex")
            .join("custom")
            .join("codex-9router-subagents.exe");
        if custom_shim.is_file() {
            let size = custom_shim.metadata().map_or(0, |m| m.len());
            println!(
                "[OK] Custom Shim Binary : {} ({} bytes)",
                custom_shim.display(),
                size
            );
        } else {
            println!(
                "[WARN] Custom Shim      : Not found at {}",
                custom_shim.display()
            );
        }
    }

    let proc_cli_path = env::var("CODEX_CLI_PATH")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let user_cli_path = get_user_env_var("CODEX_CLI_PATH")
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    match (proc_cli_path.as_deref(), user_cli_path.as_deref()) {
        (Some(val), Some(uval)) => {
            let p = PathBuf::from(val);
            let up = PathBuf::from(uval);
            if p.is_file() && up.is_file() {
                println!("[OK] CODEX_CLI_PATH     : {} (Verified)", p.display());
            } else {
                println!(
                    "[WARN] CODEX_CLI_PATH   : {} (Target file missing)",
                    p.display()
                );
            }
        }
        (None, Some(uval)) => {
            let up = PathBuf::from(uval);
            if up.is_file() {
                println!(
                    "[OK] CODEX_CLI_PATH     : {} (Verified in User Environment)",
                    up.display()
                );
            } else {
                println!(
                    "[WARN] CODEX_CLI_PATH   : {} (Target file missing)",
                    up.display()
                );
            }
        }
        (Some(val), None) => {
            let p = PathBuf::from(val);
            if p.is_file() {
                if cfg!(windows) {
                    println!(
                        "[WARN] CODEX_CLI_PATH   : {} (Set in process only; missing in User Environment)",
                        p.display()
                    );
                } else {
                    println!("[OK] CODEX_CLI_PATH     : {} (Verified)", p.display());
                }
            } else {
                println!(
                    "[WARN] CODEX_CLI_PATH   : {} (Target file missing)",
                    p.display()
                );
            }
        }
        (None, None) => {
            println!("[WARN] CODEX_CLI_PATH   : Not set in process or User environment");
        }
    }

    let provider = get_target_model_provider();
    println!("[OK] Subagent Provider  : {}", provider);
    let key_set = get_subagent_auth_header().is_some();
    if key_set {
        println!("[OK] Provider API Key   : [CONFIGURED / MASKED]");
    } else {
        println!("[INFO] NINEROUTER_KEY   : Not set in process environment");
    }

    let (cert_path, key_path) = default_cert_paths();
    let tls_status = match create_tls_acceptor(&cert_path, &key_path) {
        Ok(_) => format!("Ready ({})", cert_path.display()),
        Err(e) => format!("Error ({})", e),
    };
    println!("[OK] Loopback TLS Cert  : {}", tls_status);

    let proxy_port = get_proxy_port();
    let proxy_addr = format!("127.0.0.1:{}", proxy_port);
    let proxy_status = match TcpListener::bind(&proxy_addr) {
        Ok(_) => "Port available / ready to bind",
        Err(_) => {
            if verify_existing_tls_proxy_listener(&proxy_addr, &cert_path) {
                "Port active / verified codex-9router-proxy TLS listener"
            } else {
                "Port occupied by another process (will auto-fallback to 20130..20139)"
            }
        }
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

    match sync_codex_models_cache() {
        Some(cache_path) => {
            println!("[OK] Models Cache Sync  : Synchronized ({})", cache_path.display());
        }
        None => {
            println!(
                "[INFO] Models Cache     : Not present on disk (will enrich via GET /backend-api/models)"
            );
        }
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

/// Strip any existing `chatgpt_base_url` override from CLI arguments before `--` and insert the HTTPS loopback URL
/// before any `--` argument separator so positional arguments after `--` are never modified or polluted.
pub fn inject_loopback_base_url(args: &[String], port: &str) -> Vec<String> {
    let sep_pos = args.iter().position(|a| a == "--");
    let pre_args = match sep_pos {
        Some(idx) => &args[..idx],
        None => args,
    };
    let post_args = match sep_pos {
        Some(idx) => &args[idx..],
        None => &[],
    };

    let mut cleaned_pre = Vec::with_capacity(pre_args.len() + 2);
    let mut skip_next = false;
    for (i, a) in pre_args.iter().enumerate() {
        if skip_next {
            skip_next = false;
            continue;
        }
        if (a == "-c" || a == "--config")
            && i + 1 < pre_args.len()
            && pre_args[i + 1].contains("chatgpt_base_url")
        {
            skip_next = true;
            continue;
        }
        if (a.starts_with("-c=") || a.starts_with("--config=")) && a.contains("chatgpt_base_url") {
            continue;
        }
        cleaned_pre.push(a.clone());
    }

    let base_url_arg = format!(
        "chatgpt_base_url=\"https://127.0.0.1:{}/backend-api/\"",
        port
    );

    cleaned_pre.push("-c".to_string());
    cleaned_pre.push(base_url_arg);
    cleaned_pre.extend_from_slice(post_args);
    cleaned_pre
}

/// Determine whether CLI arguments represent a non-API / lightweight command (e.g. `--version`, `--help`,
/// `login`, `logout`, `sandbox`, `stdio-to-uds`, `mcp`, `plugin`, `update`, etc.)
/// that should not spawn the embedded reverse proxy or inject `-c chatgpt_base_url=...`.
/// Interactive `codex` invocations (`args.is_empty()`) are NOT lightweight and MUST receive proxy injection.
pub fn is_lightweight_cli_invocation(args: &[String]) -> bool {
    let pre_sep: Vec<&str> = args
        .iter()
        .take_while(|a| a.as_str() != "--")
        .map(|s| s.as_str())
        .collect();

    if pre_sep.iter().any(|&a| {
        a == "--version" || a == "-V" || a == "--help" || a == "-h" || a == "help"
    }) {
        return true;
    }

    let mut first_subcommand: Option<&str> = None;
    let mut skip_next = false;
    for (i, &a) in pre_sep.iter().enumerate() {
        if skip_next {
            skip_next = false;
            continue;
        }
        if matches!(
            a,
            "-c" | "--config"
                | "-m"
                | "--model"
                | "-p"
                | "--profile"
                | "-C"
                | "--cd"
                | "-s"
                | "--sandbox"
                | "-a"
                | "--ask-for-approval"
                | "--enable"
                | "--disable"
        ) {
            if i + 1 < pre_sep.len() {
                skip_next = true;
            }
            continue;
        }
        if a.starts_with('-') {
            continue;
        }
        first_subcommand = Some(a);
        break;
    }

    if let Some(sub) = first_subcommand {
        if matches!(
            sub,
            "version"
                | "login"
                | "logout"
                | "completion"
                | "features"
                | "sandbox"
                | "stdio-to-uds"
                | "mcp"
                | "plugin"
                | "update"
                | "debug"
                | "apply"
                | "archive"
                | "unarchive"
                | "delete"
                | "migrate-rollouts"
                | "app"
                | "generate-ts"
                | "generate-json-schema"
        ) {
            return true;
        }
    }
    if pre_sep.contains(&"daemon")
        && pre_sep
            .iter()
            .any(|&a| matches!(a, "stop" | "status" | "version"))
    {
        return true;
    }
    false
}

/// Check whether CLI arguments invoke the built-in diagnostics doctor (`--doctor`, `doctor`, `-doctor`, `--proxy-status`).
/// Only matches when it is the first argument so `codex exec "doctor"` is never hijacked.
pub fn is_doctor_cli_invocation(args: &[String]) -> bool {
    let Some(first) = args.first().map(|s| s.as_str()) else {
        return false;
    };
    first.eq_ignore_ascii_case("--doctor")
        || first.eq_ignore_ascii_case("doctor")
        || first.eq_ignore_ascii_case("-doctor")
        || first.eq_ignore_ascii_case("--proxy-status")
}

fn main() -> io::Result<()> {
    let args: Vec<String> = env::args().skip(1).collect();

    if is_doctor_cli_invocation(&args) {
        run_doctor();
        return Ok(());
    }

    if args.iter().any(|a| a.eq_ignore_ascii_case("--proxy-daemon")) {
        let port = get_proxy_port();
        let Some(_daemon_lock) = try_acquire_proxy_daemon_lock(&port) else {
            return Ok(());
        };
        let bind_addr = format!("127.0.0.1:{}", port);
        let _ = spawn_embedded_reverse_proxy_with_port(&bind_addr)?;
        let _ = sync_custom_codex_binaries();
        let _ = sync_codex_models_cache();
        loop {
            thread::park();
        }
    }

    let _ = sync_custom_codex_binaries();
    let _ = sync_codex_models_cache();

    let real_codex = find_real_codex();
    let is_lightweight_command = is_lightweight_cli_invocation(&args);

    let is_app_server_subcommand = args.iter().any(|a| {
        matches!(
            a.as_str(),
            "daemon" | "proxy" | "generate-ts" | "generate-json-schema"
        )
    });
    let is_app_server = args.iter().any(|a| a == "app-server")
        && !is_app_server_subcommand
        && !is_lightweight_command;

    if !is_app_server {
        let mut forward_args = args.clone();
        let mut cmd = Command::new(&real_codex);

        if !is_lightweight_command {
            let port = get_proxy_port();
            let bind_addr = format!("127.0.0.1:{}", port);
            if let Ok((cert_path, actual_port)) = spawn_embedded_reverse_proxy_with_port(&bind_addr)
            {
                ensure_background_proxy_daemon_running(&actual_port);
                cmd.env("CODEX_CA_CERTIFICATE", &cert_path);
                forward_args = inject_loopback_base_url(&forward_args, &actual_port);
            }
        }

        let status = cmd.args(&forward_args).status()?;
        std::process::exit(status.code().unwrap_or(1));
    }

    #[cfg(windows)]
    {
        let _ = win_desktop::heal_hidden_desktop_codex(true);
    }

    let port = get_proxy_port();
    let bind_addr = format!("127.0.0.1:{}", port);
    let (default_cert, _) = default_cert_paths();
    let (cert_path, actual_port) = match spawn_embedded_reverse_proxy_with_port(&bind_addr) {
        Ok(pair) => pair,
        Err(e) => {
            eprintln!(
                "[codex-9router-proxy] Reverse proxy note on {}: {}",
                bind_addr, e
            );
            (default_cert, port)
        }
    };
    ensure_background_proxy_daemon_running(&actual_port);

    let child_args = inject_loopback_base_url(&args, &actual_port);

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

                if let Some(
                    "thread/start"
                    | "thread/fork"
                    | "thread/resume"
                    | "thread/settings/update"
                    | "turn/start",
                ) = json.get("method").and_then(|m| m.as_str())
                {
                    if let Some(params) = json.get_mut("params").and_then(|p| p.as_object_mut()) {
                        if route_thread_params(params) {
                            modified = true;
                        }
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
model = "9router-subagent"
model_provider = "9router"
model_reasoning_effort = "high"
"#;
        assert_eq!(
            parse_model_from_role_toml(toml),
            Some("9router-subagent".to_string())
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
    fn test_builtin_role_model_defaults_all_to_9router_subagent() {
        assert_eq!(builtin_role_model("worker"), "9router-subagent");
        assert_eq!(builtin_role_model("explorer"), "9router-subagent");
        assert_eq!(builtin_role_model("reviewer"), "9router-subagent");
        assert_eq!(builtin_role_model("default"), "9router-subagent");
    }

    #[test]
    fn test_is_subagent_role_name_excludes_message_roles() {
        assert!(!is_subagent_role_name("developer"));
        assert!(!is_subagent_role_name("user"));
        assert!(!is_subagent_role_name("assistant"));
        assert!(!is_subagent_role_name("system"));
        assert!(!is_subagent_role_name("tool"));
        assert!(!is_subagent_role_name("function"));
        assert!(!is_subagent_role_name("main"));
        assert!(!is_subagent_role_name("primary"));
        assert!(!is_subagent_role_name("root"));
        assert!(!is_subagent_role_name("/root"));
        assert!(is_subagent_role_name("worker"));
        assert!(is_subagent_role_name("explorer"));
        assert!(is_subagent_role_name("reviewer"));
        assert!(is_subagent_role_name("default"));
    }

    #[test]
    fn test_main_agent_with_developer_role_and_additional_tools_not_hijacked() {
        let body = serde_json::json!({
            "model": "gpt-6-luna",
            "tools": [],
            "input": [
                {
                    "id": "fc_123",
                    "role": "developer",
                    "type": "additional_tools",
                    "tools": [
                        {
                            "type": "namespace",
                            "name": "functions",
                            "tools": [
                                {"type": "function", "name": "exec"}
                            ]
                        },
                        {
                            "type": "namespace",
                            "name": "collaboration",
                            "tools": [
                                {"type": "function", "name": "spawn_agent"}
                            ]
                        }
                    ]
                },
                {
                    "role": "developer",
                    "content": "System instructions"
                },
                {
                    "role": "user",
                    "content": "Run a PowerShell check and spawn 3 subagents"
                }
            ],
            "client_metadata": {
                "x-codex-turn-metadata": "{\"turn_id\":\"t1\",\"thread_source\":\"user\",\"agent_name\":\"/root\"}"
            }
        });
        let body_bytes = serde_json::to_vec(&body).unwrap();
        let headers = HeaderMap::new();
        let (is_subagent, routed_body) = inspect_and_route_http_request_with_headers(
            "/backend-api/codex/responses",
            &headers,
            &body_bytes,
        );
        assert!(
            !is_subagent,
            "Main agent gpt-6-luna turn with developer role in input must NOT be classified as subagent!"
        );
        let parsed: Value = serde_json::from_slice(&routed_body).unwrap();
        assert_eq!(parsed["model"], "gpt-6-luna");
    }

    #[test]
    fn test_subagent_detected_via_http_headers_and_client_metadata() {
        let body = serde_json::json!({
            "model": "gpt-6-luna",
            "input": [
                {"role": "developer", "content": "You are a subagent"},
                {"role": "user", "content": "Inspect Cargo.toml"}
            ],
            "client_metadata": {
                "x-openai-subagent": "collab_spawn",
                "x-codex-parent-thread-id": "019ac141-c9fd-7780-95b7-67f988bc83b6",
                "x-codex-turn-metadata": "{\"thread_source\":\"subagent\",\"subagent_kind\":\"thread_spawn\"}"
            }
        });
        let body_bytes = serde_json::to_vec(&body).unwrap();
        let mut headers = HeaderMap::new();
        headers.insert(
            header::HeaderName::from_static("x-openai-subagent"),
            HeaderValue::from_static("collab_spawn"),
        );

        let (is_subagent, routed_body) = inspect_and_route_http_request_with_headers(
            "/backend-api/codex/responses",
            &headers,
            &body_bytes,
        );
        assert!(is_subagent);
        let parsed: Value = serde_json::from_slice(&routed_body).unwrap();
        assert_ne!(parsed["model"], "gpt-6-luna");
    }

    #[test]
    fn test_sanitize_subagent_tools_for_9router() {
        let body = serde_json::json!({
            "model": "9router-subagent",
            "input": [
                {
                    "role": "developer",
                    "type": "additional_tools",
                    "tools": [{"type": "namespace", "name": "functions"}]
                },
                {
                    "role": "user",
                    "content": "Edit file"
                }
            ],
            "tools": [
                {
                    "type": "function",
                    "name": "exec_command",
                    "parameters": {"type": "object", "properties": {"cmd": {"type": "string"}}}
                },
                {
                    "type": "function",
                    "name": "apply_patch",
                    "parameters": {"type": "object", "properties": {"input": {"type": "string"}}}
                },
                {
                    "type": "custom",
                    "name": "apply_patch",
                    "description": "Freeform patch"
                },
                {
                    "type": "namespace",
                    "name": "mcp__codegraph",
                    "tools": []
                },
                {
                    "type": "web_search"
                }
            ]
        });
        let body_bytes = serde_json::to_vec(&body).unwrap();
        let (is_subagent, routed_body) =
            inspect_and_route_http_request("/backend-api/codex/responses", &body_bytes);
        assert!(is_subagent);
        let parsed: Value = serde_json::from_slice(&routed_body).unwrap();

        // additional_tools stripped from input
        let input_arr = parsed["input"].as_array().unwrap();
        assert_eq!(input_arr.len(), 1);
        assert_eq!(input_arr[0]["role"], "user");

        // tools array contains only function tools (exec_command and function apply_patch)
        let tools_arr = parsed["tools"].as_array().unwrap();
        assert_eq!(tools_arr.len(), 2);
        assert_eq!(tools_arr[0]["name"], "exec_command");
        assert_eq!(tools_arr[1]["name"], "apply_patch");
        assert_eq!(tools_arr[1]["type"], "function");
    }

    #[test]
    fn test_inject_subagent_models_metadata() {
        let upstream_models = serde_json::json!({
            "models": [
                {
                    "slug": "gpt-5-mini",
                    "display_name": "GPT-5 Mini",
                    "context_window": 128000,
                    "max_context_window": 128000,
                    "comp_hash": "1000"
                },
                {
                    "slug": "gpt-6-luna",
                    "display_name": "GPT-6 Luna",
                    "context_window": 272000,
                    "max_context_window": 872000,
                    "effective_context_window_percent": 95,
                    "comp_hash": "3000",
                    "luna_marker": "from_luna_template",
                    "apply_patch_tool_type": "freeform",
                    "visibility": "list",
                    "supported_in_api": true,
                    "experimental_supported_tools": ["code_mode"]
                },
                {
                    "slug": "9router-subagent",
                    "display_name": "Stale Cached Subagent",
                    "context_window": 200000,
                    "max_context_window": 200000,
                    "comp_hash": "stale_hash",
                    "apply_patch_tool_type": "freeform",
                    "experimental_supported_tools": ["code_mode"]
                }
            ]
        });
        let raw = serde_json::to_vec(&upstream_models).unwrap();
        let enriched = inject_subagent_models_metadata(&raw);
        let parsed: Value = serde_json::from_slice(&enriched).unwrap();
        let arr = parsed["models"].as_array().unwrap();

        let subagent_matches: Vec<&Value> = arr
            .iter()
            .filter(|m| m["slug"] == "9router-subagent")
            .collect();
        assert_eq!(
            subagent_matches.len(),
            1,
            "existing 9router-subagent entry should be updated in place without duplication"
        );
        let subagent_entry = subagent_matches[0];
        assert_eq!(subagent_entry["context_window"], 872000);
        assert_eq!(subagent_entry["max_context_window"], 872000);
        assert_eq!(subagent_entry["effective_context_window_percent"], 95);
        assert_eq!(subagent_entry["comp_hash"], "3000");
        assert_eq!(subagent_entry["luna_marker"], "from_luna_template");
        assert_eq!(subagent_entry["apply_patch_tool_type"], "function");
        assert_eq!(
            subagent_entry["experimental_supported_tools"]
                .as_array()
                .unwrap()
                .len(),
            0
        );

        let implement_entry = arr
            .iter()
            .find(|m| m["slug"] == "implement")
            .expect("implement should be injected using gpt-6-luna template");
        assert_eq!(implement_entry["context_window"], 872000);
        assert_eq!(implement_entry["max_context_window"], 872000);
        assert_eq!(implement_entry["effective_context_window_percent"], 95);
        assert_eq!(implement_entry["comp_hash"], "3000");
        assert_eq!(implement_entry["luna_marker"], "from_luna_template");
    }

    #[test]
    fn test_sanitize_backend_origin_rewrites_loopback_to_chatgpt() {
        let mut account_read_result = serde_json::json!({
            "account": {
                "type": "chatgpt",
                "planType": "plus"
            },
            "workspaceRouting": {
                "chatgptAccountId": "123",
                "backendOrigin": "https://127.0.0.1:20129",
                "accountRoutingOverride": "NO_CONSTRAINT"
            }
        });
        let modded = sanitize_rate_limits(&mut account_read_result);
        assert!(modded);
        assert_eq!(
            account_read_result["workspaceRouting"]["backendOrigin"],
            "https://chatgpt.com"
        );
    }

    #[test]
    fn test_route_worker_role() {
        let mut params = serde_json::Map::new();
        params.insert("agentRole".to_string(), Value::String("worker".to_string()));
        let modded = route_thread_params(&mut params);
        assert!(modded);
        assert!(params.contains_key("model"));
        assert_eq!(params.get("modelProvider").unwrap(), "9router");
        assert_eq!(params.get("model_provider").unwrap(), "9router");
    }

    #[test]
    fn test_route_explorer_role() {
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
    fn test_inspect_and_route_worker_role_rewrites_model() {
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
    fn test_inspect_and_route_explorer_role_rewrites_model() {
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
    fn test_inspect_and_route_reviewer_role_rewrites_model() {
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

    #[test]
    fn test_sanitize_subagent_normalizes_agent_message_and_developer_role() {
        let body = serde_json::json!({
            "model": "9router-subagent",
            "instructions": "You are Codex, a coding agent.",
            "input": [
                {
                    "type": "message",
                    "role": "developer",
                    "content": [{"type": "input_text", "text": "<permissions instructions>"}],
                    "internal_chat_message_metadata_passthrough": "meta_dev"
                },
                {
                    "type": "message",
                    "role": "user",
                    "content": [{"type": "input_text", "text": "<environment_context>"}],
                    "internal_chat_message_metadata_passthrough": "meta_user"
                },
                {
                    "role": "developer",
                    "content": "You are an explorer sub-agent."
                },
                {
                    "type": "agent_message",
                    "content": [
                        {
                            "type": "input_text",
                            "text": "Inspect Cargo.toml and summarize dependencies"
                        }
                    ],
                    "author": {
                        "agent_id": "019ac141-c9fd-7780-95b7-67f988bc83b6",
                        "agent_path": "/root"
                    },
                    "recipient": {
                        "agent_id": "019ac142-1111-2222-3333-444455556666",
                        "agent_path": "/root/explorer_1"
                    },
                    "internal_chat_message_metadata_passthrough": "meta_agent_msg"
                },
                {
                    "type": "agent_message",
                    "role": "agent",
                    "content": [{"type": "input_text", "text": "Agent role message"}]
                },
                {
                    "type": "agent_message",
                    "role": "  ",
                    "content": [{"type": "input_text", "text": "Empty role message"}]
                },
                {
                    "type": "agent_message",
                    "role": "developer",
                    "content": [{"type": "input_text", "text": "Developer agent_message"}]
                },
                {
                    "type": "agent_message",
                    "role": "assistant",
                    "content": [{"type": "output_text", "text": "Prior assistant reply"}]
                }
            ],
            "messages": [
                {
                    "role": "developer",
                    "content": "Chat completions developer prompt"
                }
            ]
        });

        let body_bytes = serde_json::to_vec(&body).unwrap();
        let (is_subagent, routed_body) =
            inspect_and_route_http_request("/backend-api/codex/responses", &body_bytes);
        assert!(is_subagent);

        let parsed: Value = serde_json::from_slice(&routed_body).unwrap();
        let input_arr = parsed["input"].as_array().unwrap();
        assert_eq!(input_arr.len(), 8);

        // 1. First developer message normalized to system and metadata stripped
        assert_eq!(input_arr[0]["type"], "message");
        assert_eq!(input_arr[0]["role"], "system");
        assert_eq!(
            input_arr[0]["content"][0]["text"],
            "<permissions instructions>"
        );
        assert!(input_arr[0]
            .get("internal_chat_message_metadata_passthrough")
            .is_none());

        // 2. Environment context user message preserved and V2 passthrough metadata stripped
        assert_eq!(input_arr[1]["type"], "message");
        assert_eq!(input_arr[1]["role"], "user");
        assert!(input_arr[1]
            .get("internal_chat_message_metadata_passthrough")
            .is_none());

        // 3. Role-only developer message normalized to system
        assert_eq!(input_arr[2]["role"], "system");
        assert_eq!(input_arr[2]["content"], "You are an explorer sub-agent.");

        // 4. Multi-Agents V2 agent_message normalized to message with role=user and V2 metadata stripped
        assert_eq!(input_arr[3]["type"], "message");
        assert_eq!(input_arr[3]["role"], "user");
        assert_eq!(
            input_arr[3]["content"][0]["text"],
            "Inspect Cargo.toml and summarize dependencies"
        );
        assert!(input_arr[3].get("author").is_none());
        assert!(input_arr[3].get("recipient").is_none());
        assert!(input_arr[3]
            .get("internal_chat_message_metadata_passthrough")
            .is_none());

        // 5. agent_message with role="agent" or whitespace normalized to role="user"
        assert_eq!(input_arr[4]["type"], "message");
        assert_eq!(input_arr[4]["role"], "user");
        assert_eq!(input_arr[5]["type"], "message");
        assert_eq!(input_arr[5]["role"], "user");

        // 6. agent_message with role="developer" normalized to role="system"
        assert_eq!(input_arr[6]["type"], "message");
        assert_eq!(input_arr[6]["role"], "system");

        // 7. agent_message with role="assistant" preserved as role="assistant"
        assert_eq!(input_arr[7]["type"], "message");
        assert_eq!(input_arr[7]["role"], "assistant");

        // 8. Chat completions fallback messages array developer role normalized to system
        let messages_arr = parsed["messages"].as_array().unwrap();
        assert_eq!(messages_arr[0]["role"], "system");
    }

    #[test]
    fn test_x_openai_subagent_header_preserves_role_name() {
        let mut headers = HeaderMap::new();
        headers.insert("x-openai-subagent", HeaderValue::from_static("explorer"));
        let body = serde_json::to_vec(&serde_json::json!({
            "model": "gpt-6-luna",
            "input": [
                {
                    "type": "agent_message",
                    "content": [{"type": "input_text", "text": "Explore repo"}]
                }
            ]
        }))
        .unwrap();

        let (is_subagent, routed_body) = inspect_and_route_http_request_with_headers(
            "/backend-api/codex/responses",
            &headers,
            &body,
        );
        assert!(is_subagent);
        let parsed: Value = serde_json::from_slice(&routed_body).unwrap();
        assert_eq!(parsed["model"], map_role_to_model(Some("explorer")));
        assert_eq!(parsed["input"][0]["type"], "message");
        assert_eq!(parsed["input"][0]["role"], "user");
    }

    #[derive(Debug)]
    struct TestNestedError {
        msg: String,
        source: Option<Box<dyn std::error::Error + Send + Sync>>,
    }

    impl std::fmt::Display for TestNestedError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "{}", self.msg)
        }
    }

    impl std::error::Error for TestNestedError {
        fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
            self.source
                .as_ref()
                .map(|b| b.as_ref() as &(dyn std::error::Error + 'static))
        }
    }

    #[test]
    fn test_format_error_chain_walks_full_source_hierarchy() {
        let leaf = io::Error::from_raw_os_error(11001);
        let mid = TestNestedError {
            msg: "dns error".to_string(),
            source: Some(Box::new(leaf)),
        };
        let outer = TestNestedError {
            msg: "client error (Connect)".to_string(),
            source: Some(Box::new(mid)),
        };
        let top = TestNestedError {
            msg: "error sending request for url (https://chatgpt.com/backend-api/codex/responses)"
                .to_string(),
            source: Some(Box::new(outer)),
        };

        let formatted = format_error_chain(&top);
        assert!(formatted.contains("error sending request for url"));
        assert!(formatted.contains("client error (Connect)"));
        assert!(formatted.contains("dns error"));
        assert!(formatted.contains("11001"));
    }

    #[tokio::test]
    async fn test_format_reqwest_upstream_error_includes_source_chain_and_hints() {
        // Bind and immediately drop a TcpListener to obtain an unused ephemeral port
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let closed_addr = listener.local_addr().unwrap();
        drop(listener);

        let url = format!("http://{}/backend-api/codex/responses", closed_addr);
        let client = build_upstream_http_client();
        let err = client.get(&url).send().await.unwrap_err();

        let formatted = format_reqwest_upstream_error(&url, &err);
        assert!(
            formatted.contains("Codex 9Router Proxy upstream connection error to"),
            "unexpected formatted error: {}",
            formatted
        );
        assert!(
            formatted.contains(" -> "),
            "expected full source chain separator ' -> ' in: {}",
            formatted
        );
        assert!(
            formatted.contains("[connect]") || formatted.contains("[timeout]"),
            "expected classification tag in: {}",
            formatted
        );
        assert!(
            formatted.contains("[hint:"),
            "expected diagnostic hint in: {}",
            formatted
        );
    }

    #[tokio::test]
    async fn test_send_upstream_with_retry_recovers_on_second_attempt() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let server_task = tokio::spawn(async move {
            // 1st attempt: accept and immediately drop socket after reading a byte to simulate stale socket reset/EOF
            if let Ok((mut stream, _)) = listener.accept().await {
                let mut buf = [0u8; 64];
                let _ = stream.read(&mut buf).await;
                drop(stream);
            }

            // 2nd attempt: accept and return valid HTTP 200 OK JSON
            if let Ok((mut stream, _)) = listener.accept().await {
                let mut buf = [0u8; 1024];
                let _ = stream.read(&mut buf).await;
                let resp = b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 15\r\nConnection: close\r\n\r\n{\"status\":\"ok\"}";
                let _ = stream.write_all(resp).await;
                let _ = stream.shutdown().await;
            }
        });

        let client = build_upstream_http_client();
        let target_url = format!("http://{}/backend-api/codex/responses", addr);
        let (res, attempts) = send_upstream_with_retry(
            &client,
            Method::POST,
            &target_url,
            HeaderMap::new(),
            Bytes::from_static(b"{\"model\":\"gpt-6-luna\"}"),
        )
        .await
        .expect("second attempt should succeed after transient socket drop");

        assert_eq!(attempts, 2);
        assert_eq!(res.status(), reqwest::StatusCode::OK);
        let body_text = res.text().await.unwrap();
        assert_eq!(body_text, "{\"status\":\"ok\"}");
        let _ = server_task.await;
    }

    #[tokio::test]
    async fn test_proxy_handler_retries_and_returns_502_with_full_chain_when_exhausted() {
        use axum::body::to_bytes;
        use std::sync::atomic::{AtomicUsize, Ordering};
        use tokio::io::AsyncReadExt;
        use tower::ServiceExt;

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let accept_count = Arc::new(AtomicUsize::new(0));
        let accept_count_clone = accept_count.clone();

        let server_task = tokio::spawn(async move {
            for _ in 0..2 {
                if let Ok((mut stream, _)) = listener.accept().await {
                    accept_count_clone.fetch_add(1, Ordering::SeqCst);
                    let mut buf = [0u8; 64];
                    let _ = stream.read(&mut buf).await;
                    drop(stream);
                }
            }
        });

        let state = Arc::new(ProxyAppState {
            http_client: build_upstream_http_client(),
        });
        let app = create_router(state);

        let test_upstream = format!("http://{}/backend-api/codex/responses", addr);
        let req = axum::extract::Request::builder()
            .uri("/backend-api/codex/responses")
            .method("POST")
            .header("x-codex-test-upstream", &test_upstream)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(r#"{"model":"gpt-6-luna","input":"hi"}"#))
            .unwrap();

        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
        assert_eq!(
            accept_count.load(Ordering::SeqCst),
            2,
            "proxy_handler should have attempted initial request + 1 retry"
        );

        let resp_bytes = to_bytes(response.into_body(), 65536).await.unwrap();
        let resp_json: Value = serde_json::from_slice(&resp_bytes).unwrap();
        let err_msg = resp_json["error"]["message"].as_str().unwrap_or("");
        assert!(
            err_msg.contains("Codex 9Router Proxy upstream connection error to"),
            "unexpected 502 message: {}",
            err_msg
        );
        assert!(
            err_msg.contains(" -> "),
            "expected full Error::source() chain in 502 message: {}",
            err_msg
        );
        assert!(
            err_msg.contains("[hint:"),
            "expected diagnostic hint in 502 message: {}",
            err_msg
        );

        let _ = server_task.await;
    }

    #[test]
    fn test_format_error_chain_single_and_duplicate_causes() {
        let single = TestNestedError {
            msg: "standalone error".to_string(),
            source: None,
        };
        assert_eq!(format_error_chain(&single), "standalone error");

        let dup_leaf = TestNestedError {
            msg: "os error 11001".to_string(),
            source: None,
        };
        let dup_top = TestNestedError {
            msg: "dns failure: os error 11001".to_string(),
            source: Some(Box::new(dup_leaf)),
        };
        // Duplicate substring in immediate child should not be repeated
        assert_eq!(format_error_chain(&dup_top), "dns failure: os error 11001");

        // Non-adjacent duplicate (grandchild repeating top-level substring) should also not be repeated
        let non_adj_leaf = TestNestedError {
            msg: "os error 11001".to_string(),
            source: None,
        };
        let non_adj_mid = TestNestedError {
            msg: "client error (Connect)".to_string(),
            source: Some(Box::new(non_adj_leaf)),
        };
        let non_adj_top = TestNestedError {
            msg: "dns lookup failed with os error 11001".to_string(),
            source: Some(Box::new(non_adj_mid)),
        };
        assert_eq!(
            format_error_chain(&non_adj_top),
            "dns lookup failed with os error 11001 -> client error (Connect)"
        );
    }

    #[test]
    fn test_format_upstream_error_with_flags_dns_timeout_and_request() {
        let target = "https://chatgpt.com/backend-api/codex/responses";
        let dns_chain = "error sending request for url (https://chatgpt.com/backend-api/codex/responses) -> client error (Connect) -> dns error -> No such host is known. (os error 11001)";
        let dns_msg = format_upstream_error_with_flags(target, dns_chain, false, true, true);
        assert!(dns_msg.contains("[connect/dns]"), "got: {}", dns_msg);
        assert!(
            dns_msg.contains("[hint: DNS lookup failed (e.g. os error 11001); check local internet/DNS connectivity]"),
            "got: {}",
            dns_msg
        );

        let timeout_chain = "error sending request for url -> operation timed out";
        let timeout_msg = format_upstream_error_with_flags(target, timeout_chain, true, true, true);
        assert!(timeout_msg.contains("[timeout/connect]"), "got: {}", timeout_msg);
        assert!(
            timeout_msg.contains("[hint: upstream connection timed out; check network stability or firewall]"),
            "got: {}",
            timeout_msg
        );

        let req_chain = "error sending request for url -> connection closed before message completed";
        let req_msg = format_upstream_error_with_flags(target, req_chain, false, false, true);
        assert!(req_msg.contains("[request]"), "got: {}", req_msg);
        assert!(
            req_msg.contains("[hint: transient upstream request/socket error; retried automatically]"),
            "got: {}",
            req_msg
        );
    }

    #[tokio::test]
    async fn test_format_reqwest_upstream_error_timeout_classification() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let server_task = tokio::spawn(async move {
            if let Ok((_stream, _)) = listener.accept().await {
                tokio::time::sleep(Duration::from_millis(250)).await;
            }
        });

        let client = reqwest::Client::builder()
            .timeout(Duration::from_millis(50))
            .build()
            .unwrap();
        let url = format!("http://{}/backend-api/codex/responses", addr);
        let err = client.get(&url).send().await.unwrap_err();

        let formatted = format_reqwest_upstream_error(&url, &err);
        assert!(formatted.contains("[timeout"), "expected [timeout] tag in: {}", formatted);
        assert!(
            formatted.contains("[hint: upstream connection timed out; check network stability or firewall]"),
            "expected timeout hint in: {}",
            formatted
        );
        let _ = server_task.await;
    }

    #[test]
    fn test_parse_reg_query_value() {
        let sample = "\r\nHKEY_CURRENT_USER\\Environment\r\n    CODEX_CLI_PATH    REG_SZ    C:\\Users\\test user\\AppData\\Local\\OpenAI\\Codex\\custom\\codex-9router-subagents.exe\r\n\r\n";
        assert_eq!(
            parse_reg_query_value(sample, "CODEX_CLI_PATH"),
            Some("C:\\Users\\test user\\AppData\\Local\\OpenAI\\Codex\\custom\\codex-9router-subagents.exe".to_string())
        );
        assert_eq!(parse_reg_query_value(sample, "OTHER_VAR"), None);
    }

    #[test]
    fn test_pick_best_orig_candidate_prefers_newest_and_ignores_zero_byte() {
        let tmp_dir = env::temp_dir().join(format!("codex_orig_test_{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp_dir);
        fs::create_dir_all(&tmp_dir).unwrap();

        let zero_file = tmp_dir.join("codex.orig.exe");
        let valid_file = tmp_dir.join("codex-9router-subagents.orig.exe");
        fs::write(&zero_file, b"").unwrap();
        fs::write(&valid_file, b"non-empty-binary").unwrap();

        let picked = pick_best_orig_candidate(&tmp_dir).unwrap();
        assert_eq!(picked, valid_file);

        let _ = fs::remove_dir_all(&tmp_dir);
    }

    #[tokio::test]
    async fn test_proxy_handler_retries_and_recovers_with_200_ok() {
        use axum::body::to_bytes;
        use std::sync::atomic::{AtomicUsize, Ordering};
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tower::ServiceExt;

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let accept_count = Arc::new(AtomicUsize::new(0));
        let accept_count_clone = accept_count.clone();

        let server_task = tokio::spawn(async move {
            // 1st connection: drop immediately after read
            if let Ok((mut stream, _)) = listener.accept().await {
                accept_count_clone.fetch_add(1, Ordering::SeqCst);
                let mut buf = [0u8; 64];
                let _ = stream.read(&mut buf).await;
                drop(stream);
            }
            // 2nd connection: respond 200 OK
            if let Ok((mut stream, _)) = listener.accept().await {
                accept_count_clone.fetch_add(1, Ordering::SeqCst);
                let mut buf = [0u8; 1024];
                let _ = stream.read(&mut buf).await;
                let resp = b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 19\r\nConnection: close\r\n\r\n{\"recovered\":true}\n";
                let _ = stream.write_all(resp).await;
                let _ = stream.shutdown().await;
            }
        });

        let state = Arc::new(ProxyAppState {
            http_client: build_upstream_http_client(),
        });
        let app = create_router(state);

        let test_upstream = format!("http://{}/backend-api/codex/responses", addr);
        let req = axum::extract::Request::builder()
            .uri("/backend-api/codex/responses")
            .method("POST")
            .header("x-codex-test-upstream", &test_upstream)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(r#"{"model":"gpt-6-luna","input":"hi"}"#))
            .unwrap();

        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(accept_count.load(Ordering::SeqCst), 2);

        let resp_bytes = to_bytes(response.into_body(), 65536).await.unwrap();
        let resp_json: Value = serde_json::from_slice(&resp_bytes).unwrap();
        assert_eq!(resp_json["recovered"], true);

        let _ = server_task.await;
    }

    #[test]
    fn test_classify_chatgpt_desktop_windows_hidden_and_simultaneous_cases() {
        // 1. Hidden exebox-* desktop only (PID 19056)
        let hidden_only = vec![
            ChatGptWindowEntry {
                desktop: "exebox-RUHQPLQG5QBR5H5HFN5CKAPIKK".to_string(),
                pid: 19056,
                hwnd: 0xF0080E,
                class_name: "Chrome_WidgetWin_1".to_string(),
                visible: true,
                rect: (0, 0, 960, 1080),
            },
            ChatGptWindowEntry {
                desktop: "exebox-RUHQPLQG5QBR5H5HFN5CKAPIKK".to_string(),
                pid: 19056,
                hwnd: 0x6B0A1C,
                class_name: "Chrome_WidgetWin_0".to_string(),
                visible: false,
                rect: (104, 104, 1544, 893),
            },
        ];
        let c1 = classify_chatgpt_desktop_windows(&hidden_only);
        assert!(c1.default_visible_windows.is_empty());
        assert!(c1.default_any_pids.is_empty());
        assert_eq!(c1.hidden_desktop_pids, vec![19056]);
        assert_eq!(
            c1.hidden_desktop_names,
            vec!["exebox-RUHQPLQG5QBR5H5HFN5CKAPIKK".to_string()]
        );

        // 2. Simultaneous Default (PID 32048) + Hidden exebox-* (PID 19056) + Chromium sbox_alternate_desktop (PID 31144)
        let simultaneous = vec![
            ChatGptWindowEntry {
                desktop: "Default".to_string(),
                pid: 32048,
                hwnd: 0x3510BE,
                class_name: "Chrome_WidgetWin_1".to_string(),
                visible: true,
                rect: (0, 0, 960, 1080),
            },
            ChatGptWindowEntry {
                desktop: "Default".to_string(),
                pid: 32048,
                hwnd: 0x3A0442,
                class_name: "Chrome_WidgetWin_1".to_string(),
                visible: false,
                rect: (0, 0, 872, 1080),
            },
            ChatGptWindowEntry {
                desktop: "sbox_alternate_desktop_local_winstation_0x3BD0".to_string(),
                pid: 31144,
                hwnd: 0x89073E,
                class_name: "Chrome_WidgetWin_0".to_string(),
                visible: false,
                rect: (0, 0, 0, 0),
            },
            ChatGptWindowEntry {
                desktop: "exebox-RUHQPLQG5QBR5H5HFN5CKAPIKK".to_string(),
                pid: 19056,
                hwnd: 0xF0080E,
                class_name: "Chrome_WidgetWin_1".to_string(),
                visible: true,
                rect: (0, 0, 960, 1080),
            },
        ];
        let c2 = classify_chatgpt_desktop_windows(&simultaneous);
        assert_eq!(c2.default_visible_windows.len(), 1);
        assert_eq!(c2.default_visible_windows[0].pid, 32048);
        assert_eq!(c2.default_visible_windows[0].hwnd, 0x3510BE);
        assert_eq!(c2.default_any_pids, vec![32048]);
        assert_eq!(c2.hidden_desktop_pids, vec![19056]);
    }

    #[test]
    fn test_sync_models_cache_file_updates_in_place_and_missing_file() {
        let tmp_dir = env::temp_dir().join(format!("codex_models_cache_test_{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp_dir);
        fs::create_dir_all(&tmp_dir).unwrap();

        let missing_path = tmp_dir.join("missing_models_cache.json");
        assert!(!sync_models_cache_file(&missing_path).unwrap());

        let cache_path = tmp_dir.join("models_cache.json");
        let initial_cache = serde_json::json!({
            "fetched_at": "2026-09-28T00:00:00Z",
            "etag": "W/\"test-etag\"",
            "models": [
                {
                    "slug": "gpt-6-luna",
                    "display_name": "GPT-6 Luna",
                    "context_window": 272000,
                    "max_context_window": 872000,
                    "effective_context_window_percent": 95,
                    "comp_hash": "3000",
                    "apply_patch_tool_type": "freeform"
                },
                {
                    "slug": "9router-subagent",
                    "display_name": "9router-subagent (9Router)",
                    "context_window": 200000,
                    "max_context_window": 200000
                }
            ]
        });
        fs::write(&cache_path, serde_json::to_vec(&initial_cache).unwrap()).unwrap();

        assert!(sync_models_cache_file(&cache_path).unwrap());
        let synced_bytes = fs::read(&cache_path).unwrap();
        let synced_json: Value = serde_json::from_slice(&synced_bytes).unwrap();
        assert_eq!(synced_json["etag"], "W/\"test-etag\"");

        let models = synced_json["models"].as_array().unwrap();
        let subagent = models
            .iter()
            .find(|m| m["slug"] == "9router-subagent")
            .unwrap();
        assert_eq!(subagent["context_window"], 872000);
        assert_eq!(subagent["max_context_window"], 872000);
        assert_eq!(subagent["effective_context_window_percent"], 95);
        assert_eq!(subagent["comp_hash"], "3000");
        assert_eq!(subagent["apply_patch_tool_type"], "function");

        let _ = fs::remove_dir_all(&tmp_dir);
    }

    #[test]
    fn test_sanitize_subagent_rehydrates_compaction_items_in_input() {
        let mut json = serde_json::json!({
            "model": "9router-subagent",
            "input": [
                {
                    "type": "compaction",
                    "id": "cmp_1",
                    "encrypted_content": "Audited src/main.rs and identified 3 routes.",
                    "summary": "old summary field",
                    "internal_chat_message_metadata_passthrough": "meta_1"
                },
                {
                    "type": "compaction_summary",
                    "id": "cmp_2",
                    "summary": [
                        {"type": "summary_text", "text": "Part A summary"},
                        {"type": "summary_text", "text": "Part B summary"}
                    ]
                },
                {
                    "type": "compaction",
                    "id": "cmp_3",
                    "encrypted_content": "   "
                },
                {
                    "type": "message",
                    "role": "user",
                    "content": [{"type": "input_text", "text": "Continue with step 2"}]
                }
            ]
        });

        sanitize_subagent_request_for_9router(&mut json);
        let input_arr = json["input"].as_array().unwrap();
        assert_eq!(input_arr.len(), 4);

        assert_eq!(input_arr[0]["type"], "message");
        assert_eq!(input_arr[0]["role"], "user");
        assert_eq!(
            input_arr[0]["content"][0]["text"],
            "[Compacted Conversation Summary]\nAudited src/main.rs and identified 3 routes."
        );
        assert!(input_arr[0].get("encrypted_content").is_none());
        assert!(input_arr[0].get("summary").is_none());
        assert!(input_arr[0]
            .get("internal_chat_message_metadata_passthrough")
            .is_none());

        assert_eq!(input_arr[1]["type"], "message");
        assert_eq!(input_arr[1]["role"], "user");
        assert_eq!(
            input_arr[1]["content"][0]["text"],
            "[Compacted Conversation Summary]\nPart A summary\nPart B summary"
        );
        assert!(input_arr[1].get("summary").is_none());

        assert_eq!(input_arr[2]["type"], "message");
        assert_eq!(input_arr[2]["role"], "user");
        assert_eq!(
            input_arr[2]["content"][0]["text"],
            "[Compacted Conversation Summary]"
        );
        assert!(input_arr[2].get("encrypted_content").is_none());
    }

    #[test]
    fn test_build_9router_compaction_request_body_strips_tools_and_appends_instruction() {
        let req = serde_json::json!({
            "model": "gpt-6-luna",
            "generate": false,
            "tools": [{"type": "function", "name": "exec_command"}],
            "tool_choice": "auto",
            "parallel_tool_calls": true,
            "input": [
                {
                    "type": "message",
                    "role": "developer",
                    "content": [{"type": "input_text", "text": "You are an explorer subagent."}]
                },
                {
                    "type": "agent_message",
                    "content": [{"type": "input_text", "text": "Audit Cargo.toml and src/main.rs"}]
                }
            ]
        });
        let raw = serde_json::to_vec(&req).unwrap();
        assert!(is_compaction_request(&raw));

        let transformed_bytes = build_9router_compaction_request_body(&raw).unwrap();
        assert!(!is_compaction_request(&transformed_bytes));

        let transformed: Value = serde_json::from_slice(&transformed_bytes).unwrap();
        assert_ne!(transformed["model"], "gpt-6-luna");
        assert!(transformed.get("generate").is_none());
        assert!(transformed.get("tools").is_none());
        assert!(transformed.get("tool_choice").is_none());
        assert!(transformed.get("parallel_tool_calls").is_none());

        let input_arr = transformed["input"].as_array().unwrap();
        assert_eq!(input_arr.len(), 3);
        assert_eq!(input_arr[0]["role"], "system");
        assert_eq!(input_arr[1]["type"], "message");
        assert_eq!(input_arr[1]["role"], "user");
        assert_eq!(input_arr[2]["type"], "message");
        assert_eq!(input_arr[2]["role"], "user");
        assert_eq!(
            input_arr[2]["content"][0]["text"],
            COMPACTION_SUMMARIZATION_PROMPT
        );
    }

    #[test]
    fn test_extract_summary_from_9router_response_json_and_sse_variants() {
        // 1. JSON Responses API format
        let json_resp = serde_json::to_vec(&serde_json::json!({
            "id": "resp_1",
            "output": [
                {
                    "type": "message",
                    "role": "assistant",
                    "content": [
                        {"type": "output_text", "text": "Summary from JSON response."}
                    ]
                }
            ]
        }))
        .unwrap();
        assert_eq!(
            extract_summary_from_9router_response(&json_resp),
            Some("Summary from JSON response.".to_string())
        );

        // 2. SSE delta + output_item.done format
        let sse_resp = b"event: response.output_text.delta\ndata: {\"type\":\"response.output_text.delta\",\"delta\":\"Streamed \"}\n\nevent: response.output_text.delta\ndata: {\"type\":\"response.output_text.delta\",\"delta\":\"summary.\"}\n\nevent: response.output_item.done\ndata: {\"type\":\"response.output_item.done\",\"item\":{\"type\":\"message\",\"role\":\"assistant\",\"content\":[{\"type\":\"output_text\",\"text\":\"Streamed summary.\"}]}}\n\ndata: [DONE]\n";
        assert_eq!(
            extract_summary_from_9router_response(sse_resp),
            Some("Streamed summary.".to_string())
        );

        // 3. Empty or error payload returns None
        let err_resp = br#"{"error":{"message":"upstream failed"}}"#;
        assert_eq!(extract_summary_from_9router_response(err_resp), None);
        assert_eq!(extract_summary_from_9router_response(b""), None);
    }

    #[test]
    fn test_build_deterministic_local_summary_with_unicode_and_empty_inputs() {
        let unicode_text = "◆ Subagent 🩺 — → ".repeat(60);
        let req = serde_json::json!({
            "input": [
                {
                    "type": "message",
                    "role": "user",
                    "content": [{"type": "input_text", "text": unicode_text}]
                },
                {
                    "type": "function_call",
                    "name": "exec_command",
                    "arguments": "{\"cmd\":\"cargo test\"}"
                },
                {
                    "type": "function_call_output",
                    "output": "test result: ok. 50 passed"
                }
            ]
        });
        let summary = build_deterministic_local_summary(&req);
        assert!(summary.contains("◆ Subagent 🩺"));
        assert!(summary.contains("tool_call(exec_command)"));
        assert!(summary.contains("tool_output: test result: ok. 50 passed"));

        let empty_summary = build_deterministic_local_summary(&serde_json::json!({"input": []}));
        assert!(!empty_summary.trim().is_empty());
    }

    #[tokio::test]
    async fn test_proxy_handler_subagent_compaction_via_9router_and_fallback_when_9router_fails() {
        use axum::body::to_bytes;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tower::ServiceExt;

        // Case A: 9Router returns a normal assistant message; proxy wraps it into a type=compaction SSE response
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let server_task = tokio::spawn(async move {
            if let Ok((mut stream, _)) = listener.accept().await {
                let mut buf = vec![0u8; 8192];
                let n = stream.read(&mut buf).await.unwrap_or(0);
                let req_str = String::from_utf8_lossy(&buf[..n]);
                assert!(
                    !req_str.contains("\"generate\":false"),
                    "generate:false must be stripped before sending to 9Router"
                );
                assert!(
                    !req_str.contains("gpt-6-luna"),
                    "gpt-6-luna must be rewritten to 9router subagent model"
                );
                let body = r#"{"id":"resp_9r","output":[{"type":"message","role":"assistant","content":[{"type":"output_text","text":"9Router compacted summary of audit."}]}]}"#;
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(resp.as_bytes()).await;
                let _ = stream.shutdown().await;
            }
        });

        let state = Arc::new(ProxyAppState {
            http_client: build_upstream_http_client(),
        });
        let app = create_router(state.clone());

        let test_upstream = format!("http://{}/v1/responses", addr);
        let compaction_req = serde_json::json!({
            "model": "gpt-6-luna",
            "generate": false,
            "tools": [{"type": "function", "name": "exec_command"}],
            "input": [
                {
                    "type": "message",
                    "role": "user",
                    "content": [{"type": "input_text", "text": "Audit repository security rules"}]
                }
            ]
        });
        let req = axum::extract::Request::builder()
            .uri("/backend-api/codex/responses")
            .method("POST")
            .header("x-openai-subagent", "explorer")
            .header("x-codex-test-upstream", &test_upstream)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(serde_json::to_vec(&compaction_req).unwrap()))
            .unwrap();

        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let ctype = response
            .headers()
            .get(header::CONTENT_TYPE)
            .unwrap()
            .to_str()
            .unwrap();
        assert!(ctype.starts_with("text/event-stream"));

        let resp_bytes = to_bytes(response.into_body(), 65536).await.unwrap();
        let sse_text = String::from_utf8_lossy(&resp_bytes);
        assert!(sse_text.contains("event: response.created"));
        assert!(sse_text.contains("event: response.output_item.done"));
        assert!(sse_text.contains("\"type\":\"compaction\""));
        assert!(sse_text.contains("\"encrypted_content\":\"9Router compacted summary of audit.\""));
        assert!(sse_text.contains("event: response.completed"));
        let _ = server_task.await;

        // Case B: 9Router is unreachable / returns error -> proxy falls back to deterministic local summary
        let closed_listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let closed_addr = closed_listener.local_addr().unwrap();
        drop(closed_listener);

        let app_fallback = create_router(state);
        let closed_upstream = format!("http://{}/v1/responses", closed_addr);
        let req_fallback = axum::extract::Request::builder()
            .uri("/backend-api/codex/responses")
            .method("POST")
            .header("x-openai-subagent", "worker")
            .header("x-codex-test-upstream", &closed_upstream)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(serde_json::to_vec(&compaction_req).unwrap()))
            .unwrap();

        let fallback_res = app_fallback.oneshot(req_fallback).await.unwrap();
        assert_eq!(fallback_res.status(), StatusCode::OK);
        let fb_bytes = to_bytes(fallback_res.into_body(), 65536).await.unwrap();
        let fb_text = String::from_utf8_lossy(&fb_bytes);
        assert!(fb_text.contains("event: response.output_item.done"));
        assert!(fb_text.contains("\"type\":\"compaction\""));
        assert!(fb_text.contains("Audit repository security rules"));
        assert!(fb_text.contains("event: response.completed"));
    }

    #[tokio::test]
    async fn test_is_models_path_and_proxy_handler_codex_models_injection() {
        use axum::body::to_bytes;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tower::ServiceExt;

        assert!(is_models_path(
            "/backend-api/codex/models?client_version=0.158.0-alpha.2"
        ));
        assert!(is_models_path("/backend-api/codex/models"));
        assert!(is_models_path("/codex/models"));
        assert!(is_models_path("/backend-api/models"));
        assert!(is_models_path("/models"));
        assert!(!is_models_path("/backend-api/codex/responses"));

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let server_task = tokio::spawn(async move {
            if let Ok((mut stream, _)) = listener.accept().await {
                let mut buf = vec![0u8; 4096];
                let n = stream.read(&mut buf).await.unwrap_or(0);
                let req_str = String::from_utf8_lossy(&buf[..n]).to_lowercase();
                assert!(
                    !req_str.contains("if-none-match"),
                    "If-None-Match must be stripped on /backend-api/codex/models"
                );
                let body = r#"{"models":[]}"#;
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nETag: \"v1\"\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(resp.as_bytes()).await;
                let _ = stream.shutdown().await;
            }
        });

        let state = Arc::new(ProxyAppState {
            http_client: build_upstream_http_client(),
        });
        let app = create_router(state);
        let test_upstream = format!("http://{}/backend-api/codex/models?client_version=0.158.0-alpha.2", addr);

        let req = axum::extract::Request::builder()
            .uri("/backend-api/codex/models?client_version=0.158.0-alpha.2")
            .method("GET")
            .header("if-none-match", "\"v1\"")
            .header("accept-encoding", "zstd, gzip")
            .header("x-codex-test-upstream", &test_upstream)
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert!(response.headers().get(header::ETAG).is_none());

        let resp_bytes = to_bytes(response.into_body(), 65536).await.unwrap();
        let parsed: Value = serde_json::from_slice(&resp_bytes).unwrap();
        let models = parsed["models"].as_array().unwrap();
        for slug in ["9router-subagent", "explore", "review", "implement"] {
            let entry = models
                .iter()
                .find(|m| m["slug"] == slug)
                .unwrap_or_else(|| panic!("missing injected model {}", slug));
            assert_eq!(entry["context_window"], 872000);
            assert_eq!(entry["max_context_window"], 872000);
            assert_eq!(entry["effective_context_window_percent"], 95);
            assert_eq!(entry["comp_hash"], "3000");
            assert_eq!(entry["prefer_websockets"], false);
            assert_eq!(entry["use_responses_lite"], true);
            assert!(
                entry["base_instructions"]
                    .as_str()
                    .is_some_and(|s| !s.is_empty())
                    || entry["model_messages"]["instructions_template"]
                        .as_str()
                        .is_some_and(|s| !s.is_empty()),
                "injected entry must have non-empty base_instructions or model_messages.instructions_template"
            );
        }
        let _ = server_task.await;
    }

    #[test]
    fn test_sanitize_subagent_request_strips_v2_markers_and_converts_custom_tool_calls() {
        let mut req = serde_json::json!({
            "model": "9router-subagent",
            "input": [
                {"type": "compaction_trigger", "reason": "auto"},
                {"type": "configuration_update", "model": "gpt-6-luna"},
                {"type": "context_compaction", "encrypted_content": "Context compacted summary v2"},
                {"type": "custom_tool_call", "name": "apply_patch", "input": "*** Begin Patch\n*** End Patch"},
                {"type": "custom_tool_call_output", "output": "Done"},
                {"type": "tool_search_call", "query": " collaboration "},
                {"type": "tool_search_output", "output": "spawn_agent"}
            ]
        });

        sanitize_subagent_request_for_9router(&mut req);
        let input = req["input"].as_array().unwrap();
        assert_eq!(input.len(), 5);
        assert_eq!(input[0]["type"], "message");
        assert_eq!(input[0]["role"], "user");
        assert!(input[0]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("Context compacted summary v2"));

        assert_eq!(input[1]["type"], "message");
        assert_eq!(input[1]["role"], "assistant");
        assert!(input[1]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("[Tool Call: apply_patch]"));

        assert_eq!(input[2]["type"], "message");
        assert_eq!(input[2]["role"], "user");
        assert!(input[2]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("Done"));

        assert_eq!(input[3]["type"], "message");
        assert_eq!(input[3]["role"], "assistant");
        assert!(input[3]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("[Tool Search]"));

        assert_eq!(input[4]["type"], "message");
        assert_eq!(input[4]["role"], "user");
        assert!(input[4]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("spawn_agent"));
    }

    #[tokio::test]
    async fn test_compaction_request_normalizes_tool_calls_reasoning_and_zstd_payload() {
        use axum::body::to_bytes;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tower::ServiceExt;

        let raw_req = serde_json::json!({
            "model": "gpt-6-luna",
            "generate": false,
            "include": ["reasoning.encrypted_content"],
            "service_tier": "priority",
            "tools": [{"type": "function", "name": "shell_command"}],
            "input": [
                {"type": "reasoning", "encrypted_content": "gAAAA_opaque_blob", "summary": []},
                {"type": "reasoning", "encrypted_content": "gAAAA_2", "summary": [{"type": "summary_text", "text": "Planned audit steps"}]},
                {"type": "function_call", "name": "shell_command", "arguments": "{\"command\":\"cargo test\"}", "call_id": "call_1"},
                {"type": "function_call_output", "call_id": "call_1", "output": "58 passed"}
            ]
        });

        let sum_bytes =
            build_9router_compaction_request_body(&serde_json::to_vec(&raw_req).unwrap()).unwrap();
        let sum_json: Value = serde_json::from_slice(&sum_bytes).unwrap();
        assert!(sum_json.get("include").is_none());
        assert!(sum_json.get("service_tier").is_none());
        assert!(sum_json.get("tools").is_none());

        let sum_input = sum_json["input"].as_array().unwrap();
        // Opaque reasoning dropped; summary reasoning + function_call + function_call_output + instruction = 4 message items
        assert_eq!(sum_input.len(), 4);
        for item in sum_input {
            assert_eq!(item["type"], "message");
        }

        // Now verify zstd-compressed compaction request through proxy_handler
        let compressed = zstd::encode_all(serde_json::to_vec(&raw_req).unwrap().as_slice(), 3).unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let server_task = tokio::spawn(async move {
            if let Ok((mut stream, _)) = listener.accept().await {
                let mut buf = vec![0u8; 8192];
                let _ = stream.read(&mut buf).await.unwrap_or(0);
                let body = r#"{"id":"resp_zstd","output":[{"type":"message","role":"assistant","content":[{"type":"output_text","text":"Zstd compaction summary."}]}]}"#;
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(resp.as_bytes()).await;
                let _ = stream.shutdown().await;
            }
        });

        let state = Arc::new(ProxyAppState {
            http_client: build_upstream_http_client(),
        });
        let app = create_router(state);
        let test_upstream = format!("http://{}/v1/responses", addr);

        let req = axum::extract::Request::builder()
            .uri("/backend-api/codex/responses")
            .method("POST")
            .header("x-openai-subagent", "reviewer")
            .header("content-encoding", "zstd")
            .header("x-codex-test-upstream", &test_upstream)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(compressed))
            .unwrap();

        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let resp_bytes = to_bytes(response.into_body(), 65536).await.unwrap();
        let sse_text = String::from_utf8_lossy(&resp_bytes);
        assert!(sse_text.contains("\"encrypted_content\":\"Zstd compaction summary.\""));
        let _ = server_task.await;
    }

    #[test]
    fn test_is_lightweight_cli_invocation_interactive_codex_is_not_lightweight() {
        // Interactive `codex` (empty args) MUST NOT be lightweight so it gets `:20129` proxy + chatgpt_base_url
        assert!(!is_lightweight_cli_invocation(&[]));
        assert!(!is_lightweight_cli_invocation(&[
            "exec".to_string(),
            "check git status".to_string()
        ]));
        assert!(!is_lightweight_cli_invocation(&[
            "resume".to_string(),
            "--last".to_string()
        ]));
        assert!(!is_lightweight_cli_invocation(&[
            "app-server".to_string(),
            "daemon".to_string(),
            "start".to_string()
        ]));
        assert!(!is_lightweight_cli_invocation(&[
            "app-server".to_string(),
            "daemon".to_string(),
            "restart".to_string()
        ]));

        // True lightweight commands
        assert!(is_lightweight_cli_invocation(&["--version".to_string()]));
        assert!(is_lightweight_cli_invocation(&["-h".to_string()]));
        assert!(is_lightweight_cli_invocation(&["login".to_string()]));
        assert!(is_lightweight_cli_invocation(&[
            "app-server".to_string(),
            "daemon".to_string(),
            "stop".to_string()
        ]));
    }

    #[test]
    fn test_is_compaction_http_headers_and_body_variants() {
        // 1. x-codex-turn-metadata JSON with request_kind = compaction and compaction object
        let mut h1 = HeaderMap::new();
        h1.insert(
            header::HeaderName::from_static("x-codex-turn-metadata"),
            HeaderValue::from_static(
                r#"{"turn_id":"019ac141","thread_source":"subagent","request_kind":"compaction","compaction":{"trigger":"auto"}}"#,
            ),
        );
        assert!(is_compaction_http_headers(&h1));
        assert!(is_subagent_http_headers(&h1));

        let body_without_generate = br#"{"model":"9router-subagent","input":[{"type":"message","role":"user","content":[{"type":"input_text","text":"Inspect git status"}]}]}"#;
        assert!(!is_compaction_request(body_without_generate));
        assert!(is_compaction_request_with_headers(&h1, body_without_generate));

        // 2. x-codex-turn-metadata with only "compaction":{...} object
        let mut h2 = HeaderMap::new();
        h2.insert(
            header::HeaderName::from_static("x-codex-turn-metadata"),
            HeaderValue::from_static(r#"{"compaction":{"reason":"model_downshift"}}"#),
        );
        assert!(is_compaction_http_headers(&h2));

        // 3. Substring fallback when x-codex-turn-metadata is truncated/non-standard
        let mut h3 = HeaderMap::new();
        h3.insert(
            header::HeaderName::from_static("x-codex-turn-metadata"),
            HeaderValue::from_static(r#"{"request_kind": "compaction", "truncated": "#),
        );
        assert!(is_compaction_http_headers(&h3));

        // 4. Normal subagent turn metadata (not compaction, even if compaction: null is present)
        let mut h_normal = HeaderMap::new();
        h_normal.insert(
            header::HeaderName::from_static("x-codex-turn-metadata"),
            HeaderValue::from_static(
                r#"{"turn_id":"t1","thread_source":"subagent","subagent_kind":"thread_spawn","compaction":null}"#,
            ),
        );
        assert!(!is_compaction_http_headers(&h_normal));
        assert!(is_subagent_http_headers(&h_normal));
        assert!(!is_compaction_request_with_headers(
            &h_normal,
            body_without_generate
        ));

        // 5. Normal root user turn metadata (neither subagent nor compaction)
        let mut h_root = HeaderMap::new();
        h_root.insert(
            header::HeaderName::from_static("x-codex-turn-metadata"),
            HeaderValue::from_static(
                r#"{"turn_id":"t_root","thread_source":"user","agent_name":"/root","parent_thread_id":null}"#,
            ),
        );
        assert!(!is_compaction_http_headers(&h_root));
        assert!(!is_subagent_http_headers(&h_root));

        // 6. Body-level client_metadata and request_kind compaction variants
        let body_client_meta = serde_json::to_vec(&serde_json::json!({
            "model": "9router-subagent",
            "client_metadata": {
                "x-codex-turn-metadata": "{\"request_kind\":\"compaction\",\"compaction\":{\"trigger\":\"auto\"}}"
            },
            "input": []
        }))
        .unwrap();
        assert!(is_compaction_request(&body_client_meta));
        let transformed = build_9router_compaction_request_body(&body_client_meta).unwrap();
        assert!(!is_compaction_request(&transformed));
    }

    #[tokio::test]
    async fn test_proxy_handler_http_post_compaction_via_turn_metadata_header_without_generate_false() {
        use axum::body::to_bytes;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tower::ServiceExt;

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let server_task = tokio::spawn(async move {
            if let Ok((mut stream, _)) = listener.accept().await {
                let mut buf = vec![0u8; 8192];
                let n = stream.read(&mut buf).await.unwrap_or(0);
                let req_str = String::from_utf8_lossy(&buf[..n]);
                let req_lower = req_str.to_ascii_lowercase();
                assert!(
                    !req_lower.contains("x-codex-turn-metadata"),
                    "x-codex-turn-metadata header must be stripped before forwarding summarization request to 9Router: {}",
                    req_str
                );
                assert!(
                    req_str.contains(COMPACTION_SUMMARIZATION_PROMPT),
                    "summarization prompt must be appended to input when forwarding to 9Router"
                );
                let body = r#"{"id":"resp_hdr_cmp","output":[{"type":"message","role":"assistant","content":[{"type":"output_text","text":"Compacted via x-codex-turn-metadata header."}]}]}"#;
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(resp.as_bytes()).await;
                let _ = stream.shutdown().await;
            }
        });

        let state = Arc::new(ProxyAppState {
            http_client: build_upstream_http_client(),
        });
        let app = create_router(state);
        let test_upstream = format!("http://{}/v1/responses", addr);

        // Body intentionally omits "generate": false and "client_metadata", matching codex.orig.exe HTTP POST behavior
        let http_compaction_body = serde_json::json!({
            "model": "9router-subagent",
            "instructions": "You are a coding worker.",
            "tools": [{"type": "function", "name": "shell_command"}],
            "input": [
                {
                    "type": "message",
                    "role": "user",
                    "content": [{"type": "input_text", "text": "Audit git worktree ownership"}]
                }
            ]
        });

        let req = axum::extract::Request::builder()
            .uri("/backend-api/codex/responses")
            .method("POST")
            .header(
                "x-codex-turn-metadata",
                r#"{"turn_id":"019ac999","thread_source":"subagent","subagent_kind":"thread_spawn","request_kind":"compaction","compaction":{"trigger":"auto"}}"#,
            )
            .header("x-codex-test-upstream", &test_upstream)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(serde_json::to_vec(&http_compaction_body).unwrap()))
            .unwrap();

        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let ctype = response
            .headers()
            .get(header::CONTENT_TYPE)
            .unwrap()
            .to_str()
            .unwrap();
        assert!(ctype.starts_with("text/event-stream"));

        let resp_bytes = to_bytes(response.into_body(), 65536).await.unwrap();
        let sse_text = String::from_utf8_lossy(&resp_bytes);
        assert!(sse_text.contains("event: response.created"));
        assert!(sse_text.contains("event: response.output_item.done"));
        assert!(sse_text.contains("\"type\":\"compaction\""));
        assert!(
            sse_text.contains("\"encrypted_content\":\"Compacted via x-codex-turn-metadata header.\"")
        );
        assert!(sse_text.contains("event: response.completed"));
        let _ = server_task.await;
    }

    #[test]
    fn test_base64_and_codex_orig_turn_metadata_variants() {
        // 1. Base64-encoded JSON {"turn_id":"t_b64","thread_source":"thread_spawn","agent_name":"/root/explorer","request_kind":"compaction"}
        // Standard base64: eyJ0dXJuX2lkIjoidF9iNjQiLCJ0aHJlYWRfc291cmNlIjoidGhyZWFkX3NwYXduIiwiYWdlbnRfbmFtZSI6Ii9yb290L2V4cGxvcmVyIiwicmVxdWVzdF9raW5kIjoiY29tcGFjdGlvbiJ9
        let b64_meta = "eyJ0dXJuX2lkIjoidF9iNjQiLCJ0aHJlYWRfc291cmNlIjoidGhyZWFkX3NwYXduIiwiYWdlbnRfbmFtZSI6Ii9yb290L2V4cGxvcmVyIiwicmVxdWVzdF9raW5kIjoiY29tcGFjdGlvbiJ9";
        let mut h_b64 = HeaderMap::new();
        h_b64.insert(
            header::HeaderName::from_static("x-codex-turn-metadata"),
            HeaderValue::from_static(b64_meta),
        );
        assert!(is_subagent_http_headers(&h_b64));
        assert!(is_compaction_http_headers(&h_b64));
        assert_eq!(
            extract_role_from_http_headers(&h_b64).as_deref(),
            Some("explorer")
        );

        // 2. codex.orig.exe TurnMetadata with parent_turn_id / forked_from_thread_id and subagent_kind = compact
        let mut h_forked = HeaderMap::new();
        h_forked.insert(
            header::HeaderName::from_static("x-codex-turn-metadata"),
            HeaderValue::from_static(
                r#"{"turn_id":"t2","parent_turn_id":"pt1","forked_from_thread_id":"th_parent","subagent_kind":"compact","agent_name":"/root/worker_1"}"#,
            ),
        );
        assert!(is_subagent_http_headers(&h_forked));
        assert!(is_compaction_http_headers(&h_forked));
        assert_eq!(
            extract_role_from_http_headers(&h_forked).as_deref(),
            Some("worker")
        );

        // 3. Mechanism tag x-openai-subagent: collab_spawn paired with x-codex-turn-metadata agent_name: /root/reviewer
        let mut h_collab = HeaderMap::new();
        h_collab.insert(
            header::HeaderName::from_static("x-openai-subagent"),
            HeaderValue::from_static("collab_spawn"),
        );
        h_collab.insert(
            header::HeaderName::from_static("x-codex-turn-metadata"),
            HeaderValue::from_static(
                r#"{"thread_source":{"subagent":"thread_spawn"},"agent_name":"/root/reviewer"}"#,
            ),
        );
        assert!(is_subagent_http_headers(&h_collab));
        assert_eq!(
            extract_role_from_http_headers(&h_collab).as_deref(),
            Some("reviewer")
        );
    }

    #[tokio::test]
    async fn test_previous_model_compaction_fallback_and_field_stripping() {
        use axum::body::to_bytes;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tower::ServiceExt;

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let server_task = tokio::spawn(async move {
            if let Ok((mut stream, _)) = listener.accept().await {
                let mut buf = vec![0u8; 16384];
                let n = stream.read(&mut buf).await.unwrap_or(0);
                let req_str = String::from_utf8_lossy(&buf[..n]);
                let body_idx = req_str.find("\r\n\r\n").map(|i| i + 4).unwrap_or(0);
                let forwarded_json: Value = serde_json::from_str(&req_str[body_idx..]).unwrap();

                // Model must be rewritten away from parent gpt-6-luna
                assert_ne!(
                    forwarded_json.get("model").and_then(|v| v.as_str()),
                    Some("gpt-6-luna")
                );
                // Internal OpenAI fields must be stripped
                for forbidden in [
                    "previous_response_id",
                    "prompt_cache_key",
                    "access_programs",
                    "codex_output_schema",
                    "store",
                    "stream_options",
                    "tools",
                ] {
                    assert!(
                        forwarded_json.get(forbidden).is_none(),
                        "field {} should be stripped from compaction summarization request",
                        forbidden
                    );
                }

                // Input items must have image_generation_call and local_shell_call_output normalized to "message",
                // and must NOT duplicate COMPACTION_SUMMARIZATION_PROMPT when CONTEXT CHECKPOINT COMPACTION is already present
                let input_arr = forwarded_json
                    .get("input")
                    .and_then(|v| v.as_array())
                    .unwrap();
                for item in input_arr {
                    assert_eq!(item.get("type").and_then(|v| v.as_str()), Some("message"));
                }
                let dump = serde_json::to_string(input_arr).unwrap();
                assert!(dump.contains("[Image Generation Call]"));
                assert!(dump.contains("[Tool Output]"));
                assert!(dump.contains("CONTEXT CHECKPOINT COMPACTION"));
                assert!(
                    !dump.contains(COMPACTION_SUMMARIZATION_PROMPT),
                    "should not append duplicate COMPACTION_SUMMARIZATION_PROMPT when CONTEXT CHECKPOINT COMPACTION is already present"
                );

                let body = r#"{"id":"resp_prev_model_cmp","output":[{"type":"message","role":"assistant","content":[{"type":"output_text","text":"Previous-model fallback compacted via 9Router."}]}]}"#;
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(resp.as_bytes()).await;
                let _ = stream.shutdown().await;
            }
        });

        let state = Arc::new(ProxyAppState {
            http_client: build_upstream_http_client(),
        });
        let app = create_router(state);
        let test_upstream = format!("http://{}/v1/responses", addr);

        // Simulate compact_model_fallback.rs ("previous-model compaction" with model="gpt-6-luna")
        let fallback_compaction_body = serde_json::json!({
            "model": "gpt-6-luna",
            "previous_response_id": "resp_parent_123",
            "prompt_cache_key": "cache_key_abc",
            "access_programs": ["internal_alpha"],
            "codex_output_schema": {"type": "object"},
            "store": false,
            "stream_options": {"include_obfuscation": true},
            "tools": [{"type": "custom", "name": "apply_patch"}],
            "input": [
                {
                    "type": "image_generation_call",
                    "id": "img_1",
                    "revised_prompt": "architecture diagram"
                },
                {
                    "type": "local_shell_call_output",
                    "call_id": "call_1",
                    "output": "exit code 0"
                },
                {
                    "type": "message",
                    "role": "user",
                    "content": [{"type": "input_text", "text": "CONTEXT CHECKPOINT COMPACTION\nSummarize work so far."}]
                }
            ]
        });

        let req = axum::extract::Request::builder()
            .uri("/backend-api/codex/responses")
            .method("POST")
            .header(
                "x-codex-turn-metadata",
                r#"{"turn_id":"t_prev","parent_turn_id":"pt_root","forked_from_thread_id":"th_root","agent_name":"/root/explorer","request_kind":"compact"}"#,
            )
            .header("x-codex-test-upstream", &test_upstream)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                serde_json::to_vec(&fallback_compaction_body).unwrap(),
            ))
            .unwrap();

        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let resp_bytes = to_bytes(response.into_body(), 65536).await.unwrap();
        let sse_text = String::from_utf8_lossy(&resp_bytes);
        assert!(sse_text.contains(
            "\"encrypted_content\":\"Previous-model fallback compacted via 9Router.\""
        ));
        let _ = server_task.await;
    }

    #[test]
    fn test_subagent_never_leaks_chatgpt_oauth_token_when_ninerouter_key_unset() {
        let mut incoming = HeaderMap::new();
        incoming.insert(
            header::AUTHORIZATION,
            HeaderValue::from_static("Bearer eyJhbGciOiJSUzI1NiIsInR5cCI6IkpXVCJ9.chatgpt_oauth_secret"),
        );
        incoming.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        );

        let subagent_headers_no_key = build_forward_headers_with_subagent_auth(&incoming, true, None);
        assert_eq!(
            subagent_headers_no_key
                .get(header::AUTHORIZATION)
                .and_then(|v| v.to_str().ok()),
            Some("Bearer dummy-9router-key"),
            "subagent requests must replace incoming ChatGPT OAuth token with dummy-9router-key when NINEROUTER_KEY is unset"
        );

        let subagent_headers_empty_key =
            build_forward_headers_with_subagent_auth(&incoming, true, Some("   "));
        assert_eq!(
            subagent_headers_empty_key
                .get(header::AUTHORIZATION)
                .and_then(|v| v.to_str().ok()),
            Some("Bearer dummy-9router-key")
        );

        let subagent_headers_custom_key =
            build_forward_headers_with_subagent_auth(&incoming, true, Some("Bearer custom-9router-token"));
        assert_eq!(
            subagent_headers_custom_key
                .get(header::AUTHORIZATION)
                .and_then(|v| v.to_str().ok()),
            Some("Bearer custom-9router-token")
        );

        let parent_headers = build_forward_headers_with_subagent_auth(&incoming, false, None);
        assert_eq!(
            parent_headers
                .get(header::AUTHORIZATION)
                .and_then(|v| v.to_str().ok()),
            Some("Bearer eyJhbGciOiJSUzI1NiIsInR5cCI6IkpXVCJ9.chatgpt_oauth_secret"),
            "parent requests must preserve incoming ChatGPT OAuth token"
        );
    }

    #[test]
    fn test_inject_subagent_models_metadata_inherits_non_luna_parent_comp_hash_and_context_override() {
        // 1. Catalog where primary parent model is gpt-5.4 with comp_hash = "1000" (no gpt-6-luna)
        let catalog_gpt54 = serde_json::json!({
            "models": [
                {
                    "slug": "gpt-5.4",
                    "display_name": "GPT-5.4",
                    "context_window": 272000,
                    "max_context_window": 400000,
                    "comp_hash": "1000"
                }
            ]
        });
        let enriched_bytes =
            inject_subagent_models_metadata(&serde_json::to_vec(&catalog_gpt54).unwrap());
        let enriched: Value = serde_json::from_slice(&enriched_bytes).unwrap();
        let models = enriched["models"].as_array().unwrap();
        let subagent = models
            .iter()
            .find(|m| m["slug"] == "9router-subagent")
            .unwrap();
        assert_eq!(
            subagent["comp_hash"], "1000",
            "must inherit comp_hash from parent gpt-5.4 template to prevent comp_hash_changed compaction"
        );
        assert_eq!(subagent["context_window"], 872000);
        assert_eq!(subagent["max_context_window"], 872000);

        // 2. Catalog containing both gpt-6-luna (comp_hash="3000") and gpt-5.4 (comp_hash="1000")
        //    when user's primary model in config.toml is "gpt-5.4"
        let catalog_both = serde_json::json!({
            "models": [
                {
                    "slug": "gpt-6-luna",
                    "display_name": "GPT-6 Luna",
                    "context_window": 272000,
                    "max_context_window": 872000,
                    "comp_hash": "3000"
                },
                {
                    "slug": "gpt-5.4",
                    "display_name": "GPT-5.4",
                    "context_window": 272000,
                    "max_context_window": 400000,
                    "comp_hash": "1000"
                }
            ]
        });
        let enriched_primary_bytes = inject_subagent_models_metadata_with_primary(
            &serde_json::to_vec(&catalog_both).unwrap(),
            Some("gpt-5.4"),
        );
        let enriched_primary: Value = serde_json::from_slice(&enriched_primary_bytes).unwrap();
        let subagent_primary = enriched_primary["models"]
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["slug"] == "9router-subagent")
            .unwrap();
        assert_eq!(subagent_primary["comp_hash"], "1000");

        // 3. Stale subagent-only catalog must NOT select subagent as template and must reset comp_hash to "3000"
        let stale_subagent_only = serde_json::json!({
            "models": [
                {
                    "slug": "9router-subagent",
                    "context_window": 200000,
                    "max_context_window": 200000,
                    "comp_hash": "stale_hash"
                }
            ]
        });
        let enriched_stale_bytes = inject_subagent_models_metadata_with_primary(
            &serde_json::to_vec(&stale_subagent_only).unwrap(),
            None,
        );
        let enriched_stale: Value = serde_json::from_slice(&enriched_stale_bytes).unwrap();
        let subagent_stale = enriched_stale["models"]
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["slug"] == "9router-subagent")
            .unwrap();
        assert_eq!(
            subagent_stale["comp_hash"], "3000",
            "stale subagent-only catalog must not inherit its own stale_hash as template"
        );
        assert_eq!(subagent_stale["context_window"], 872000);

        // 4. Explicit CODEX_SUBAGENT_CONTEXT_WINDOW override and larger parent template fallback
        let tmpl = &catalog_gpt54["models"][0];
        assert_eq!(
            resolve_subagent_context_window_with_override(Some(tmpl), Some("256000")),
            256000
        );
        assert_eq!(
            resolve_subagent_context_window_with_override(Some(tmpl), None),
            872000
        );
        let large_tmpl = serde_json::json!({"max_context_window": 1000000, "comp_hash": "5000"});
        assert_eq!(
            resolve_subagent_context_window_with_override(Some(&large_tmpl), None),
            1000000
        );
        assert_eq!(resolve_subagent_comp_hash(Some(&large_tmpl)), "5000");
        assert_eq!(resolve_subagent_comp_hash(None), "3000");
    }

    #[test]
    fn test_is_parent_chatgpt_model_includes_o4_and_codex_prefixes() {
        assert!(is_parent_chatgpt_model("gpt-6-luna"));
        assert!(is_parent_chatgpt_model("gpt-5.4"));
        assert!(is_parent_chatgpt_model("o1-pro"));
        assert!(is_parent_chatgpt_model("o3-mini"));
        assert!(is_parent_chatgpt_model("o4-mini"));
        assert!(is_parent_chatgpt_model("codex-mini-latest"));
        assert!(is_parent_chatgpt_model("chatgpt-4o-latest"));
        assert!(!is_parent_chatgpt_model("9router-subagent"));
        assert!(!is_parent_chatgpt_model("qwen2.5-coder:32b"));

        let mut headers = HeaderMap::new();
        headers.insert("x-openai-subagent", HeaderValue::from_static("worker"));
        for parent_model in ["o4-mini", "codex-mini-latest"] {
            let body = serde_json::to_vec(&serde_json::json!({
                "model": parent_model,
                "input": [{"type": "message", "role": "user", "content": [{"type": "input_text", "text": "test"}]}]
            }))
            .unwrap();
            let (is_sub, routed) = inspect_and_route_http_request_with_headers(
                "/backend-api/codex/responses",
                &headers,
                &body,
            );
            assert!(is_sub);
            let parsed: Value = serde_json::from_slice(&routed).unwrap();
            assert_eq!(parsed["model"], map_role_to_model(Some("worker")));
        }
    }

    #[test]
    fn test_config_and_role_toml_resolution_for_models_provider_and_endpoint() {
        let tmp_home =
            env::temp_dir().join(format!("codex_home_portability_test_{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp_home);
        let agents_dir = tmp_home.join("agents");
        fs::create_dir_all(&agents_dir).unwrap();

        // Both default.toml (with 9router) and worker.toml (customized to ollama) exist
        fs::write(
            agents_dir.join("default.toml"),
            "name = \"default\"\nmodel = \"default-toml-model\"\nmodel_provider = \"9router\"\n",
        )
        .unwrap();
        fs::write(
            agents_dir.join("worker.toml"),
            "name = \"worker\"\nmodel = \"hot-reloaded-worker\"\nmodel_provider = \"ollama\"\n",
        )
        .unwrap();
        let config_toml = "model = \"gpt-5.4\"\n\n[model_providers.openai]\nname = \"openai\"\nbase_url = \"https://api.openai.com/v1\"\n\n[model_providers.ollama]\nname = \"ollama\"\nbase_url = \"http://127.0.0.1:11434/v1\"\nenv_key = \"OLLAMA_API_KEY\"\n\n[agents]\ndefault_subagent_model = \"hot-reloaded-default\"\n";
        fs::write(tmp_home.join("config.toml"), config_toml).unwrap();

        assert_eq!(
            read_primary_model_from_config_in_dir(&tmp_home).as_deref(),
            Some("gpt-5.4")
        );
        assert_eq!(
            read_model_from_config_in_dir(&tmp_home, "worker").as_deref(),
            Some("hot-reloaded-worker")
        );
        assert_eq!(
            read_model_from_config_in_dir(&tmp_home, "explorer").as_deref(),
            Some("hot-reloaded-default")
        );
        assert_eq!(
            read_provider_for_role_from_config_in_dir(&tmp_home, Some("worker")).as_deref(),
            Some("ollama")
        );
        assert_eq!(
            read_provider_for_role_from_config_in_dir(&tmp_home, Some("default")).as_deref(),
            Some("9router")
        );
        assert_eq!(
            read_provider_from_config_in_dir(&tmp_home).as_deref(),
            Some("ollama")
        );
        assert_eq!(
            read_subagent_endpoint_from_config_in_dir(&tmp_home, "ollama").as_deref(),
            Some("http://127.0.0.1:11434/v1/responses")
        );
        assert_eq!(
            read_subagent_endpoint_from_config_in_dir(&tmp_home, "unconfigured_provider"),
            None,
            "must not fall back to [model_providers.openai] or unrelated provider base_url"
        );
        assert_eq!(
            parse_provider_env_key_from_config_toml(config_toml, "ollama").as_deref(),
            Some("OLLAMA_API_KEY")
        );

        let _ = fs::remove_dir_all(&tmp_home);
    }

    #[test]
    fn test_sync_custom_codex_binaries_from_candidates_auto_heals_and_guards_against_self_copy() {
        let tmp_root = env::temp_dir().join(format!(
            "codex_sync_binaries_test_{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&tmp_root);
        let candidate_dir = tmp_root.join("store_app_resources");
        let custom_dir = tmp_root.join("custom");
        fs::create_dir_all(&candidate_dir).unwrap();
        fs::create_dir_all(&custom_dir).unwrap();

        let fake_proxy_exe = custom_dir.join("codex.exe");
        // Small proxy binary (e.g. 200 bytes)
        fs::write(&fake_proxy_exe, vec![b'P'; 200]).unwrap();

        // Candidate dir has real codex.exe (5000 bytes, larger than proxy) + companion binaries
        let real_codex_bytes = vec![b'R'; 5000];
        fs::write(candidate_dir.join("codex.exe"), &real_codex_bytes).unwrap();
        fs::write(
            candidate_dir.join("codex-command-runner.exe"),
            b"runner-v2",
        )
        .unwrap();
        fs::write(
            candidate_dir.join("codex-windows-sandbox-setup.exe"),
            b"sandbox-v2",
        )
        .unwrap();
        fs::write(
            candidate_dir.join("codex-code-mode-host.exe"),
            b"codemode-v2",
        )
        .unwrap();

        // Pre-populate stale custom orig binaries
        fs::write(custom_dir.join("codex.orig.exe"), b"old-codex").unwrap();
        fs::write(
            custom_dir.join("codex-9router-subagents.orig.exe"),
            b"old-codex",
        )
        .unwrap();

        let updated = sync_custom_codex_binaries_from_candidates_with_min_size(
            &custom_dir,
            std::slice::from_ref(&candidate_dir),
            1_000,
        );
        assert!(updated >= 5, "expected at least 5 binaries synced, got {}", updated);
        assert_eq!(
            fs::read(custom_dir.join("codex.orig.exe")).unwrap(),
            real_codex_bytes
        );
        assert_eq!(
            fs::read(custom_dir.join("codex-9router-subagents.orig.exe")).unwrap(),
            real_codex_bytes
        );
        assert_eq!(
            fs::read(custom_dir.join("codex-command-runner.exe")).unwrap(),
            b"runner-v2"
        );
        assert_eq!(
            fs::read(custom_dir.join("codex-windows-sandbox-setup.exe")).unwrap(),
            b"sandbox-v2"
        );
        assert_eq!(
            fs::read(custom_dir.join("codex-code-mode-host.exe")).unwrap(),
            b"codemode-v2"
        );

        // Now simulate a candidate dir where codex.exe was already replaced with the small proxy binary (200 bytes):
        // sync_custom_codex_binaries_from_candidates MUST NOT overwrite codex.orig.exe with the proxy binary!
        let shimmed_candidate_dir = tmp_root.join("shimmed_bin");
        fs::create_dir_all(&shimmed_candidate_dir).unwrap();
        fs::write(shimmed_candidate_dir.join("codex.exe"), vec![b'P'; 200]).unwrap();
        let updated_shimmed = sync_custom_codex_binaries_from_candidates_with_min_size(
            &custom_dir,
            std::slice::from_ref(&shimmed_candidate_dir),
            1_000,
        );
        assert_eq!(updated_shimmed, 0);
        assert_eq!(
            fs::read(custom_dir.join("codex.orig.exe")).unwrap().len(),
            5000,
            "must never overwrite codex.orig.exe with the proxy shim binary"
        );

        let _ = fs::remove_dir_all(&tmp_root);
    }

    #[test]
    fn test_extract_package_family_name_and_version_tuple() {
        assert_eq!(
            extract_package_family_name_from_full_name(
                "OpenAI.Codex_26.924.2738.0_x64__2p2nqsd0c76g0"
            )
            .as_deref(),
            Some("OpenAI.Codex_2p2nqsd0c76g0")
        );
        assert_eq!(
            parse_package_version_tuple("OpenAI.Codex_26.924.2738.0_x64__2p2nqsd0c76g0"),
            (26, 924, 2738, 0)
        );
        assert!(
            parse_package_version_tuple("OpenAI.Codex_26.1000.0.0_x64__2p2nqsd0c76g0")
                > parse_package_version_tuple("OpenAI.Codex_26.924.2738.0_x64__2p2nqsd0c76g0")
        );
    }

    #[test]
    fn test_custom_role_manifests_model_metadata_and_per_role_auth_endpoint_resolution() {
        let tmp_home = env::temp_dir().join(format!(
            "codex_custom_role_test_{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&tmp_home);
        let agents_dir = tmp_home.join("agents");
        fs::create_dir_all(&agents_dir).unwrap();

        fs::write(
            agents_dir.join("researcher.toml"),
            "name = \"researcher\"\nmodel = \"deepseek-r1-custom\"\nmodel_provider = \"openrouter\"\n",
        )
        .unwrap();
        fs::write(
            agents_dir.join("worker.toml"),
            "name = \"worker\"\nmodel = \"qwen3-coder-plus\"\nmodel_provider = \"9router\"\n",
        )
        .unwrap();

        let config_toml = r#"
model = "gpt-5.4"

[model_providers.9router]
name = "9router"
base_url = "http://127.0.0.1:20128/v1"
env_key = "NINEROUTER_KEY"

[model_providers.openrouter]
name = "openrouter"
base_url = "https://openrouter.ai/api/v1"
env_key = "CODEX_TEST_OPENROUTER_KEY"

[subagent_models]
auditor = "claude-3-7-sonnet-audit"
"#;
        fs::write(tmp_home.join("config.toml"), config_toml).unwrap();

        // 1. Custom role model discovery & is_subagent_model_name_in_dir
        let models = collect_configured_subagent_models_in_dir(Some(&tmp_home));
        assert!(models.contains(&"deepseek-r1-custom".to_string()));
        assert!(models.contains(&"qwen3-coder-plus".to_string()));
        assert!(models.contains(&"claude-3-7-sonnet-audit".to_string()));
        assert!(is_subagent_model_name_in_dir(
            "deepseek-r1-custom",
            Some(&tmp_home)
        ));
        assert!(is_subagent_model_name_in_dir(
            "claude-3-7-sonnet-audit",
            Some(&tmp_home)
        ));
        assert!(!is_subagent_model_name_in_dir("o5-mini", Some(&tmp_home)));
        assert!(!is_subagent_model_name_in_dir(
            "computer-use-preview",
            Some(&tmp_home)
        ));

        // 2. Models catalog injection includes custom role models
        let base_catalog = serde_json::json!({
            "models": [
                {
                    "slug": "gpt-5.4",
                    "display_name": "GPT-5.4",
                    "context_window": 272000,
                    "max_context_window": 400000,
                    "comp_hash": "1000"
                }
            ]
        });
        let enriched_bytes = inject_subagent_models_metadata_in_dir(
            &serde_json::to_vec(&base_catalog).unwrap(),
            Some("gpt-5.4"),
            Some(&tmp_home),
        );
        let enriched: Value = serde_json::from_slice(&enriched_bytes).unwrap();
        let arr = enriched["models"].as_array().unwrap();
        assert!(arr.iter().any(|m| m["slug"] == "deepseek-r1-custom"));
        assert!(arr.iter().any(|m| m["slug"] == "claude-3-7-sonnet-audit"));

        // 3. Per-role provider & endpoint resolution
        assert_eq!(
            get_target_model_provider_for_role_in_dir(Some(&tmp_home), Some("researcher")),
            "openrouter"
        );
        assert_eq!(
            get_subagent_responses_url_for_role_in_dir(Some("researcher"), Some(&tmp_home)),
            "https://openrouter.ai/api/v1/responses"
        );
        assert_eq!(
            get_subagent_responses_url_for_role_in_dir(Some("worker"), Some(&tmp_home)),
            "http://127.0.0.1:20128/v1/responses"
        );

        // 4. Active provider env_key takes precedence over NINEROUTER_KEY & strips duplicate Bearer prefix
        env::set_var("CODEX_TEST_OPENROUTER_KEY", "Bearer sk-or-test-key-123");
        let auth = get_subagent_auth_header_for_role_in_dir(Some("researcher"), Some(&tmp_home));
        env::remove_var("CODEX_TEST_OPENROUTER_KEY");
        assert_eq!(auth.as_deref(), Some("Bearer sk-or-test-key-123"));
        assert_eq!(
            format_bearer_header_value("Bearer Bearer my-token").as_deref(),
            Some("Bearer my-token")
        );

        let _ = fs::remove_dir_all(&tmp_home);
    }

    #[test]
    fn test_sanitize_subagent_request_filters_tool_search_and_drops_empty_tools_array() {
        let mut req = serde_json::json!({
            "model": "9router-subagent",
            "tool_choice": "auto",
            "tools": [
                { "type": "tool_search" },
                { "type": "function", "name": "tool_search", "parameters": {} },
                { "type": "web_search_preview" }
            ],
            "input": [
                { "type": "message", "role": "user", "content": [{"type": "input_text", "text": "hi"}] }
            ]
        });
        sanitize_subagent_request_for_9router(&mut req);
        assert!(
            req.get("tools").is_none(),
            "empty tools array must be removed after filtering incompatible tools"
        );
        assert!(
            req.get("tool_choice").is_none(),
            "tool_choice must be removed when tools array becomes empty"
        );

        let mut req_with_valid_tool = serde_json::json!({
            "model": "9router-subagent",
            "tool_choice": "auto",
            "tools": [
                { "type": "tool_search" },
                { "type": "function", "name": "apply_patch", "parameters": {} }
            ]
        });
        sanitize_subagent_request_for_9router(&mut req_with_valid_tool);
        let remaining_tools = req_with_valid_tool["tools"].as_array().unwrap();
        assert_eq!(remaining_tools.len(), 1);
        assert_eq!(remaining_tools[0]["name"], "apply_patch");
        assert_eq!(req_with_valid_tool["tool_choice"], "auto");
    }

    #[test]
    fn test_create_tls_acceptor_self_heals_corrupted_pem_and_fallback_port_binding() {
        let tmp_tls = env::temp_dir().join(format!("codex_tls_heal_test_{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp_tls);
        fs::create_dir_all(&tmp_tls).unwrap();

        let cert_path = tmp_tls.join("bridge-cert.pem");
        let key_path = tmp_tls.join("bridge-key.pem");
        fs::write(&cert_path, "-----BEGIN CERTIFICATE-----\ncorrupted\n-----END CERTIFICATE-----\n").unwrap();
        fs::write(&key_path, "-----BEGIN PRIVATE KEY-----\ncorrupted\n-----END PRIVATE KEY-----\n").unwrap();

        let acceptor = create_tls_acceptor(&cert_path, &key_path);
        assert!(
            acceptor.is_ok(),
            "create_tls_acceptor must self-heal corrupted PEM files on disk"
        );

        // Occupy a random local TCP port with a raw non-TLS listener and verify
        // spawn_embedded_reverse_proxy_with_port detects it is not our TLS proxy and binds a fallback port.
        let dummy_listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let occupied_addr = dummy_listener.local_addr().unwrap().to_string();
        let occupied_port = dummy_listener.local_addr().unwrap().port().to_string();

        let (returned_cert, bound_port) =
            spawn_embedded_reverse_proxy_with_port(&occupied_addr).unwrap();
        assert!(returned_cert.is_file());
        assert_ne!(
            bound_port, occupied_port,
            "must bind a fallback port when requested port is occupied by a non-proxy listener"
        );
        let verified = verify_existing_tls_proxy_listener(
            &format!("127.0.0.1:{}", bound_port),
            &returned_cert,
        );
        assert!(
            verified,
            "spawned fallback proxy listener must pass TLS /health verification"
        );

        drop(dummy_listener);
        let _ = fs::remove_dir_all(&tmp_tls);
    }

    #[test]
    fn test_cli_arg_injection_before_double_dash_lightweight_subcommands_and_doctor_guard() {
        let args = vec![
            "exec".to_string(),
            "--full-auto".to_string(),
            "--".to_string(),
            "-leading-dash-prompt".to_string(),
        ];
        let injected = inject_loopback_base_url(&args, "20129");
        assert_eq!(
            injected,
            vec![
                "exec".to_string(),
                "--full-auto".to_string(),
                "-c".to_string(),
                "chatgpt_base_url=\"https://127.0.0.1:20129/backend-api/\"".to_string(),
                "--".to_string(),
                "-leading-dash-prompt".to_string(),
            ]
        );

        let args_with_pre_and_post_overrides = vec![
            "exec".to_string(),
            "--config".to_string(),
            "chatgpt_base_url=\"https://old.example/\"".to_string(),
            "--".to_string(),
            "-c".to_string(),
            "chatgpt_base_url=literal_prompt_arg".to_string(),
        ];
        let injected2 = inject_loopback_base_url(&args_with_pre_and_post_overrides, "20130");
        assert_eq!(
            injected2,
            vec![
                "exec".to_string(),
                "-c".to_string(),
                "chatgpt_base_url=\"https://127.0.0.1:20130/backend-api/\"".to_string(),
                "--".to_string(),
                "-c".to_string(),
                "chatgpt_base_url=literal_prompt_arg".to_string(),
            ],
            "must strip --config chatgpt_base_url before -- while preserving literal -c args after --"
        );

        let tmp_hook = env::temp_dir().join(format!("codex_hook_surface_test_{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp_hook);
        let ext_bin = tmp_hook.join("vscode_ext_bin");
        fs::create_dir_all(&ext_bin).unwrap();
        let proxy_shim = tmp_hook.join("codex-9router-subagents.exe");
        fs::write(&proxy_shim, vec![b'P'; 200]).unwrap();
        // Fresh VS Code extension directory with stock > 1000-byte codex.exe and NO codex.orig.exe yet
        fs::write(ext_bin.join("codex.exe"), vec![b'S'; 5000]).unwrap();
        let hooked_count = sync_hooked_surface_binaries_with_min_size(
            &proxy_shim,
            std::slice::from_ref(&ext_bin),
            1_000,
        );
        assert_eq!(hooked_count, 1);
        assert_eq!(fs::read(ext_bin.join("codex.orig.exe")).unwrap().len(), 5000);
        assert_eq!(fs::read(ext_bin.join("codex.exe")).unwrap().len(), 200);
        let _ = fs::remove_dir_all(&tmp_hook);

        let tmp_role_lookup = env::temp_dir().join(format!("codex_role_lookup_test_{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp_role_lookup);
        fs::create_dir_all(tmp_role_lookup.join("agents")).unwrap();
        fs::write(
            tmp_role_lookup.join("agents").join("researcher.toml"),
            "name = \"researcher\"\nmodel = \"deepseek-r1-custom\"\nmodel_provider = \"openrouter\"\n",
        )
        .unwrap();
        assert_eq!(
            find_role_for_model_in_dir(Some(&tmp_role_lookup), "deepseek-r1-custom").as_deref(),
            Some("researcher")
        );
        let _ = fs::remove_dir_all(&tmp_role_lookup);

        for sub in [
            "sandbox",
            "stdio-to-uds",
            "mcp",
            "plugin",
            "update",
            "debug",
            "apply",
            "archive",
            "unarchive",
            "delete",
            "migrate-rollouts",
            "app",
            "generate-ts",
            "generate-json-schema",
        ] {
            assert!(
                is_lightweight_cli_invocation(&[sub.to_string()]),
                "{} must be classified as lightweight CLI invocation",
                sub
            );
            assert!(
                is_lightweight_cli_invocation(&[
                    "-c".to_string(),
                    "foo=bar".to_string(),
                    sub.to_string()
                ]),
                "{} after -c flag must be classified as lightweight CLI invocation",
                sub
            );
        }
        assert!(!is_lightweight_cli_invocation(&[]));
        assert!(!is_lightweight_cli_invocation(&[
            "exec".to_string(),
            "hello".to_string()
        ]));
        assert!(!is_lightweight_cli_invocation(&["app-server".to_string()]));

        assert!(is_doctor_cli_invocation(&["--doctor".to_string()]));
        assert!(is_doctor_cli_invocation(&["doctor".to_string()]));
        assert!(is_doctor_cli_invocation(&["--proxy-status".to_string()]));
        assert!(
            !is_doctor_cli_invocation(&["exec".to_string(), "doctor".to_string()]),
            "codex exec doctor must never be hijacked by run_doctor()"
        );
    }

    #[test]
    fn test_standby_failover_takeover_and_proxy_daemon_single_instance_lock_and_antigravity_cleanup() {
        // 1. Verify Antigravity extension roots & .old.* cleanup in sync_hooked_surface_binaries_with_min_size
        assert!(IDE_EXTENSION_ROOTS.contains(&".antigravity\\extensions"));
        assert!(IDE_EXTENSION_ROOTS.contains(&".antigravity-ide\\extensions"));
        assert!(IDE_EXTENSION_ROOTS.contains(&".vscode\\extensions"));
        assert!(IDE_EXTENSION_ROOTS.contains(&".cursor\\extensions"));
        assert!(IDE_EXTENSION_ROOTS.contains(&".windsurf\\extensions"));

        let tmp_dir = env::temp_dir().join(format!(
            "codex_standby_failover_test_{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&tmp_dir);
        let ag_ext_bin = tmp_dir.join("antigravity_ext_bin");
        fs::create_dir_all(&ag_ext_bin).unwrap();
        let proxy_shim = tmp_dir.join("codex-9router-subagents.exe");
        fs::write(&proxy_shim, vec![b'P'; 200]).unwrap();
        fs::write(ag_ext_bin.join("codex.exe"), vec![b'S'; 5000]).unwrap();
        let stale_old_file = ag_ext_bin.join("codex.exe.old.99999");
        fs::write(&stale_old_file, b"stale").unwrap();

        let hooked = sync_hooked_surface_binaries_with_min_size(
            &proxy_shim,
            std::slice::from_ref(&ag_ext_bin),
            1_000,
        );
        assert_eq!(hooked, 1);
        assert!(
            !stale_old_file.exists(),
            "unlocked .old.* files in hooked surface directories must be cleaned up"
        );
        assert_eq!(fs::read(ag_ext_bin.join("codex.orig.exe")).unwrap().len(), 5000);
        assert_eq!(fs::read(ag_ext_bin.join("codex.exe")).unwrap().len(), 200);

        // 2. Verify single-instance --proxy-daemon exclusive lockfile acquisition & non-mutating probe
        let lock_path = proxy_daemon_lockfile_path_in_dir(&tmp_dir, "20129");
        assert_eq!(lock_path, tmp_dir.join("proxy-daemon-20129.lock"));
        assert_eq!(
            proxy_daemon_lockfile_path_in_dir(&tmp_dir, "   "),
            tmp_dir.join("proxy-daemon-20129.lock")
        );
        assert!(
            !is_proxy_daemon_lock_held_at(&lock_path),
            "non-existent lockfile must not be reported as held"
        );

        let lock1 = try_acquire_proxy_daemon_lock_at(&lock_path);
        assert!(
            lock1.is_some(),
            "first --proxy-daemon lock acquisition must succeed"
        );
        #[cfg(windows)]
        {
            assert!(
                is_proxy_daemon_lock_held_at(&lock_path),
                "lockfile must be reported as held while handle is open"
            );
            let lock2 = try_acquire_proxy_daemon_lock_at(&lock_path);
            assert!(
                lock2.is_none(),
                "second --proxy-daemon lock acquisition must fail while first handle is held"
            );
        }
        drop(lock1);
        assert!(
            !is_proxy_daemon_lock_held_at(&lock_path),
            "unlocked lockfile must not be reported as held after handle is dropped"
        );

        // Simulate a brief 5ms probe holding share_mode(0) while try_acquire_proxy_daemon_lock_at starts:
        let probe_lock_path = lock_path.clone();
        let probe_thread = thread::spawn(move || {
            let mut opts = fs::OpenOptions::new();
            opts.read(true).write(true);
            #[cfg(windows)]
            {
                use std::os::windows::fs::OpenOptionsExt;
                opts.share_mode(0);
            }
            if let Ok(h) = opts.open(&probe_lock_path) {
                thread::sleep(Duration::from_millis(8));
                drop(h);
            }
        });
        thread::sleep(Duration::from_millis(2));
        let lock3 = try_acquire_proxy_daemon_lock_at(&lock_path);
        probe_thread.join().unwrap();
        assert!(
            lock3.is_some(),
            "--proxy-daemon lock acquisition must ride through transient probe and succeed"
        );
        drop(lock3);

        // 3. Verify 5 concurrent standby failover threads contending on AddrInUse when primary listener closes
        let (cert_path, key_path) = default_cert_paths();
        let primary_acceptor = create_tls_acceptor(&cert_path, &key_path).unwrap();
        let primary_std = TcpListener::bind("127.0.0.1:0").unwrap();
        let primary_addr = primary_std.local_addr().unwrap().to_string();
        let primary_port = primary_std.local_addr().unwrap().port().to_string();
        primary_std.set_nonblocking(true).unwrap();

        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
        let primary_handle = thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            rt.block_on(async move {
                let listener = tokio::net::TcpListener::from_std(primary_std).unwrap();
                let app = create_router(Arc::new(ProxyAppState {
                    http_client: build_upstream_http_client(),
                }));
                tokio::pin!(shutdown_rx);
                loop {
                    tokio::select! {
                        _ = &mut shutdown_rx => {
                            break;
                        }
                        res = listener.accept() => {
                            let Ok((stream, _)) = res else { continue; };
                            let acc = primary_acceptor.clone();
                            let router = app.clone();
                            tokio::spawn(async move {
                                if let Ok(tls_stream) = acc.accept(stream).await {
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
                                    let _ = auto::Builder::new(TokioExecutor::new())
                                        .serve_connection_with_upgrades(io, service)
                                        .await;
                                }
                            });
                        }
                    }
                }
            });
        });

        assert!(
            verify_existing_tls_proxy_listener(&primary_addr, &cert_path),
            "primary TLS proxy listener must be healthy before standby registration"
        );

        // Register 5 concurrent standby failover threads against the already-bound primary_addr
        // (simulating 5 concurrent app-server / CLI processes all in standby mode).
        let mut standby_cert = cert_path.clone();
        for _ in 0..5 {
            let (c, standby_port) =
                spawn_embedded_reverse_proxy_with_port(&primary_addr).unwrap();
            assert_eq!(standby_port, primary_port);
            standby_cert = c;
        }

        // Now close the primary listener (simulating closing the ChatGPT Desktop app).
        let _ = shutdown_tx.send(());
        primary_handle.join().unwrap();

        // Wait ~320ms for one of the 5 standby failover threads to win TcpListener::bind(primary_addr) and start serving.
        thread::sleep(Duration::from_millis(320));
        assert!(
            verify_existing_tls_proxy_listener(&primary_addr, &standby_cert),
            "one of the 5 standby failover threads must take over {} within ~300ms after primary listener exits",
            primary_addr
        );

        // Verify /__codex_9router_proxy_healthz is also served with 200 OK by the standby listener
        // and that the remaining 4 standby threads stay cleanly in their 250ms loop across another cycle.
        thread::sleep(Duration::from_millis(300));
        let cert_bytes = fs::read(&standby_cert).unwrap();
        let healthz_url = format!("https://{}/__codex_9router_proxy_healthz", primary_addr);
        let healthz_ok = thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            rt.block_on(async move {
                let cert = reqwest::Certificate::from_pem(&cert_bytes).unwrap();
                let client = reqwest::Client::builder()
                    .add_root_certificate(cert)
                    .timeout(Duration::from_millis(800))
                    .build()
                    .unwrap();
                let resp = client.get(&healthz_url).send().await.unwrap();
                resp.status() == reqwest::StatusCode::OK
                    && resp.text().await.unwrap().contains("codex-9router-proxy")
            })
        })
        .join()
        .unwrap();
        assert!(
            healthz_ok,
            "/__codex_9router_proxy_healthz must return 200 OK on standby listener after multi-thread contention"
        );

        let _ = fs::remove_dir_all(&tmp_dir);
    }
}
