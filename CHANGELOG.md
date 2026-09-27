# v0.2.3 (2026-09-27)

## Features
- **Model Metadata Injection**: inject `9router-subagent` and all custom role models configured under `[subagent_models]` in `~/.codex/config.toml` directly into `/backend-api/codex/models` responses (`inject_subagent_models_metadata`) with `272,000` context window, reasoning levels (`low` / `medium` / `high`), `parallel` tool support, and `shell_type: "default"`, eliminating the `"model metadata for 9router-subagent not found"` warning and enabling native tool execution inside spawned subagents
- **Installer (`install.ps1`)**: auto-detect prebuilt `codex-9router-proxy.exe` binaries extracted from GitHub Release `.zip` archives so layman users can install without Rust or Cargo installed
- **Dynamic Store Relocation Hash**: compute the SHA-256 `<bin-hash>` directory dynamically over the Microsoft Store `resources` executables (`codex.exe`, `codex-code-mode-host.exe`, `codex-windows-sandbox-setup.exe`, `codex-command-runner.exe`) to match Electron's runtime binary relocation
- **Documentation**: add a beginner-friendly 3-method installation guide and a Tested Environment & Version Compatibility Matrix to `README.md`

## Fixes
- **Main Agent Tool Routing**: restrict `is_subagent_role_name` exclusively to true subagent roles (`worker`, `explorer`, `reviewer`, `default`) and exclude OpenAI message roles (`system`, `developer`, `user`, `assistant`, `tool`, `function`) so main agent turns carrying `developer`/`system` instructions are never misclassified as subagents or stripped of native host tools (`powershell`, `spawn_agent`)
- **Multi-Agents V2 Input Normalization**: normalize `"type": "agent_message"` items in `/v1/responses` `"input"` into `"type": "message"` with `"role": "user"` (stripping V2-only `author`, `recipient`, and `internal_chat_message_metadata_passthrough` fields) and normalize `"role": "developer"` items into `"role": "system"` inside `sanitize_subagent_request_for_9router` so 9Router and downstream Gemini/Claude translators preserve the delegated task prompt and developer instructions
- **Subagent Tool Sanitization**: strip Codex Desktop proprietary/unregistered tool declarations (`collaboration`, `spawn_agent`, `functions`, and namespace-only stubs without `name`) before forwarding subagent turns to 9Router (`sanitize_subagent_tools_for_9router`), preventing upstream `unsupported call: functions` and `unsupported call: collaboration` errors
- **Desktop Usage Progress Bar**: normalize `rate_limits.secondary: null` into a valid zero-used window object (`sanitize_rate_limits`) on `/backend-api/wham/usage` and `/backend-api/codex/usage` responses so the Codex Desktop settings quota indicator renders without crashing on `null` dereference
- **Privacy & Binary Portability**: replace static local fallback paths with portable relative resolution (`codex.orig.exe`)

# v0.2.2 (2026-09-27)

## Features
- **HTTPS / HTTP/2 Loopback Reverse Proxy**: add self-signed TLS certificate generation (`rcgen` + `tokio-rustls`) and dual-stack HTTP/1.1 + HTTP/2 connection serving (`hyper-util` auto builder) so Codex Desktop and CLI connect seamlessly over both HTTP and HTTPS loopback
- **Zstandard (`zstd`) Request Decompression**: transparently detect and decompress `Content-Encoding: zstd` request payloads produced by `codex.exe` (`0.158.0-alpha.2+`) before inspecting JSON bodies for subagent routing
- **Helper Binary Synchronization**: copy missing helper executables (`codex-command-runner.exe`, `codex-windows-sandbox-setup.exe`, `codex-code-mode-host.exe`, `rg.exe`) into the active relocated `<bin-hash>` directory during `install.ps1` so subagent PowerShell tool execution succeeds in freshly created Store relocation folders

## Fixes
- **WebSocket Fallback (`426 Upgrade Required`)**: return HTTP `426 Upgrade Required` on WebSocket upgrade requests to `/backend-api/codex/responses` so `codex.exe` cleanly falls back to standard HTTP POST SSE where per-turn subagent inspection and routing occur
- **Upstream Compression Headers**: strip `zstd` from `Accept-Encoding` when forwarding subagent turns to 9Router so local OpenAI-compatible gateways return standard uncompressed SSE streams

# v0.2.1 (2026-09-26)

## Features
- **Embedded Loopback Reverse Proxy (`127.0.0.1:20129`)**: embed an asynchronous Axum/Reqwest loopback reverse proxy inside `codex-9router-proxy.exe` that intercepts outbound `/backend-api/codex/responses` turns spawned inside `codex.orig.exe`
- **In-Process Subagent Turn Inspection**: inspect `model`, `agent_role`, `thread_source`, and client metadata headers on every `/backend-api/codex/responses` turn; route subagents (`9router-subagent`, `worker`, `explorer`, `reviewer`, `default`) to 9Router (`/v1/responses`) while streaming main agent ChatGPT turns (`gpt-6-luna`) transparently to `https://chatgpt.com/backend-api/*`
- **Config Loopback Injection**: automatically inject `chatgpt_base_url = "http://127.0.0.1:20129/backend-api/"` into `config.toml` when missing so internal subagent turns spawned via `collaboration.spawn_agent` traverse the local routing proxy

## Fixes
- **ChatGPT Account Subagent 400 Rejection**: resolve `HTTP 400: The '9router-subagent' model is not supported when using Codex with a ChatGPT account` when spawning subagents inside the Codex Desktop GUI (`app-server`)

# v0.2.0 (2026-09-26)

## Features
- **Per-Role Model Customization**: support `[subagent_models]` in `~/.codex/config.toml` (`default`, `worker`, `explorer`, `reviewer`) and per-role `.toml` manifests in `~/.codex/agents/*.toml` so each subagent role can target a distinct 9Router combo or LLM model
- **Universal LLM Gateway Support**: add interactive provider configuration in `install.ps1` supporting 9Router, Ollama, LM Studio, OpenRouter, vLLM, LiteLLM, or any custom OpenAI-compatible `/v1` endpoint
- **Diagnostics Doctor (`--doctor`)**: add `codex --doctor` / `codex --proxy-status` CLI flag and `install.ps1 -Doctor` mode to verify binary hooks, `codex.orig.exe` discovery, role model mappings, SQLite trigger health, and live `/v1/models` gateway reachability
- **CI/CD Release Pipeline**: add `.github/workflows/release.yml` to run release unit tests, compile optimized Windows `x86_64` binaries, and publish `codex-9router-proxy-windows-amd64.zip` on version tags (`v*`)

## Fixes
- **Inherited ChatGPT Model Override**: rewrite inherited parent ChatGPT models (`gpt-6-luna`, `gpt-5*`, `o1*`, `o3*`, `o4*`, `codex*`) on subagent threads to the role's configured 9Router model

# v0.1.0 (2026-09-26)

## Features
- **Transparent Stdio JSON-RPC Proxy**: intercept `app-server` stdio JSON-RPC 2.0 requests (`thread/start`, `thread/fork`, `thread/resume`, `thread/settings/update`, `turn/start`) between Codex Desktop / VS Code Extension and `codex.orig.exe`
- **SQLite State Trigger (`fix_subagent_provider_trigger`)**: automatically install an `AFTER INSERT ON threads` trigger on `~/.codex/state_*.sqlite` to enforce `model_provider = '9router'` on newly created subagent rows
- **Interactive Installer & Uninstaller**: add `install.ps1` and `uninstall.ps1` with automatic backup/restore of `codex.orig.exe` and Windows `Startup` persistence after Microsoft Store updates
