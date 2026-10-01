# Codex 9Router Proxy ⚡

[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/Rust-1.80%2B-orange.svg)](https://www.rust-lang.org/)
[![Platform](https://img.shields.io/badge/Platform-Windows-0078D6.svg)](https://www.microsoft.com/windows)
[![Release](https://img.shields.io/github/v/release/ArchdukeViel/codex-9router-proxy?include_prereleases&color=success)](https://github.com/ArchdukeViel/codex-9router-proxy/releases)

A transparent, high-performance proxy bridge that hooks the official **OpenAI Codex Desktop App & CLI** engine on Windows.

It transparently routes child **subagent threads** (`spawn_agent`) to **9Router** or any OpenAI-compatible local/remote LLM endpoint, while preserving your primary interactive chat session on official **ChatGPT (`gpt-6-luna`)** with zero proxy latency.

---

## 🌟 Why Codex 9Router Proxy?

### The Problem
When using the official OpenAI Codex Desktop App signed into a personal or Pro ChatGPT account:
1. Spawning subagents with third-party models (`9router-subagent`, `implement`, etc.) causes OpenAI's cloud backend to reject the request with `HTTP 400: The model is not supported when using Codex with a ChatGPT account.`
2. Alternative MCP delegation tools (`subagent_9router:delegate_to_9router`) execute in the background as synchronous tool calls, which render only as small inline cards (`Worked for 1m 7s >`) in the main thread. They do **not** show up in the native sidebar.

### The Solution: Native GUI Subagents
`codex-9router-proxy` hooks the internal JSON-RPC IPC stream between the Electron frontend and the Codex engine:
- 💎 **Purple Diamond**: Renders `◆ Subagent started working` directly in the chat timeline.
- 📑 **Sidebar Child Session**: Spawns a dedicated, clickable conversation item in the left sidebar.
- ⚡ **Live Real-Time Streaming**: Watch the subagent think, execute shell commands, edit files, and stream tokens live as it works.
- 🛡️ **Zero Touched Credentials**: Your ChatGPT credentials, tokens, and official sessions stay 100% untouched and direct.

```
┌─────────────────────────────────────────────────────────────────┐
│              Codex Desktop App (Electron GUI)                   │
│        Primary Session: gpt-6-luna (ChatGPT Account)            │
└───────────────────────────────┬─────────────────────────────────┘
                                │ JSON-RPC (thread/start, turn/start)
                                ▼
┌─────────────────────────────────────────────────────────────────┐
│                    codex-9router-proxy.exe                      │
│   [Layer 1: JSON-RPC IPC Interception]                          │
│   - Detects subagents (agent_role, subagent_source, nickname)   │
│   - Multi-tier dynamic model resolution (default: 9router-subagent)│
│   - Rewrites workspaceRouting.backendOrigin -> https://chatgpt.com│
│     so Electron Usage & Billing UI and Plus badge work natively │
│   [Layer 2: Embedded HTTPS/HTTP2 Reverse Proxy (:20129)]        │
│   - Auto-generates self-signed TLS cert (CODEX_CA_CERTIFICATE)  │
│   - Rejects wss:// upgrades with HTTP 426 (0ms HTTPS fallback)  │
│   - Injects subagent model metadata into GET /backend-api/models│
│   - Sanitizes namespace/custom tools for 9Router compatibility  │
│   - Subagent responses -> 9Router (127.0.0.1:20128/v1/responses)│
│   - Parent turns -> ChatGPT Upstream (https://chatgpt.com)      │
└───────────────────────────────┬─────────────────────────────────┘
                                │ launches with -c chatgpt_base_url="https://127.0.0.1:20129/backend-api/"
                                ▼
┌─────────────────────────────────────────────────────────────────┐
│                  Official codex.orig.exe                        │
│   - Parent turns   -> HTTPS :20129 -> https://chatgpt.com       │
│   - Subagent turns -> HTTPS :20129 -> 9Router :20128            │
└───────────────────────────────┬─────────────────────────────────┘
                                │ Native IPC Stream
                                ▼
┌─────────────────────────────────────────────────────────────────┐
│                    Desktop App UI Experience                    │
│   💎 Purple Diamond: ◆ Subagent started working                 │
│   📑 Clickable child session in the left sidebar               │
│   ⚡ Live streaming thoughts, tool calls, and file diffs        │
└─────────────────────────────────────────────────────────────────┘
```

---

## 🖥️ Tested Environment & Version Compatibility Matrix

`codex-9router-proxy` `v0.2.8` is developed and verified against the following environment:

| Component | Verified Version | Notes |
| :--- | :--- | :--- |
| **Operating System** | **Windows 11 Pro** (`10.0.26200` / Build `26200`) | Compatible with Windows 10 & Windows 11 (`x86_64`) |
| **OpenAI Codex Desktop App** | **`26.929.21022.0`** ([Microsoft Store `OpenAI.Codex` & `OpenAI.CodexPrimaryRuntime`](https://apps.microsoft.com/detail/9mz1741s0917)) | Uses `CODEX_CLI_PATH` override; auto-heals across Store & split-runtime updates |
| **OpenAI Codex CLI Engine** | **`codex-cli 0.159.0-alpha.4`** (`0.158.0-alpha.2+`) | Bundled Desktop engine, standalone CLI (`Programs\OpenAI\Codex\bin`), & VS Code extension |
| **Primary ChatGPT Session** | **`gpt-6-luna`** (ChatGPT Plus / Pro Account) | Direct pass-through to `https://chatgpt.com` |
| **9Router** | **`0.5.91`** (`npm i -g 9router@latest`) | Default subagent endpoint (`http://localhost:20128/v1`) |
| **PowerShell** | **PowerShell `7.6.6`** & **Windows PowerShell `5.1`** | Both `pwsh.exe` and built-in `powershell.exe` supported |
| **Rust Toolchain** *(Optional)* | **`rustc 1.98.1`** | **Not required** when installing from the prebuilt Release `.zip` |

---

## 🤖 Codex Subagent Taxonomy: How Subagents Work

In Codex, subagents divide into two architectural tiers:

### 1. User-Facing Collaboration Subagents (Invoked via `spawn_agent`)
When the primary agent coordinates work across multiple threads, it assigns subagents specific roles:
- **`worker` (Implementation Agent)**:
  - Default execution subagent.
  - Full permissions to edit files, apply code diffs, and run terminal commands.
- **`explorer` (Research Agent)**:
  - Read-only investigation subagent.
  - Specializes in surveying repository structures, reading code, searching symbols, and analyzing logs without workspace mutations.
- **`reviewer` (Review & Audit Agent)**:
  - Dedicated code review and security audit subagent.
  - Evaluates changes against project conventions, security posture, and test coverage.
- **Custom Configured Roles (`~/.codex/agents/*.toml`)**:
  - Codex supports user-defined custom agent roles with specific prompts, tool permissions, and dynamic nicknames.

### 2. Internal Engine Subagents (System Level)
Codex also runs automated background subagents:
- **`guardian_review`**: Validates security and permission policies before sensitive tool executions.
- **`memory_consolidation`**: Condenses conversation insights into persistent memory files in `~/.codex/memories`.
- **`thread_spawn` / `thread_compaction`**: Compresses earlier turns when conversations approach token limits.

---

## 🌐 Universal OpenAI-Compatible Support (Non-9Router)

`codex-9router-proxy` works universally with **any OpenAI-compatible provider**, both local and cloud-based. You are not locked to 9Router!

Use the built-in `-Preset` switch or enter custom endpoints during installation:

### 1. Ollama (Local)
```powershell
.\install.ps1 -Preset ollama -DefaultModel "qwen2.5-coder:32b"
```
Or manually configure in `~/.codex/config.toml`:
```toml
[model_providers.ollama]
name = "ollama"
base_url = "http://localhost:11434/v1"

[agents]
default_subagent_model = "qwen2.5-coder:32b"
```

### 2. LM Studio (Local)
```powershell
.\install.ps1 -Preset lmstudio -DefaultModel "qwen2.5-coder-32b-instruct"
```
Or manually in `~/.codex/config.toml`:
```toml
[model_providers.lmstudio]
name = "lmstudio"
base_url = "http://localhost:1234/v1"

[agents]
default_subagent_model = "qwen2.5-coder-32b-instruct"
```

### 3. OpenRouter (Cloud)
```powershell
.\install.ps1 -Preset openrouter -ApiKey "sk-or-v1-..." -DefaultModel "anthropic/claude-3.5-sonnet"
```
Or manually in `~/.codex/config.toml`:
```toml
[model_providers.openrouter]
name = "openrouter"
base_url = "https://openrouter.ai/api/v1"
env_key = "NINEROUTER_KEY"

[agents]
default_subagent_model = "anthropic/claude-3.5-sonnet"
```

### 4. vLLM / SGLang / LiteLLM
```powershell
.\install.ps1 -Preset vllm -DefaultModel "deepseek-ai/DeepSeek-V3"
```

---

## ⚙️ Customizing Models per Subagent Role

You can assign different models to each subagent role:

### Option A: Via `install.ps1`
The installer individually prompts for each role model:
```text
[?] Enter Default Subagent Model [default: 9router-subagent]: 9router-subagent
[?] Enter Worker Subagent Model [default: 9router-subagent]: 9router-subagent
[?] Enter Explorer Subagent Model [default: 9router-subagent]: 9router-subagent
[?] Enter Reviewer Subagent Model [default: 9router-subagent]: 9router-subagent
```

Or pass flags directly:
```powershell
.\install.ps1 -Provider "9router" `
  -DefaultModel "9router-subagent" `
  -WorkerModel "9router-subagent" `
  -ExplorerModel "9router-subagent" `
  -ReviewerModel "9router-subagent"
```

### Option B: On the Fly in `~/.codex/config.toml` (No Reinstall Required!)
The proxy dynamically reads `~/.codex/agents/*.toml` (or `[subagent_models]`) on every turn:
```toml
[subagent_models]
default = "9router-subagent"
worker = "ag/claude-sonnet-4-6"
explorer = "ag/gemini-3.8-flash-high"
reviewer = "ag/claude-opus-4-6-thinking"
```

### Option C: Per-Session Environment Overrides
```powershell
$env:CODEX_WORKER_MODEL = "ag/claude-sonnet-4-6"
$env:CODEX_EXPLORER_MODEL = "ag/gemini-3.8-flash-high"
```

---

## 🛡️ Out-of-the-Box "Always-Allow" & Stealth Browser CDP Automation (v0.2.8)

In stock Codex, subagents run under an automated `approval_policy = "never"`. When external tools (such as Playwright, CDP browser tabs, or external MCP servers) require human confirmation, subagents fail immediately with `"this session’s approval policy is never"`.

`install.ps1` automatically configures non-blocking automation out of the box across any new machine or user:

1. **Auto-Approve All Tools**:
   ```toml
   [apps._default]
   default_tools_approval_mode = "approve"
   ```
2. **Autonomous Reviewer**:
   ```toml
   approvals_reviewer = "auto_review"
   ```
   Routes boundary actions to an autonomous internal subagent rather than interrupting workflows.
3. **Full Browser CDP & History Control**:
   ```toml
   [browser_use]
   allow_history_access = true

   [browser_use.default_origin_policy]
   access = "allow"
   full_cdp_access = "allow"
   downloads = "allow"
   uploads = "allow"
   ```
4. **Native Chrome Extension Integration**:
   Codex claims and inspects browser tabs natively via the ChatGPT extension host (`com.openai.codexextension`), providing seamless tab access without requiring port 9222 or triggering remote debugging confirmation modals.
5. **Codex Plugin Namespace Bridging**:
   Transparently converts Codex plugin `"type": "namespace"` containers into `<namespace>__<tool_name>` functions for 9Router and dynamically rewrites streaming responses back to native Codex events.

---

## 🚀 Easy Step-by-Step Installation Guide (No Coding Required!)

You do **not** need to know how to code or install Rust to use `codex-9router-proxy`.

### Before You Begin (What You Need)
1. **Windows 10 or Windows 11** (64-bit).
2. The official **[OpenAI Codex Desktop App](https://apps.microsoft.com/detail/9mz1741s0917)** installed from the Microsoft Store and signed into your ChatGPT account.
3. **9Router** running locally on your PC (or another local/remote provider such as Ollama, LM Studio, or OpenRouter).
   - *If you use 9Router, make sure you have created a model/combo named **`9router-subagent`** (or your preferred model name) in your 9Router dashboard (`http://localhost:20128`).*

---

### Method 1: 1-Line PowerShell Quick Install (Easiest)
1. Press the **Windows Key**, type **`PowerShell`**, and click **Open**.
2. Copy and paste the following command into the PowerShell window and press **Enter**:

```powershell
$zip = "$env:TEMP\codex-9router-proxy.zip"; $dir = "$env:TEMP\codex-9router-proxy"; Invoke-RestMethod "https://github.com/ArchdukeViel/codex-9router-proxy/releases/latest/download/codex-9router-proxy-windows-amd64.zip" -OutFile $zip; Expand-Archive $zip -DestinationPath $dir -Force; powershell -ExecutionPolicy Bypass -File "$dir\install.ps1"
```

3. Press **Enter** at each prompt to accept the default settings (or type your API key when asked).

---

### Method 2: Download the `.zip` File (3 Simple Steps — No Rust Needed)

#### Step 1: Download & Extract
1. Go to the **[Releases Page](https://github.com/ArchdukeViel/codex-9router-proxy/releases/latest)**.
2. Click on **`codex-9router-proxy-windows-amd64.zip`** to download it.
3. Open your **Downloads** folder, **right-click** `codex-9router-proxy-windows-amd64.zip`, and click **Extract All...** $\rightarrow$ **Extract**.

#### Step 2: Open PowerShell in the Extracted Folder
1. Open the extracted `codex-9router-proxy-windows-amd64` folder (you will see `codex-9router-proxy.exe` and `install.ps1` inside).
2. **Right-click** on an empty space inside that folder and select **Open in Terminal** (or click the folder address bar at the top, type `powershell`, and press **Enter**).

#### Step 3: Run the Installer
1. Copy and paste this command into the window and press **Enter**:
   ```powershell
   powershell -ExecutionPolicy Bypass -File .\install.ps1
   ```
2. The installer will ask a few simple questions. **You can just press `Enter` on every question to use the recommended defaults**:
   - **Model Provider name**: Press **Enter** (uses `9router`).
   - **API Endpoint / Base URL**: Press **Enter** (uses `http://localhost:20128/v1`).
   - **API Key**: Paste your 9Router API key if you use one, or press **Enter** to keep your existing key.
   - **Default / Worker / Explorer / Reviewer Subagent Model**: Press **Enter** on each (uses `9router-subagent`).
3. Close and reopen the **Codex Desktop App** — you're done!

---

### Method 3: Build from Source (For Developers)
If you have [Git](https://git-scm.com/) and [Rust (`cargo`)](https://rustup.rs/) installed and prefer compiling from source:

```powershell
git clone https://github.com/ArchdukeViel/codex-9router-proxy.git
cd codex-9router-proxy
powershell -ExecutionPolicy Bypass -File .\install.ps1
```

Sample installer output:
```text
===================================================================
         Codex 9Router Proxy - Native GUI Subagents Installer      
===================================================================

[?] Enter Model Provider name [default: 9router]: 9router
[?] Enter API Endpoint / Base URL [default: http://localhost:20128/v1]: http://localhost:20128/v1
[?] Enter API Key [press Enter to keep existing key]: ********
[?] Enter Default Subagent Model [default: 9router-subagent]: 9router-subagent
[?] Enter Worker Subagent Model [default: 9router-subagent]: 9router-subagent
[?] Enter Explorer Subagent Model [default: 9router-subagent]: 9router-subagent
[?] Enter Reviewer Subagent Model [default: 9router-subagent]: 9router-subagent

[+] Target Configuration:
    Provider       : 9router
    Endpoint       : http://localhost:20128/v1
    API Key        : [PROTECTED / CONFIGURED]
    Default Model  : 9router-subagent
    Worker Model   : 9router-subagent
    Explorer Model : 9router-subagent
    Reviewer Model : 9router-subagent

[OK] Saved NINEROUTER_KEY to Windows User Environment (Registry HKCU\Environment).
[OK] Saved CODEX_CLI_PATH -> C:\Users\user\AppData\Local\OpenAI\Codex\custom\codex-9router-subagents.exe (User Environment).
[OK] Configured C:\Users\user\.codex\config.toml and role manifests in C:\Users\user\.codex\agents (BOM-Free UTF-8).
[OK] Injected SQLite subagent trigger into C:\Users\user\.codex\state_5.sqlite.
[OK] Deployed standalone shim to C:\Users\user\AppData\Local\OpenAI\Codex\custom\codex-9router-subagents.exe
[OK] Deployed standalone proxy copy to C:\Users\user\AppData\Local\OpenAI\Codex\custom\codex.exe
[OK] Registered self-healing startup hook in Codex9RouterHookSync.cmd.
[OK] Codex daemon restart dispatched successfully.
```

---

## 🩺 Diagnostics Doctor (`-Doctor`)

Run the built-in doctor check at any time to verify system health:

```powershell
.\install.ps1 -Doctor
```

Or directly via binary:
```powershell
codex --doctor
```

Output:
```text
===================================================================
           Codex 9Router Proxy - Diagnostics Doctor 🩺             
===================================================================

[OK] Running executable : C:\Users\user\codex-9router-proxy\codex-9router-proxy.exe
[OK] Official engine    : C:\Users\user\AppData\Local\OpenAI\Codex\custom\codex-9router-subagents.orig.exe (321969456 bytes)
[OK] Custom Shim Binary : C:\Users\user\AppData\Local\OpenAI\Codex\custom\codex-9router-subagents.exe (6220288 bytes)
[OK] CODEX_CLI_PATH     : C:\Users\user\AppData\Local\OpenAI\Codex\custom\codex-9router-subagents.exe (Verified)
[OK] Subagent Provider  : 9router
[OK] Provider API Key   : [CONFIGURED / MASKED]
[OK] Loopback TLS Cert  : Ready (C:\Users\user\AppData\Local\OpenAI\Codex\custom\bridge-cert.pem)
[OK] Reverse Proxy Port : 127.0.0.1:20129 (Port active / in use)
     - Loopback URL     : https://127.0.0.1:20129/backend-api/
     - Subagent Route   : Strict -> http://localhost:20128/v1/responses
     - Parent Route     : Upstream -> https://chatgpt.com/backend-api/
[OK] Codex Config TOML  : C:\Users\user\.codex\config.toml
     - Default Model    : 9router-subagent
     - Worker Model     : 9router-subagent
     - Explorer Model   : 9router-subagent
     - Reviewer Model   : 9router-subagent
[OK] Role Manifests     : C:\Users\user\.codex\agents
     - default  TOML   : Found
     - worker   TOML   : Found
     - explorer TOML   : Found
     - reviewer TOML   : Found
[OK] Endpoint socket reachable : localhost:20128 (127.0.0.1:20128)

===================================================================
Diagnostics complete. All checks finished.
===================================================================
```

---

## 🔄 Self-Healing Microsoft Store, CLI & VS Code / Antigravity Updates

Because `OpenAI.Codex` (`26.929.21022.0` `app.asar`) checks the exact byte size and SHA-256 of `%LOCALAPPDATA%\OpenAI\Codex\bin\<hash>\codex.exe` and deletes `bin\<hash>` if modified—while checking `process.env.CODEX_CLI_PATH` (`source=override`) first:
- `install.ps1` sets `CODEX_CLI_PATH = "%LOCALAPPDATA%\OpenAI\Codex\custom\codex-9router-subagents.exe"` in your Windows User environment (`HKCU\Environment`), prepends `%LOCALAPPDATA%\OpenAI\Codex\custom` to index `0` of your User `Path`, hooks standalone CLI (`%LOCALAPPDATA%\Programs\OpenAI\Codex\bin\codex.exe`) and VS Code / VS Code Insiders / Cursor / Windsurf / Antigravity (`openai.chatgpt-*`) extension binaries, and leaves `bin\<hash>\codex.exe` completely untouched as the stock Microsoft Store binary.
- Both `codex-9router-proxy.exe` (in-process on every startup and `--doctor`) and the Windows Startup Hook (`Codex9RouterHookSync.cmd` $\rightarrow$ `%LOCALAPPDATA%\OpenAI\Codex\custom\hook-sync.ps1`) automatically discover `OpenAI.Codex*` and split-runtime `OpenAI.CodexPrimaryRuntime*` packages via `HKCU\Software\Classes\Local Settings\Software\Microsoft\Windows\CurrentVersion\AppModel\Repository\Packages` (`PackageRootFolder`) and refresh `custom\codex.orig.exe`, `custom\codex-9router-subagents.orig.exe`, companion helpers (`codex-command-runner.exe`, `codex-windows-sandbox-setup.exe`, `codex-windows-sandbox-service.exe`, `codex-code-mode-host.exe`, `rg.exe`), and hooked CLI/extension shims whenever Microsoft Store, CLI, or extension updates occur.
- Every `app-server` and CLI invocation ensures a single-instance background `--proxy-daemon` holds `%LOCALAPPDATA%\OpenAI\Codex\custom\proxy-daemon-<port>.lock` and spawns a lightweight `250ms` local `TcpListener::bind` standby failover thread when `127.0.0.1:20129` is already bound—so closing the Codex Desktop app while keeping VS Code, Cursor, Windsurf, or Antigravity open triggers sub-250ms port takeover with zero stream interruption.

---

## 🧪 Verification

### 1. Test CLI Routing
```powershell
# Verify CLI returns official version
codex --version

# Verify main agent routes to ChatGPT
"ping" | codex exec --skip-git-repo-check "Say CHATGPT_OK in one word"

# Verify subagent routes to 9Router
"ping" | codex exec -m "9router-subagent" --skip-git-repo-check "Say 9ROUTER_OK in one word"
```

### 2. Test in Codex Desktop App
1. Open the **Codex Desktop App**.
2. Start a new chat and type:
   ```text
   delegate a test task to a subagent
   ```
3. Observe:
   - The purple diamond banner: **`◆ Subagent started working`** appears in the conversation.
   - A clickable child session thread appears in the left sidebar.
   - Click into the thread to watch live streaming thoughts, tool calls, and diffs!

---

## 📦 What's New in `v0.2.7`

- **Preserve User `"Fork from here"` (`thread/fork`) Main Conversations on ChatGPT (`is_subagent_turn_metadata_value`, `is_subagent_turn_metadata_str`, `contains_subagent_source`)**: Removes unconditional `forked_from_thread_id` / `forkedFromThreadId` matching from subagent detection so user-initiated conversation forks in Codex Desktop and VS Code extension remain on the signed-in ChatGPT account (`gpt-6-luna`), while forked subagents (carrying `parent_turn_id`, `parent_thread_id`, `subagent_kind`, or `/root/<role>`) continue routing strictly to 9Router.
- **Strict Subagent Role Whitelist & Prompt/Schema Exclusions (`is_subagent_role_name_in_dir`, `discover_configured_custom_roles_in_dir`, `find_agent_role`)**: Replaces the permissive `_ => true` catch-all in `is_subagent_role_name` with an explicit allowlist of built-in subagent roles (`worker`, `explorer`, `reviewer`, `default`, `implement`, `explore`, `review`, `subagent`, `9router-subagent`) plus custom roles discovered from `~/.codex/agents/*.toml` and `[agents.<role>]` in `config.toml`, and skips prompt/schema/history containers (`items`, `additionalContext`, `history`, `initialTurnsPage`, `dynamicTools`, `collaborationMode`, `codex_output_schema`, `text`, `schema`, `output_schema`, `response_format`) during recursive JSON role scanning so arbitrary user prompt text or JSON schemas containing `"role"` keys never hijack a main ChatGPT turn.
- **Exclude Internal Guardian Safety Classifier (`is_guardian_internal_subagent`) & Multi-Agents V2 Nickname Precedence (`extract_role_from_http_headers`, `infer_subagent_role_from_name_in_dir`)**: Explicitly excludes `guardian_classifier` and `guardian_review` across HTTP header inspection, JSON-RPC IPC, and SQLite triggers so Codex's internal safety classifier remains on ChatGPT, and evaluates `x-openai-subagent` (`explorer`, `worker`, `reviewer`) before `x-codex-turn-metadata` (`"agent_name": "/root/Gauss"`) so V2 nicknamed subagents preserve their specific role model.
- **Real ChatGPT Account Quota Usage Display (`sanitize_rate_limits`)**: Preserves real `primary.usedPercent` and `secondary.usedPercent` values in `/backend-api/wham/usage` and `/backend-api/codex/usage` responses while still populating a valid default `secondary` window when `secondary` is `null` and rewriting loopback `backendOrigin` back to `https://chatgpt.com`.
- **SQLite `AFTER UPDATE` Provider Enforcement Trigger (`fix_subagent_provider_update_trigger`) & `turn/steer` Inspection (`stdin_thread`)**: Adds an `AFTER UPDATE OF model_provider, model ON threads` trigger alongside `fix_subagent_provider_trigger` (`AFTER INSERT ON threads`) across all `state_*.sqlite` databases so `thread/resume` and `thread/settings/update` never revert subagent threads to `openai`, and inspects `turn/steer` and `turn/settings/update` JSON-RPC methods.
- **Subagent Compaction Hardening (`build_9router_compaction_request_body`, `build_compaction_sse_stream`, `handle_subagent_compaction`)**: Explicitly sets `"stream": false` on 9Router compaction summarization requests, generates globally unique `resp_9router_compact_<nanos>_<seq>` / `cmp_9router_compact_<nanos>_<seq>` IDs per compaction event, and bounds upstream compaction calls with a 120-second timeout before falling back to deterministic local summarization.
- **Cross-Surface Engine Unification, Periodic `--proxy-daemon` Sync, & IDE Extension Originator Guard (`pick_best_orig_candidate_across_dirs_with_min_size`, `find_real_codex`, `sync_hooked_surface_binaries_with_min_size`, `win_desktop::is_ide_extension_originator`)**: Compares all `> 10 MB` `.orig.exe` candidates across both `current_exe().parent()` and `%LOCALAPPDATA%\OpenAI\Codex\custom` by newest modification time (`(mtime, len)`), refreshes stale `codex.orig.exe` and companion helpers in hooked CLI/IDE extension directories, runs `sync_custom_codex_binaries()` + `sync_codex_models_cache()` every 15 seconds inside `--proxy-daemon`, and guards `heal_hidden_desktop_codex` when launched by VS Code / IDE extensions (`codex_vscode`).

---

## 🗑️ Uninstallation

To restore official stock binaries and remove scheduled tasks at any time:

```powershell
.\uninstall.ps1
```

---

## 📄 License

This project is licensed under the [MIT License](LICENSE).
