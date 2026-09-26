# Codex 9Router Proxy ⚡

[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/Rust-1.80%2B-orange.svg)](https://www.rust-lang.org/)
[![Platform](https://img.shields.io/badge/Platform-Windows-0078D6.svg)](https://www.microsoft.com/windows)

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
│   - Detects subagents (role = worker/explorer or model = 9router)│
│   - Maps roles to specialized 9Router models:                   │
│       * worker   -> "implement"                                 │
│       * explorer -> "explore"                                   │
│       * default  -> "9router-subagent"                          │
│   - Injects model_provider = user-configured provider           │
│   - Injects API key from Windows Registry (HKCU) / Env          │
│   - Leaves parent turns on model_provider = "openai"            │
└───────────────────────────────┬─────────────────────────────────┘
                                │ Modified JSON-RPC
                                ▼
┌─────────────────────────────────────────────────────────────────┐
│                  Official codex.orig.exe                        │
│   - Parent turns   -> https://chatgpt.com (ChatGPT Cloud)       │
│   - Subagent turns -> User endpoint (e.g. 127.0.0.1:20128/v1)   │
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
  - Automatically mapped by the proxy to the `implement` model.
- **`explorer` (Research Agent)**:
  - Read-only investigation subagent.
  - Specializes in surveying repository structures, reading code, searching symbols, and analyzing logs without workspace mutations.
  - Automatically mapped by the proxy to the `explore` model.
- **Custom Configured Roles (`.codex/agents/*.toml`)**:
  - Codex supports user-defined custom agent roles with specific prompts, tool permissions, and dynamic nicknames (e.g. `reviewer`, `tester`).
  - Mapped by default to `9router-subagent` or role-specific models.

### 2. Internal Engine Subagents (System Level)
Codex also runs automated background subagents:
- **`guardian_review`**: Validates security and permission policies before sensitive tool executions.
- **`memory_consolidation`**: Condenses conversation insights into persistent memory files in `~/.codex/memories`.
- **`thread_spawn` / `thread_compaction`**: Compresses earlier turns when conversations approach token limits.

---

## 🚀 Quick Start

### 1. Prerequisites
- Windows 10/11
- [Rust & Cargo](https://rustup.rs/) (to compile from source)
- [9Router](http://127.0.0.1:20128) or any local OpenAI-compatible LLM endpoint running on your machine.
- Official [OpenAI Codex Desktop App](https://apps.microsoft.com/detail/9mz1741s0917) (Microsoft Store) or CLI.

### 2. Interactive Installation
Run `install.ps1` in PowerShell:

```powershell
.\install.ps1
```

The installer will interactively prompt you for:
1. **Model Provider**: Name of the provider (default: `9router`).
2. **Endpoint / Base URL**: API URL (default: `http://localhost:20128/v1`).
3. **API Key**: API key for your local/remote server (securely masked, press Enter to keep existing key).

```text
===================================================================
         Codex 9Router Proxy - Native GUI Subagents Installer      
===================================================================

[?] Enter Model Provider name [default: 9router]: 9router
[?] Enter API Endpoint / Base URL [default: http://localhost:20128/v1]: http://localhost:20128/v1
[?] Enter API Key [press Enter to keep existing key]: ********

[+] Target Configuration:
    Provider : 9router
    Endpoint : http://localhost:20128/v1
    API Key  : [PROTECTED / CONFIGURED]

[OK] Saved NINEROUTER_KEY to Windows User Environment (Registry HKCU\Environment).
[OK] Configured C:\Users\user\.codex\config.toml (BOM-Free UTF-8).
[OK] Backed up official binary to codex.orig.exe
[OK] Installed proxy hook to codex.exe
[OK] Registered self-healing logon task 'Codex9RouterHookSync'.
[OK] Codex daemon restarted successfully.
```

### 3. Non-Interactive / Automated Setup
For automated setups, pass flags directly:

```powershell
.\install.ps1 -Provider "9router" -Endpoint "http://localhost:20128/v1" -NonInteractive
```

---

## 🔄 Self-Healing Microsoft Store Updates

When the Microsoft Store auto-updates the Codex Desktop App, Windows downloads the new build to a brand new folder under:
`%LOCALAPPDATA%\OpenAI\Codex\bin\<new_hash>\`

The installer registers a lightweight Windows Scheduled Task named **`Codex9RouterHookSync`**.
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
