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

`codex-9router-proxy` `v0.2.3` is developed and verified against the following environment:

| Component | Verified Version | Notes |
| :--- | :--- | :--- |
| **Operating System** | **Windows 11 Pro** (`10.0.26200` / Build `26200`) | Compatible with Windows 10 & Windows 11 (`x86_64`) |
| **OpenAI Codex Desktop App** | **`26.924.1866.0`** ([Microsoft Store `OpenAI.Codex`](https://apps.microsoft.com/detail/9mz1741s0917)) | Auto-heals across Microsoft Store app updates |
| **OpenAI Codex CLI Engine** | **`codex-cli 0.158.0-alpha.2`** | Bundled Desktop engine & standalone CLI (`app-server` & `exec`) |
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
[OK] Configured C:\Users\user\.codex\config.toml and role manifests in C:\Users\user\.codex\agents (BOM-Free UTF-8).
[OK] Injected SQLite subagent trigger into C:\Users\user\.codex\state_5.sqlite.
[OK] Backed up official binary to codex.orig.exe
[OK] Installed proxy hook to codex.exe
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

==================================================================
Diagnostics complete. All checks finished.
===================================================================
```

---

## 🔄 Self-Healing Microsoft Store Updates

When the Microsoft Store auto-updates the Codex Desktop App, Windows downloads the new build to a brand new folder under:
`%LOCALAPPDATA%\OpenAI\Codex\bin\<new_hash>\`

The installer registers a lightweight Windows Startup Hook named **`Codex9RouterHookSync`**.
- Triggers seamlessly at user logon.
- Detects if an unhooked Microsoft Store binary is present.
- Safely preserves the official binary as `codex.orig.exe` and applies the proxy hook.
- You never have to manually re-run the installer after Store updates.

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

## 📦 What's New in `v0.2.3`

- **Main Agent Tool Preservation (`functions` & `collaboration`)**: Excluded message-level roles (`developer`, `user`, `assistant`, `system`, `tool`, `function`) and stopped recursing into `"input"` / `"messages"` / `"tools"` during role detection so Main Agent (`gpt-6-luna`) turns carrying `"type": "additional_tools"` are never misclassified as subagents.
- **Multi-Agents V2 Input Normalization (`agent_message` & `developer`)**: Normalizes Codex `multi_agents_v2` `"type": "agent_message"` items in `/v1/responses` `"input"` into `"type": "message"` with `"role": "user"` (stripping V2-only `author`, `recipient`, and `internal_chat_message_metadata_passthrough` fields) and normalizes `"role": "developer"` items into `"role": "system"` so 9Router and downstream Gemini/Claude/OpenAI translators always receive the delegated task prompt and role instructions.
- **Subagent Tool Sanitization (`9Router` Compatibility)**: Strips unsupported `"type": "additional_tools"` input items and `"type": "namespace"` / `"type": "web_search"` / `"type": "custom"` (`Freeform`) tool definitions before forwarding subagent turns to 9Router.
- **Automatic Model Metadata Injection (`GET /backend-api/models`)**: Injects model metadata descriptors for `9router-subagent` (and configured role models) with a `200,000` token context window and `apply_patch_tool_type = "function"`, eliminating `Model metadata for ... not found` warnings and `apply_patch` payload mismatches.
- **Desktop Usage & Billing UI + `Plus` Badge**: Rewrites `workspaceRouting.backendOrigin` in `account/read` JSON-RPC responses from `https://127.0.0.1:20129` back to `https://chatgpt.com` so the Electron Desktop App's `AuthService` (`electron.net.fetch`) loads `/wham/usage` and subscription details directly from `https://chatgpt.com`.
- **Safe Windows Runtime Helper Sync**: Separates `OpenAI\Codex\bin\<hash>` helpers from `rg.exe` so Electron's ripgrep hash cleanup (`Tn`) never purges `codex-code-mode-host.exe`.

---

## 🗑️ Uninstallation

To restore official stock binaries and remove scheduled tasks at any time:

```powershell
.\uninstall.ps1
```

---

## 📄 License

This project is licensed under the [MIT License](LICENSE).
