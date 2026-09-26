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
│   - Detects subagents (role = worker/explorer, nickname, etc.)  │
│   - Multi-tier dynamic model resolution:                        │
│       * worker   -> implement                                   │
│       * explorer -> explore                                     │
│       * reviewer -> review                                      │
│       * default  -> 9router-subagent                            │
│   - Injects model_provider = "9router"                          │
│   - Leaves parent turns on model_provider = "openai"            │
│   [Layer 2: Embedded Loopback Reverse Proxy (127.0.0.1:20129)]  │
│   - Intercepts chatgpt_base_url requests from codex.orig.exe    │
│   - Subagent responses -> 9Router (127.0.0.1:20128/v1/responses)│
│   - Parent turns -> ChatGPT Upstream (https://chatgpt.com)      │
└───────────────────────────────┬─────────────────────────────────┘
                                │ launches with -c chatgpt_base_url="http://127.0.0.1:20129/backend-api/"
                                ▼
┌─────────────────────────────────────────────────────────────────┐
│                  Official codex.orig.exe                        │
│   - Parent turns   -> Loopback :20129 -> https://chatgpt.com    │
│   - Subagent turns -> Loopback :20129 -> 9Router :20128         │
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
[?] Enter Worker Subagent Model [default: 9router-subagent]: implement
[?] Enter Explorer Subagent Model [default: 9router-subagent]: explore
[?] Enter Reviewer Subagent Model [default: 9router-subagent]: review
```

Or pass flags directly:
```powershell
.\install.ps1 -Provider "9router" `
  -DefaultModel "9router-subagent" `
  -WorkerModel "implement" `
  -ExplorerModel "explore" `
  -ReviewerModel "review"
```

### Option B: On the Fly in `~/.codex/config.toml` (No Reinstall Required!)
The proxy dynamically reads `[subagent_models]` from `~/.codex/config.toml` on every turn:
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

## 🚀 Quick Start

### 1. Prerequisites
- Windows 10/11
- [Rust & Cargo](https://rustup.rs/) (to compile from source, or download prebuilt release)
- 9Router, Ollama, LM Studio, or any OpenAI-compatible LLM endpoint
- Official [OpenAI Codex Desktop App](https://apps.microsoft.com/detail/9mz1741s0917) (Microsoft Store) or CLI

### 2. Interactive Installation
Run `install.ps1` in PowerShell:

```powershell
.\install.ps1
```

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

[OK] Running executable : C:\Users\user\codex-9router-proxy\target\release\codex-9router-proxy.exe
[OK] Official engine    : C:\Users\user\AppData\Local\OpenAI\Codex\bin\<bin-hash>\codex.orig.exe (321969456 bytes)
[OK] Subagent Provider  : 9router
[OK] Provider API Key   : [CONFIGURED / MASKED]
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

## 🗑️ Uninstallation

To restore official stock binaries and remove scheduled tasks at any time:

```powershell
.\uninstall.ps1
```

---

## 📄 License

This project is licensed under the [MIT License](LICENSE).
