# v0.2.5 (2026-09-28)

## Features
- **Subagent Remote-Compaction via 9Router + Deterministic Local Fallback (`"generate": false` & `x-codex-turn-metadata`)**: intercept subagent `/responses` remote-compaction requests via both `x-codex-turn-metadata` HTTP header (raw JSON, base64-encoded JSON, `"request_kind": "compaction"` / `"compact"`, `"subagent_kind": "compact"`, `"compaction": {...}`, `"parent_turn_id"`, `"forked_from_thread_id"`, and `"agent_name": "/root/<role>"`) and JSON body (`"generate": false` / `"request_kind"` / `"client_metadata"`), preserve subagent role routing even during previous-model compaction fallback (`compact_model_fallback.rs`), strip `x-codex-turn-metadata` and ChatGPT-internal fields (`"previous_response_id"`, `"prompt_cache_key"`, `"access_programs"`, `"codex_output_schema"`, `"store"`, `"stream_options"`), transform them into 9Router summarization requests (stripping `"generate"`, `"tools"`, `"tool_choice"`, `"parallel_tool_calls"`, `"include"`, and `"service_tier"`, normalizing tool-call/output and reasoning history items into `"type": "message"`, and appending a summarization instruction when not already present), extract the assistant summary from 9Router (or fall back to a deterministic local summary constructed from `"input"`), and return a valid Responses API `text/event-stream` SSE sequence (`response.created`, `response.output_item.done` with `"type": "compaction"` and `"encrypted_content"`, and `response.completed`) strictly without routing subagent requests to ChatGPT
- **Compaction Checkpoint & Forked History Rehydration (`"type": "compaction"`, `"context_compaction"`, `"custom_tool_call"`, `"image_generation_call"`)**: rehydrate `"type": "compaction"` / `"compaction_summary"` / `"context_compaction"` items in `"input"` into standard `"type": "message"` (`"role": "user"`) items prefixed with `[Compacted Conversation Summary]`, convert forked `gpt-6-luna` `"custom_tool_call"`, `"custom_tool_call_output"`, `"tool_search_call"`, `"tool_search_output"`, and `"image_generation_call"` items into standard `"type": "message"` items, and strip internal `"compaction_trigger"` and `"configuration_update"` markers inside `sanitize_subagent_request_for_9router`
- **`gpt-6-luna` Metadata Alignment (`872k` Context Window) & `models_cache.json` In-Place Sync**: update `inject_subagent_models_metadata` to prefer `"gpt-6-luna"` as the base template, configure `"context_window": 872000`, `"max_context_window": 872000`, `"effective_context_window_percent": 95`, `"comp_hash": "3000"`, `"prefer_websockets": false`, and `"use_responses_lite": true`, guarantee non-empty `base_instructions` / `model_messages.instructions_template`, update existing subagent entries in place, and synchronize `~/.codex/models_cache.json` at startup, `--doctor`, and installation (`sync_codex_models_cache` / `sync_models_cache_file`) so subagents forked from `gpt-6-luna` never trigger spurious `model_downshift` or `comp_hash_changed` pre-sampling compactions
- **Hidden-Desktop (`exebox-*`) GUI Self-Healing**: detect and terminate `ChatGPT.exe` instances stranded on hidden sandbox desktops (`exebox-*`) while preserving active `WinSta0\Default` windows and cleaning stale singleton lockfiles during `app-server` startup, `codex --doctor`, `install.ps1`, and `hook-sync.ps1`
- **Standalone Reverse-Proxy Daemon (`--proxy-daemon`)**: support `codex-9router-subagents.exe --proxy-daemon` in `main()`, `install.ps1`, and `hook-sync.ps1` to maintain an always-on loopback proxy listener on `127.0.0.1:20129`

## Fixes
- **`GET /backend-api/codex/models` Path Matching**: update `is_models_path` to match `/backend-api/codex/models`, `/codex/models`, and `*/models` (not just `/backend-api/models`) so live `codex.orig.exe` model catalog refreshes strip `If-None-Match` / `Accept-Encoding` and receive subagent model metadata injection
- **Interactive CLI (`codex` with No Arguments) Proxy Injection**: fix `is_lightweight_cli_invocation` so interactive `codex` TUI invocations (`args.is_empty()`) and `app-server daemon start/restart` are no longer treated as lightweight commands and always receive the embedded `:20129` reverse proxy, `CODEX_CA_CERTIFICATE`, and `-c chatgpt_base_url="https://127.0.0.1:20129/backend-api/"`

# v0.2.4 (2026-09-28)

## Features
- **Upstream Connection Resilience & Automatic Retry**: add `.connect_timeout(Duration::from_secs(10))` to the shared upstream `reqwest::Client` (`build_upstream_http_client`) and perform a single automatic retry (`send_upstream_with_retry`) in `proxy_handler` when `send().await` fails with a transient connection/request error (`err.is_connect() || err.is_request()`), recovering transparently from stale pooled HTTP/2 or TLS sockets after sleep/wake or network transitions
- **Full `Error::source()` Diagnostic Chain & Hints (`502 Bad Gateway`)**: walk the full `std::error::Error::source()` hierarchy (`format_error_chain` / `format_reqwest_upstream_error`) when formatting `502 Bad Gateway` JSON responses and include connection/timeout/DNS classifications and hints so local network/DNS blackouts (e.g. Windows `os error 11001`) are immediately self-explanatory in the Codex UI
- **`CODEX_CLI_PATH` Override Persistence**: persist `CODEX_CLI_PATH = "%LOCALAPPDATA%\OpenAI\Codex\custom\codex-9router-subagents.exe"` in Windows User environment variables (`install.ps1` & `hook-sync.ps1`) so `OpenAI.Codex` (`26.924.1866.0` `app.asar` `source=override`) launches the custom proxy shim directly, and clean up `CODEX_CLI_PATH` in `uninstall.ps1`
- **Automatic Microsoft Store Binary & Helper Sync**: update `install.ps1` and `%LOCALAPPDATA%\OpenAI\Codex\custom\hook-sync.ps1` to automatically refresh `custom\codex.orig.exe`, `custom\codex-9router-subagents.orig.exe`, and companion helpers (`codex-command-runner.exe`, `codex-windows-sandbox-setup.exe`, `codex-windows-sandbox-service.exe`, `codex-code-mode-host.exe`, `rg.exe`) from `WindowsApps\OpenAI.Codex*\app\resources` whenever the Microsoft Store `OpenAI.Codex` package updates
- **Diagnostics Doctor (`--doctor`)**: report `Custom Shim Binary` (`custom\codex-9router-subagents.exe`) and `CODEX_CLI_PATH` verification status in `run_doctor`

## Fixes
- **Preserve Stock `bin\<hash>\codex.exe` Integrity**: stop overwriting `bin\<hash>\codex.exe` with the 6.2 MB proxy binary and restore any previously replaced `bin\<hash>\codex.exe` from `codex.orig.exe` or Microsoft Store `app\resources\codex.exe`, preventing Electron's runtime byte-size and SHA-256 verification from deleting `bin\<hash>`

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
