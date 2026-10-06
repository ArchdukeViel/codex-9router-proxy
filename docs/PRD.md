# Product Requirements Document — Codex 9Router Proxy & Pool Gateway

> **Version**: `0.4.1` · **Last Updated**: 2026-10-04 (rev.5 — Singleflight Streaming Coalescing, Proactive Rate-Limit Smoothing, AuthError Handling, Internal Auth) · **Status**: Living Document  
> **Author**: ArchdukeViel · **License**: MIT

---

## Table of Contents

- [1. Overview](#1-overview)
- [2. Problem Statement](#2-problem-statement)
- [3. Goals & Non-Goals](#3-goals--non-goals)
- [4. Target Users & Supported Surfaces](#4-target-users--supported-surfaces)
- [5. System Architecture](#5-system-architecture)
  - [5.1 Three-Layer Architecture](#51-three-layer-architecture)
  - [5.2 Request Routing Decision Tree](#52-request-routing-decision-tree)
  - [5.3 Process Lifecycle](#53-process-lifecycle)
- [6. Feature Specifications](#6-feature-specifications)
  - **Codex Interception Proxy (Layers 1 & 2)**
  - [6.1 JSON-RPC IPC Interception (Layer 1)](#61-json-rpc-ipc-interception-layer-1)
  - [6.2 Embedded HTTPS/HTTP2 Reverse Proxy (Layer 2)](#62-embedded-httpshttp2-reverse-proxy-layer-2)
  - [6.3 Subagent Detection & Classification](#63-subagent-detection--classification)
  - [6.4 Model & Provider Resolution](#64-model--provider-resolution)
  - [6.5 Request Sanitization & Compatibility](#65-request-sanitization--compatibility)
  - [6.6 SSE Stream Transformation](#66-sse-stream-transformation)
  - [6.7 Subagent Remote Compaction](#67-subagent-remote-compaction)
  - [6.8 Model Metadata Injection](#68-model-metadata-injection)
  - [6.9 Binary Discovery & Multi-Surface Self-Healing](#69-binary-discovery--multi-surface-self-healing)
  - [6.10 Zero-Downtime Standby Failover](#610-zero-downtime-standby-failover)
  - [6.11 Windows Desktop Healing](#611-windows-desktop-healing)
  - [6.12 Diagnostics Doctor](#612-diagnostics-doctor)
  - [6.13 TLS Certificate Management](#613-tls-certificate-management)
  - [6.14 Always-Allow Policies & Autonomous Execution](#614-always-allow-policies--autonomous-execution)
  - [6.15 Codex Plugin Namespace Bridging](#615-codex-plugin-namespace-bridging)
  - [6.16 Multi-Agent V2 Protocol Support](#616-multi-agent-v2-protocol-support)
  - [6.17 Browser CDP Automation Support](#617-browser-cdp-automation-support)
  - [6.18 NVIDIA NIM Model Catalog](#618-nvidia-nim-direct-provider-support)
  - **Provider Pool Gateway (Layer 3)**
  - [6.19 Provider Pool Gateway Engine](#619-provider-pool-gateway-engine)
  - [6.20 API Key Management & Authentication](#620-api-key-management--authentication)
  - [6.21 Usage Analytics & Cost Tracking](#621-usage-analytics--cost-tracking)
  - [6.22 Quota Tracker & Account Health Manager](#622-quota-tracker--account-health-manager)
  - **Operational Controls**
  - [6.23 Proxy Passthrough Toggle](#623-proxy-passthrough-toggle)
  - [6.24 System Tray Controller](#624-system-tray-controller)
  - [6.25 Request Deduplication & Response Cache](#625-request-deduplication--response-cache)
- [7. Configuration Reference](#7-configuration-reference)
  - [7.1 Environment Variables](#71-environment-variables)
  - [7.2 Configuration Files](#72-configuration-files)
  - [7.3 SQLite Triggers](#73-sqlite-triggers)
- [8. HTTP Endpoints](#8-http-endpoints)
- [9. Installation & Deployment](#9-installation--deployment)
  - [9.1 Installer (`install.ps1`)](#91-installer-installps1)
  - [9.2 Uninstaller (`uninstall.ps1`)](#92-uninstaller-uninstallps1)
  - [9.3 CI/CD Pipeline](#93-cicd-pipeline)
  - [9.4 Build from Source](#94-build-from-source)
- [10. Compatibility Matrix](#10-compatibility-matrix)
- [11. Security Considerations](#11-security-considerations)
- [12. Testing](#12-testing)
- [13. Future Considerations](#13-future-considerations)
  - [F-1 Web Dashboard / Admin UI](#f-1-web-dashboard--admin-ui)
    - [F-1.1 Information Architecture & Navigation](#f-11-information-architecture--navigation)
    - [F-1.2 Page: Live Feed](#f-12-page-live-feed-default-landing)
    - [F-1.3 Page: Subagents](#f-13-page-subagents)
    - [F-1.4 Page: Providers & Accounts](#f-14-page-providers--accounts)
    - [F-1.5 Page: Usage & Analytics](#f-15-page-usage--analytics)
    - [F-1.6 Page: API Keys](#f-16-page-api-keys)
    - [F-1.7 Page: Combos](#f-17-page-combos)
    - [F-1.8 Page: Configuration](#f-18-page-configuration)
    - [F-1.9 Page: Doctor](#f-19-page-doctor)
    - [F-1.10 Dashboard UI/UX Flow Summary](#f-110-dashboard-uiux-flow-summary)
  - [F-2 Provider-Specific Prompt Optimization](#f-2-provider-specific-prompt-optimization)
  - [F-3 Proxy Self-Update](#f-3-proxy-self-update)
- [Appendix A: Glossary](#appendix-a-glossary)

---

## 1. Overview

**Codex 9Router Proxy** (`codex-9router-proxy`) is a single-binary Rust application for Windows that serves two complementary roles:

1. **Codex Interception Proxy** — A transparent proxy bridge that hooks the official **OpenAI Codex Desktop App** and **Codex CLI** engine, intercepting and routing child **subagent threads** (`spawn_agent` / `collaboration.spawn_agent`) to custom LLM providers while preserving the primary chat session on **ChatGPT** (`gpt-6-luna`) with zero additional latency.

2. **Provider Pool Gateway** — An OpenAI-compatible API gateway on `:20130` that pools multiple upstream LLM provider accounts (Antigravity, NVIDIA NIM, OpenRouter, Poolside, Ollama Cloud, and optionally 9Router), with smart routing, self-issued API keys, real-time usage analytics, cost tracking, and automatic quota management. Any client that speaks the OpenAI API (Codex, Claude Code, Cursor, Cline, Copilot, Antigravity) can connect.

The proxy is a single self-contained Rust binary (~6.2 MB) requiring no external dependencies, no Node.js, and no separate 9Router instance.

### Key Value Proposition

| Without Proxy | With Proxy |
|:---|:---|
| `HTTP 400: The model is not supported when using Codex with a ChatGPT account` | ✅ Subagents route seamlessly to pooled providers |
| Depends on external 9Router Node.js service | ✅ Self-contained — single binary IS the router |
| MCP delegation renders as inline cards (`Worked for 1m 7s >`) | ✅ Native purple diamond `◆ Subagent started working` in timeline |
| No sidebar entry for subagent work | ✅ Clickable child session in left sidebar |
| No live streaming of subagent activity | ✅ Real-time thought, tool call, and file diff streaming |
| Single provider, single API key | ✅ Pool multiple providers, multiple accounts per provider, auto-failover |
| No visibility into subagent costs/usage | ✅ Real-time usage analytics, cost tracking, quota management |
| Manual API key management | ✅ Self-issued keys with rate limits, expiry, IP whitelists |

---

## 2. Problem Statement

### P-1: Subagent Model Rejection

When the Codex Desktop App is signed into a ChatGPT Plus/Pro account, invoking `spawn_agent` with any non-OpenAI model (e.g., `9router-subagent`) triggers:

```
HTTP 400: The '9router-subagent' model is not supported when using Codex with a ChatGPT account.
```

OpenAI's cloud backend validates the model name and rejects anything outside their known set.

### P-2: Degraded MCP Delegation UX

Prior workarounds using MCP-based delegation tools (`subagent_9router:delegate_to_9router`) execute as synchronous tool calls. These:

- Render only as small collapsed inline cards in the main thread
- Do **not** appear in the native sidebar conversation list
- Lack real-time streaming visibility
- Block the parent thread during execution

### P-3: Multi-Surface Fragmentation

The Codex engine runs across multiple surfaces — Microsoft Store app, standalone CLI, VS Code, VS Code Insiders, Cursor, Windsurf, and Antigravity IDE extensions. Keeping proxy hooks synchronized across all surfaces and surviving automatic Store/CLI updates requires automated self-healing.

---

## 3. Goals & Non-Goals

### Goals

| ID | Goal |
|:---|:---|
| **G-1** | Route subagent threads to any OpenAI-compatible provider without modifying Codex source |
| **G-2** | Preserve 100% native GUI experience (sidebar, purple diamond, live streaming) |
| **G-3** | Zero latency impact on primary ChatGPT sessions |
| **G-4** | Zero-touch operation after installation — survive Store updates, CLI updates, IDE updates |
| **G-5** | Support per-role model customization (worker → model A, explorer → model B) |
| **G-6** | Support all Codex surfaces: Desktop App, CLI, VS Code, Cursor, Windsurf, Antigravity |
| **G-7** | Transparent to Codex — no API key exposure, no credential modification |
| **G-8** | Preserve ChatGPT billing/usage UI accuracy |
| **G-9** | Clean install and uninstall with full rollback capability |
| **G-10** | Pool multiple upstream providers with smart adaptive routing and auto-failover |
| **G-11** | Issue self-managed API keys with per-key rate limits, expiry, combo assignment, and IP whitelists |
| **G-12** | Track real-time usage analytics: token consumption, cost estimation, latency, and error rates |
| **G-13** | Auto-detect and manage provider account quotas — disable exhausted accounts, re-enable after reset |
| **G-14** | Serve as a universal OpenAI-compatible API gateway usable by any AI coding client (not just Codex) |
| **G-15** | Eliminate dependency on external 9Router Node.js service — single binary does everything |
| **G-16** | Hot-reload all configuration changes without restarting the proxy |
| **G-17** | Allow instant ON/OFF toggling of proxy interception — subagents seamlessly fall back to ChatGPT's official model in passthrough mode |
| **G-18** | Provide a Windows system tray icon for at-a-glance status and quick proxy control |

### Non-Goals

| ID | Non-Goal |
|:---|:---|
| **NG-1** | Linux or macOS support (Windows-only by design, leverages Win32 APIs) |
| **NG-2** | Modifying or patching the Codex Desktop Electron app binary |
| **NG-3** | Replacing the primary ChatGPT model for the main conversation |
| **NG-4** | Multi-tenant SaaS deployment — this is a personal/team tool on localhost |

---

## 4. Target Users & Supported Surfaces

### Users

- **Primary**: Developers using OpenAI Codex Desktop App or CLI on Windows with a ChatGPT Plus/Pro account who want to delegate subagent work to custom or local models.
- **Secondary**: Power users running multi-agent coding workflows across multiple IDEs simultaneously.
- **Gateway Users**: Developers using any OpenAI-compatible AI coding client (Claude Code, Cursor, Cline, Copilot, Antigravity) who want a unified local gateway to pool multiple LLM providers with smart routing, cost tracking, and quota management.

### Supported Surfaces

| Surface | Hook Method | Binary Location |
|:---|:---|:---|
| **Codex Desktop App** (Microsoft Store) | `CODEX_CLI_PATH` override + stdio IPC interception | `WindowsApps\OpenAI.Codex*\app\resources` |
| **Codex CLI** (standalone) | PATH prepend + shim replacement | `Programs\OpenAI\Codex\bin` |
| **VS Code Extension** | Binary shim in extension `bin/` | `.vscode\extensions\openai.chatgpt-*` |
| **VS Code Insiders** | Binary shim in extension `bin/` | `.vscode-insiders\extensions\openai.chatgpt-*` |
| **Cursor Extension** | Binary shim in extension `bin/` | `.cursor\extensions\openai.chatgpt-*` |
| **Windsurf Extension** | Binary shim in extension `bin/` | `.windsurf\extensions\openai.chatgpt-*` |
| **Antigravity IDE** | Binary shim in extension `bin/` | `.antigravity\extensions\openai.chatgpt-*` |

---

## 5. System Architecture

### 5.1 Three-Layer Architecture

The proxy operates as three cooperating layers within a single binary:

```mermaid
flowchart TD
    A["Codex Desktop App<br/>(Electron GUI)"] -->|"JSON-RPC stdio<br/>thread/start, turn/start"| B["codex-9router-proxy.exe"]

    subgraph PROXY["codex-9router-proxy.exe"]
        direction TB
        L1["Layer 1: JSON-RPC IPC Interception<br/>• Detect subagent threads<br/>• Resolve role → model<br/>• Rewrite modelProvider<br/>• Rewrite backendOrigin → chatgpt.com"]
        L2["Layer 2: Embedded HTTPS Reverse Proxy<br/>127.0.0.1:20129<br/>• Self-signed TLS (CODEX_CA_CERTIFICATE)<br/>• Reject WebSocket → HTTP 426<br/>• Request sanitization<br/>• SSE stream transformation"]
        L3["Layer 3: Provider Pool Gateway<br/>127.0.0.1:20130<br/>• OpenAI-compatible /v1/ API<br/>• Smart adaptive routing<br/>• Self-issued API keys<br/>• Usage analytics & quota tracking<br/>• Web Dashboard"]
        L1 --> L2
        L2 -->|"Subagent turns"| L3
    end

    B -->|"Piped stdio"| C["codex.orig.exe<br/>(Official Codex Engine)"]
    C -->|"HTTPS POST<br/>/backend-api/codex/responses"| L2

    L2 -->|"Parent turns<br/>(gpt-6-luna)"| D["https://chatgpt.com<br/>(ChatGPT Backend)"]

    L3 -->|"Provider pool<br/>smart routing"| POOL["Upstream Providers"]

    subgraph POOL["Provider Pool"]
        NV["NVIDIA NIM<br/>integrate.api.nvidia.com"]
        AG["Antigravity<br/>Gemini models"]
        OR["OpenRouter<br/>openrouter.ai"]
        PS["Poolside<br/>coding models"]
        OC["Ollama Cloud"]
        NR["9Router (legacy)<br/>localhost:20128"]
    end

    EXT["External Clients<br/>(Claude Code, Cursor,<br/>Cline, Copilot, etc.)"] -->|"OpenAI API<br/>Bearer pool-sk-..."| L3

    style PROXY fill:#1a1a2e,stroke:#16213e,color:#e0e0e0
    style L1 fill:#0f3460,stroke:#533483,color:#e0e0e0
    style L2 fill:#533483,stroke:#e94560,color:#e0e0e0
    style L3 fill:#e94560,stroke:#fdcb6e,color:#fff
    style POOL fill:#2d3436,stroke:#636e72,color:#dfe6e9
    style EXT fill:#00b894,stroke:#55efc4,color:#fff
```

**Layer 1 — JSON-RPC IPC Interception** intercepts the stdio stream between the Electron frontend and `codex.orig.exe`. It operates on JSON-RPC methods (`thread/start`, `thread/fork`, `thread/resume`, `thread/settings/update`, `turn/start`, `turn/steer`, `turn/settings/update`) to detect subagent threads and rewrite their `modelProvider` and `model` fields before the engine processes them.

**Layer 2 — Embedded HTTPS/HTTP2 Reverse Proxy** runs on `127.0.0.1:20129` and intercepts the actual HTTP requests that `codex.orig.exe` makes to what it believes is `chatgpt.com`. For subagent turns, the proxy forwards the request to the Pool Gateway (Layer 3), and transforms the SSE response stream back into Codex-compatible format. Parent turns pass through transparently to ChatGPT.

**Layer 3 — Provider Pool Gateway** runs on `127.0.0.1:20130` and serves as a standalone OpenAI-compatible API gateway. It accepts requests from both Layer 2 (Codex subagents) and external clients (Claude Code, Cursor, etc.), authenticates via self-issued `pool-sk-*` keys, scores and selects the best upstream provider account, translates between API formats, tracks usage/cost, and manages provider quotas.

### 5.2 Request Routing Decision Tree

```mermaid
flowchart TD
    REQ["Inbound Request<br/>→ 127.0.0.1:20129"] --> HEALTH{"/health or<br/>/__codex_9router_proxy_healthz?"}
    HEALTH -->|Yes| H200["200 OK"]
    HEALTH -->|No| WSCHECK{"GET /responses?<br/>(WebSocket upgrade)"}
    WSCHECK -->|Yes| H426["426 Upgrade Required<br/>(force HTTPS POST)"]
    WSCHECK -->|No| MODELS{"GET /models?"}
    MODELS -->|Yes| MODELINJECT["Forward → ChatGPT<br/>Inject subagent model metadata<br/>Return enriched JSON"]
    MODELS -->|No| POST{"POST /responses?"}
    POST -->|No| FALLBACK["Forward → chatgpt.com<br/>(transparent passthrough)"]
    POST -->|Yes| GUARDIAN{"Guardian classifier?<br/>(guardian_classifier,<br/>guardian_review)"}
    GUARDIAN -->|Yes| CHATGPT["Forward → ChatGPT<br/>(keep on account)"]
    GUARDIAN -->|No| SUBAGENT{"Is subagent turn?"}
    SUBAGENT -->|No| CHATGPT
    SUBAGENT -->|Yes| COMPACT{"Is compaction<br/>request?"}
    COMPACT -->|Yes| COMPACTION["Handle compaction<br/>→ Pool Gateway summarization<br/>or local deterministic summary<br/>→ Return SSE stream"]
    COMPACT -->|No| ROUTE["Resolve role → model<br/>Sanitize request<br/>Forward → Pool Gateway (:20130)<br/>Transform SSE response"]

    style REQ fill:#2d3436,stroke:#636e72,color:#dfe6e9
    style ROUTE fill:#6c5ce7,stroke:#a29bfe,color:#fff
    style CHATGPT fill:#00b894,stroke:#55efc4,color:#fff
    style COMPACTION fill:#fdcb6e,stroke:#f39c12,color:#2d3436
    style H426 fill:#d63031,stroke:#ff7675,color:#fff
```

### 5.3 Process Lifecycle

```mermaid
stateDiagram-v2
    [*] --> ParseArgs: main()

    ParseArgs --> Doctor: --doctor / --proxy-status
    ParseArgs --> ProxyDaemon: --proxy-daemon
    ParseArgs --> LightCLI: --version / --help
    ParseArgs --> CLIMode: codex exec / codex ...
    ParseArgs --> DesktopMode: app-server

    Doctor --> RunDoctor: run_doctor()
    RunDoctor --> [*]

    ProxyDaemon --> AcquireLock: Single-instance lock
    AcquireLock --> BindProxy: Bind :20129
    BindProxy --> SyncLoop: 15s sync loop
    SyncLoop --> SyncLoop

    LightCLI --> ForwardDirect: Forward to codex.orig.exe
    ForwardDirect --> [*]

    CLIMode --> SyncBinaries: sync_custom_codex_binaries()
    SyncBinaries --> SpawnProxy: Spawn reverse proxy :20129
    SpawnProxy --> InjectEnv: Inject CODEX_CA_CERTIFICATE + chatgpt_base_url
    InjectEnv --> SpawnCodex: Spawn codex.orig.exe
    SpawnCodex --> WaitExit: Wait for exit
    WaitExit --> [*]

    DesktopMode --> HealDesktop: heal_hidden_desktop_codex()
    HealDesktop --> SyncBinaries2: sync_custom_codex_binaries()
    SyncBinaries2 --> SpawnProxy2: Spawn reverse proxy :20129 (+ standby)
    SpawnProxy2 --> InjectEnv2: Inject env vars
    InjectEnv2 --> SpawnCodexPiped: Spawn codex.orig.exe (piped stdio)
    SpawnCodexPiped --> IOThreads: Launch I/O threads

    state IOThreads {
        STDIN: Thread 1 (STDIN)\nIntercept JSON-RPC requests\nroute_thread_params()
        STDOUT: Thread 2 (STDOUT)\nIntercept JSON-RPC responses\nsanitize_rate_limits()
    }
```

---

## 6. Feature Specifications

### 6.1 JSON-RPC IPC Interception (Layer 1)

**Purpose**: Intercept and rewrite subagent thread configurations before the Codex engine processes them.

**Intercepted Methods**:

| JSON-RPC Method | Action |
|:---|:---|
| `thread/start` | Detect new subagent threads, rewrite `modelProvider` and `model` |
| `thread/fork` | Detect forked subagent threads (preserve user-initiated forks on ChatGPT) |
| `thread/resume` | Rewrite resumed subagent threads |
| `thread/settings/update` | Rewrite subagent settings changes |
| `turn/start` | Rewrite subagent turn model/provider |
| `turn/steer` | Rewrite mid-turn model/provider overrides |
| `turn/settings/update` | Rewrite turn settings changes |

**Behavior**:
- Reads JSON-RPC messages line-by-line from stdin
- Parses `params` to identify subagent threads via role, metadata, and thread source
- Rewrites `modelProvider` to the target provider (e.g., `9router`)
- Rewrites `model` to the resolved model for the subagent's role
- Rewrites `backendOrigin` to `https://chatgpt.com` so billing/usage UI renders correctly
- Forwards the modified message to `codex.orig.exe`'s stdin

**Stdout Interception**:
- Intercepts JSON-RPC responses from `codex.orig.exe`
- Runs `sanitize_rate_limits` to rewrite loopback URLs back to `https://chatgpt.com` in usage/billing responses
- Preserves real `primary.usedPercent` and `secondary.usedPercent` values

### 6.2 Embedded HTTPS/HTTP2 Reverse Proxy (Layer 2)

**Purpose**: Intercept HTTP requests from `codex.orig.exe` and route subagent turns to the correct provider.

**Technical Details**:
- Binds on `127.0.0.1:20129` (configurable via `CODEX_PROXY_PORT`)
- Dual-stack HTTP/1.1 and HTTP/2 via `hyper-util` auto connection builder
- Self-signed TLS certificate generated at runtime via `rcgen`
- TLS termination via `tokio-rustls` (pure Rust, no OpenSSL dependency)
- Asynchronous request handling via `axum` + `tokio`
- Zstandard request body decompression (`Content-Encoding: zstd`)
- Connection timeout: 10 seconds with single automatic retry on transient errors
- Full `Error::source()` chain in 502 responses for self-explanatory diagnostics

### 6.3 Subagent Detection & Classification

The proxy uses a multi-signal detection system to distinguish subagent turns from primary turns.

**Detection Signals** (evaluated in order):

1. **HTTP Headers**: `x-openai-subagent`, `x-codex-parent-thread-id`, `x-codex-turn-metadata`
2. **JSON Body Fields**: `thread_source`, `subagent_kind`, `parent_thread_id`, `parent_turn_id`
3. **Model Name**: `9router-subagent` or custom models in `[subagent_models]`
4. **Role Matching**: Built-in roles (`worker`, `explorer`, `reviewer`, `default`, `implement`, `explore`, `review`, `subagent`, `compact`, `memory_consolidation`) plus custom roles from `~/.codex/agents/*.toml`

**Explicit Exclusions**:
- **Guardian classifiers**: `guardian_classifier`, `guardian_review` — always stay on ChatGPT
- **Standard OpenAI message roles**: `system`, `developer`, `user`, `assistant`, `tool`, `function` — never treated as subagent role names
- **User-initiated forks**: `thread/fork` without child subagent markers stays on ChatGPT
- **Prompt/schema containers**: `items`, `additionalContext`, `history`, `text`, `schema`, `output_schema` — skipped during recursive JSON role scanning

**Multi-Agent V2 Precedence**:
- `x-openai-subagent` header (explicit role: `explorer`, `worker`, `reviewer`) evaluated **before** `x-codex-turn-metadata` (`agent_name: /root/Gauss`)
- `/root/<nickname>` normalized to `default` when no explicit role header present

### 6.4 Model & Provider Resolution

**Resolution Cascade** (highest to lowest priority):

```
1. Per-role environment variable override
   (CODEX_WORKER_MODEL, CODEX_EXPLORER_MODEL, CODEX_REVIEWER_MODEL)
       ↓
2. Per-role agent manifest
   (~/.codex/agents/<role>.toml → model, model_provider, base_url, env_key)
       ↓
3. config.toml [subagent_models] section
   (default = "...", worker = "...", explorer = "...", reviewer = "...")
       ↓
4. config.toml default_subagent_model
       ↓
5. CODEX_DEFAULT_MODEL environment variable
       ↓
6. Hardcoded default: "9router-subagent"
```

**Provider Resolution**:
- Provider name derived from `model_provider` in agent TOML or `config.toml [model_providers.<name>]`
- Endpoint derived from provider's `base_url` (default: `http://localhost:20128/v1`)
- API key derived from provider's `env_key` (default: `NINEROUTER_KEY`)

### 6.5 Request Sanitization & Compatibility

Before forwarding subagent requests to 9Router, the proxy performs extensive sanitization:

| Sanitization | Purpose |
|:---|:---|
| Strip `access_programs`, `codex_output_schema`, `compaction_trigger`, `configuration_update` | Remove Codex-internal fields unknown to providers |
| Strip `store`, `stream_options`, `service_tier`, `prompt_cache_key` | Remove ChatGPT-specific fields |
| Normalize `"type": "agent_message"` → `"type": "message"` with `"role": "user"` | Multi-Agent V2 compatibility |
| Normalize `"role": "developer"` → `"role": "system"` | Standard OpenAI API compliance |
| Strip `collaboration.spawn_agent` from subagent tools | Prevent recursive delegation runaway |
| Inject `exec_command` worker tools | Ensure subagents can execute shell commands |
| Unflatten `"type": "namespace"` → `<namespace>__<tool_name>` functions | Plugin compatibility with non-Codex providers |
| Strip namespace-only tool stubs (no `name` field) | Remove unregistered tool declarations |
| Rehydrate compaction/context items into `"type": "message"` | Preserve conversation context across compactions |
| Convert `custom_tool_call`, `tool_search_call`, `image_generation_call` → message items | Handle forked ChatGPT history items |

**Gemini/Vertex AI OpenAPI Schema Sanitization** (`sanitize_tool_parameters_for_subagents`):
- Inject empty `properties: {}` for bare `"type": "object"` definitions
- Inject `items` for bare `"type": "array"` definitions
- Normalize shorthand primitive property strings (e.g., `"headers": "object"` → `"headers": {"type": "object", "properties": {}}`)
- Recursive traversal of all tool parameter schemas

### 6.6 SSE Stream Transformation

The proxy transforms Server-Sent Event streams from 9Router back into Codex-compatible format:

| Transform | Function |
|:---|:---|
| Namespace tool bridging | `transform_response_text_for_namespace_tools` — rewrites `<namespace>__<tool_name>` back to `{"name": "tool_name", "namespace": "namespace"}` |
| Final answer phase injection | `ensure_subagent_message_final_answer_phase` — ensures `"phase": "final_answer"` on assistant messages |
| Fallback final answer synthesis | `ensure_fallback_final_answer_in_output` — synthesizes final answer when model terminates without assistant text |
| Unsupported tool mapping | `map_subagent_unsupported_tool_call_to_exec_command` — maps `browser_tabs`, `cua_repl__js` etc. to `exec_command` |

**Implementation**: Custom `SubagentSseTransformStream` built on `futures-util` stream combinators, operating on SSE chunks in real-time without buffering the full response.

### 6.7 Subagent Remote Compaction

**Purpose**: Handle subagent context compaction (summarization) without routing to ChatGPT.

**Detection**:
- JSON body: `"generate": false`, `"request_kind": "compaction"`
- HTTP header: `x-codex-turn-metadata` with `"request_kind": "compaction"`, `"subagent_kind": "compact"`, or `"compaction": {...}`

**Behavior**:
1. Strip compaction-irrelevant fields (`tools`, `tool_choice`, `parallel_tool_calls`, `include`, `service_tier`)
2. Normalize tool-call/output and reasoning history items into `"type": "message"`
3. Append summarization instruction if not present
4. Forward to 9Router with `"stream": false` and 120-second timeout
5. Extract assistant summary from 9Router response
6. **Fallback**: If 9Router fails/times out, generate deterministic local summary from input items
7. Return valid Responses API SSE stream: `response.created` → `response.output_item.done` (with `"type": "compaction"`, `"encrypted_content"`) → `response.completed`

**ID Generation**: Globally unique IDs per compaction event: `resp_9router_compact_<nanos>_<seq>`, `cmp_9router_compact_<nanos>_<seq>`

### 6.8 Model Metadata Injection

**Purpose**: Inject subagent model metadata into `/backend-api/codex/models` responses so Codex recognizes custom models.

**Injected Metadata** (for `9router-subagent`):
- `context_window`: 872,000
- `max_context_window`: 872,000
- `effective_context_window_percent`: 95
- `default_reasoning_level`: `high`
- Supported reasoning efforts: `low`, `medium`, `high`, `xhigh`, `max`
- `prefer_websockets`: `false`
- `use_responses_lite`: `true`
- `comp_hash`: `"3000"`
- Tool support: `parallel`
- `shell_type`: `"default"`

**Sync Targets**:
- Live `GET /backend-api/codex/models` responses (in-flight injection)
- `~/.codex/models_cache.json` (on-disk sync via atomic `.tmp.<pid>` + `fs::rename`)
- Synced at: startup, `--doctor`, installation, and every 15s via `--proxy-daemon`

### 6.9 Binary Discovery & Multi-Surface Self-Healing

**Purpose**: Automatically discover, validate, and synchronize Codex binaries across all surfaces.

**Discovery Sources** (in priority order):
1. **Microsoft Store packages**: `HKCU\...\AppModel\Repository\Packages` for `OpenAI.Codex*` and `OpenAI.CodexPrimaryRuntime*`
2. **Desktop bin relocation**: `%LOCALAPPDATA%\OpenAI\Codex\bin\<hash>`
3. **Standalone CLI**: `%LOCALAPPDATA%\Programs\OpenAI\Codex\bin`
4. **IDE extensions**: VS Code, VS Code Insiders, Cursor, Windsurf, Antigravity (`openai.chatgpt-*` under respective config dirs)

**Health Validation** (`is_healthy_codex_binary`):
- PE header sanity check (`MZ` magic bytes)
- Execute `--version` and verify successful exit
- Reject corrupted binaries that would crash with `0xc0000005`

**Synchronized Binaries**:
- `custom\codex-9router-subagents.orig.exe` — latest stock engine
- `custom\codex.orig.exe` — latest stock engine (companion copy)
- `codex-command-runner.exe`, `codex-windows-sandbox-setup.exe`, `codex-windows-sandbox-service.exe`, `codex-code-mode-host.exe`, `rg.exe` — companion helpers

**File Integrity**:
- `LastWriteTimeUtc` timestamps preserved across copies
- Selection criteria: `> 10 MB` stock binaries, sorted by newest modification time

**Daemon Sync**: `--proxy-daemon` runs `sync_custom_codex_binaries()` + `sync_codex_models_cache()` every 15 seconds.

### 6.10 Zero-Downtime Standby Failover

**Purpose**: Ensure uninterrupted proxy availability across Codex process restarts and multi-surface concurrent usage.

**Mechanism**:
- When `:20129` is already bound by a healthy proxy instance (verified via TLS health probe to `/health`), spawn a lightweight 250ms poll standby thread (`TcpListener::bind` loop)
- If the active process exits or becomes unhealthy, standby takes over in sub-250ms
- Prevents `os error 10061` disconnects in VS Code/Cursor/Windsurf/Antigravity extensions when the Desktop app closes

**Single-Instance Daemon Lock**:
- OS file lock: `%LOCALAPPDATA%\OpenAI\Codex\custom\proxy-daemon-<port>.lock` with `share_mode(0)`
- Ensures only one `--proxy-daemon` instance runs per port

### 6.11 Windows Desktop Healing

**Purpose**: Detect and recover from Codex instances stranded on hidden Windows sandbox desktops.

**Win32 APIs Used**:
- `EnumDesktopsW` — enumerate desktops in `WinSta0`
- `EnumDesktopWindows` — find windows on each desktop
- `CreateToolhelp32Snapshot` / `Process32First` / `Process32Next` — snapshot running processes
- `QueryFullProcessImageNameW` — resolve NTFS image paths

**Behavior**:
- Detect `ChatGPT.exe` instances on hidden `exebox-*` desktops
- Terminate them while preserving active `WinSta0\Default` windows
- Clean stale singleton lockfiles
- **IDE Protection**: Inspect ancestor process chain for IDE hosts (`Code.exe`, `Cursor.exe`, `Windsurf.exe`, `Antigravity.exe`) — never heal/terminate when launched from an IDE

### 6.12 Diagnostics Doctor

**Invocation**: `codex --doctor`, `codex --proxy-status`, `install.ps1 -Doctor`

**Checks Performed**:

| Check | Description |
|:---|:---|
| Binary hooks | Verify `codex-9router-subagents.exe` shim is in place |
| `codex.orig.exe` | Verify stock engine binary exists and is healthy |
| Environment variables | Verify `CODEX_CLI_PATH`, `CODEX_CA_CERTIFICATE`, `NINEROUTER_KEY` presence |
| TLS certificates | Verify `bridge-cert.pem` and `bridge-key.pem` exist |
| Loopback proxy | Verify `:20129` is bound and healthy |
| Config files | Verify `config.toml` provider configuration |
| Role manifests | Verify `agents/*.toml` files and model mappings |
| SQLite triggers | Verify INSERT and UPDATE triggers on `state_*.sqlite` |
| Network reachability | Verify 9Router `/v1/models` endpoint is reachable |
| HKCU registry | Verify `CODEX_CLI_PATH` persistence in Windows User Environment |

### 6.13 TLS Certificate Management

- Self-signed certificate and key generated at runtime via `rcgen`
- Stored at `%LOCALAPPDATA%\OpenAI\Codex\custom\bridge-cert.pem` and `bridge-key.pem`
- Injected into Codex via `CODEX_CA_CERTIFICATE` environment variable
- Certificate is for loopback only (`127.0.0.1`) — never exposed externally
- Regenerated if files are missing or corrupted

### 6.14 Always-Allow Policies & Autonomous Execution

**Purpose**: Configure Codex for fully autonomous subagent execution without approval prompts.

**Configured Policies** (via `install.ps1`):
- `approvals_reviewer = "auto_review"`
- `[apps._default] default_tools_approval_mode = "approve"`
- `[approval_policy.granular]` — per-tool exemptions
- `[browser_use.default_origin_policy]` — full CDP, download, upload permissions
- MCP server `default_tools_approval_mode = "approve"` across all configured servers

### 6.15 Codex Plugin Namespace Bridging

**Problem**: Codex plugins declare tools with `"type": "namespace"` containers. 9Router and most LLM providers expect flat `"type": "function"` declarations.

**Solution**:
- **Outbound** (`sanitize_subagent_request_for_9router`): Flatten `{ "type": "namespace", "name": "ns", "functions": [{ "name": "tool" }] }` → `{ "type": "function", "name": "ns__tool" }`
- **Inbound** (`transform_response_text_for_namespace_tools`): Rewrite `"ns__tool"` in SSE function call events → `{ "name": "tool", "namespace": "ns" }` for Codex dispatch
- Double-underscore `__` separator convention

### 6.16 Multi-Agent V2 Protocol Support

- Normalize `"type": "agent_message"` items → `"type": "message"` with `"role": "user"`
- Strip V2-only fields: `author`, `recipient`, `internal_chat_message_metadata_passthrough`
- Normalize `"role": "developer"` → `"role": "system"`
- Inject `"phase": "final_answer"` on assistant output messages
- Synthesize fallback `final_answer` when models terminate without explicit assistant text
- Evaluate `x-openai-subagent` header before `x-codex-turn-metadata` for V2 nickname precedence

### 6.17 Browser CDP Automation Support

- Auto-detect system Chrome or Edge across 64-bit, 32-bit, and per-user LocalAppData paths
- Set `AGENT_BROWSER_EXECUTABLE_PATH` environment variable
- Map unsupported browser tool calls (`browser_tabs`, `cua_repl__js`) to `exec_command` with the detected browser path
- Configure full CDP, download, and upload permissions in `[browser_use.default_origin_policy]`

### 6.18 NVIDIA NIM Direct Provider Support

**Purpose**: First-class integration with NVIDIA's hosted NIM API (`https://integrate.api.nvidia.com/v1`) as a direct subagent provider, enabling access to frontier open-weight models without self-hosting.

**API Compatibility**: OpenAI-compatible (`/v1/chat/completions`). Authentication via `Authorization: Bearer nvapi-...` header. Environment variable: `NVIDIA_API_KEY`.

**Installer Preset**: `nvidia`

```toml
[model_providers.nvidia]
name = "nvidia"
base_url = "https://integrate.api.nvidia.com/v1"
env_key = "NVIDIA_API_KEY"
```

#### 6.18.1 Supported Models

| Model | API ID | Publisher | Architecture | Total Params | Active Params | Context Window | Max Output | Input Modality | Reasoning | Tool Calling | Best For |
|:---|:---|:---|:---|:---|:---|:---|:---|:---|:---|:---|:---|
| **DeepSeek-V4.1-Flash** | `deepseek-ai/deepseek-v4.1-flash` | DeepSeek | MoE (552B) | 552B | 8B prefill / 16B decode | 1,048,576 | 262,144 (up to 1M) | Text + Image (≤2 imgs) | ✅ Controllable (1–100) | ✅ | Cost-efficient multimodal reasoning, coding, document understanding |
| **GLM-5.3-Flash** | `z-ai/glm-5.3-flash` | Z.ai | MoE (320B) + Hybrid KDA/Sparse | 320B | 18B | 1,048,576 | 1,024 (must override) | Text + Image (≤8 imgs) | ✅ (`low`/`high`/`max`) | ✅ | High-throughput cost-sensitive serving, visual QA, agentic coding |
| **GLM-5.3** | `z-ai/glm-5.3` | Z.ai | MoE (753B) + DSA Sparse Attention | 753B | ~40B | 1,048,576 | 1,024 (must override) | Text only | ✅ (`low`/`high`/`max`) | ✅ | Complex coding, repo-scale refactoring, long-horizon agentic work, security |
| **Kimi-K3** | `moonshotai/kimi-k3` | Moonshot AI | MoE (2.8T) + KDA + LatentMoE | 2.8T | 104B | 1,048,576 | 16,384 (up to 65,536) | Text + Image (≤5 imgs) | ✅ Always-on (`low`/`high`/`max`) | ✅ | Long-horizon software engineering, agentic knowledge work, multimodal |

> [!IMPORTANT]
> **API ID format**: GLM models use dots in the API ID (`z-ai/glm-5.3-flash`, `z-ai/glm-5.3`) while the NVIDIA URL slugs use dashes (`glm-5-3-flash`). The proxy must use the dotted API IDs in request payloads.

> [!WARNING]
> **GLM default `max_tokens` is only 1,024**. The proxy MUST explicitly set `max_tokens` to a reasonable value (e.g., 32,768) for GLM models, otherwise subagent responses will be severely truncated.

**Rate Limits** (NVIDIA evaluation tier — shared pool):
- 40 requests per minute (RPM)
- 10,000 requests per day (RPD)
- Trial accounts receive 1,000 free credits; exhaustion returns HTTP 402

#### 6.18.2 Per-Model Role Mapping Strategy

Models have different strengths. The recommended default role mapping for the NVIDIA provider:

```toml
# ~/.codex/config.toml
[subagent_models]
default = "deepseek-ai/deepseek-v4.1-flash"       # Fast, cheap, good all-rounder
worker  = "moonshotai/kimi-k3"                     # Heavy-duty implementation with deep reasoning
explorer = "z-ai/glm-5.3-flash"                    # Fast multimodal research, cost-efficient
reviewer = "z-ai/glm-5.3"                          # Deep code review, security audit, vulnerability discovery
```

**Rationale**:
- **DeepSeek-V4.1-Flash as default**: Smallest active params (8B), lowest cost, still 1M context — ideal for quick delegation tasks
- **Kimi-K3 as worker**: Largest model (2.8T/104B active), always-on reasoning, best for complex multi-step implementation
- **GLM-5.3-Flash as explorer**: Multimodal (can read screenshots/diagrams), hybrid attention for fast long-context scanning, ~1/10th the cost of full GLM
- **GLM-5.3 as reviewer**: Strongest at coding benchmarks (Terminal Bench 3.0 SOTA, CyberGym leader), text-only but deepest code understanding

#### 6.18.3 Provider-Specific Request Optimization

The proxy must apply NVIDIA-specific request transformations when the target provider is `nvidia`:

| Optimization | Description |
|:---|:---|
| **Model ID format** | Use full qualified IDs with dots (`deepseek-ai/deepseek-v4.1-flash`, `z-ai/glm-5.3-flash`) — NVIDIA NIM requires publisher-prefixed model names |
| **Reasoning effort mapping** | Map Codex reasoning levels to model-native formats: DeepSeek uses numeric `reasoning_effort` (1–100, default 100), Kimi-K3 uses enum `reasoning_effort` (`low`/`high`/`max`), GLM uses enum `reasoning_effort` (`low`/`high`/`max`) |
| **Tool schema compliance** | Apply existing Gemini schema sanitization (§6.5) — NVIDIA models served via vLLM share similar OpenAPI strictness. Schema enforces `additionalProperties: false` — strip all unknown fields. |
| **Vision input format** | For multimodal models: convert image payloads to `{"type": "image_url", "image_url": {"url": "data:image/jpeg;base64,..."}}` format. Image limits: V4.1-Flash (≤2), GLM-5.3-Flash (≤8), Kimi-K3 (≤5). GLM-5.3 is text-only. |
| **Max output tokens** | **CRITICAL**: Set explicit `max_tokens` per model — defaults are too low for subagent work. V4.1-Flash: 262,144 (good default), GLM-5.3-Flash: override to 32,768 (default is only 1,024!), GLM-5.3: override to 32,768, Kimi-K3: 16,384 (up to 65,536) |
| **Content-Type** | Ensure `Content-Type: application/json` (no zstd compression — NVIDIA NIM does not accept zstd) |
| **Strip ChatGPT fields** | Remove `previous_response_id`, `store`, `service_tier`, `stream_options` — NVIDIA NIM rejects unknown fields (`additionalProperties: false`) |
| **Kimi-K3 reasoning history** | **CRITICAL**: Kimi-K3 was trained with preserved thinking history. In multi-turn conversations, the proxy MUST pass back the complete previous assistant message including `reasoning_content` and `tool_calls`. Omitting `reasoning_content` degrades coherence. |
| **Kimi-K3 async polling** | Kimi-K3 may return HTTP `202 Accepted` with `NVCF-REQID` header for long-running queries. The proxy must poll `GET /v1/status/{requestId}` until `200 OK` is returned. |
| **GLM `clear_thinking`** | For GLM models in chat/tool-calling scenarios, set `clear_thinking=true` parameter to separate reasoning from final answer |
| **GLM sampling constraints** | GLM docs warn: do not modify both `temperature` and `top_p` simultaneously. Default `temperature: 0.5`, `top_p: 1.0` |
| **DeepSeek temperature** | DeepSeek recommends `temperature: 1.0` and `top_p: 0.95` — do not lower from defaults |
| **Kimi-K3 fixed sampling** | Kimi-K3 fixes `top_p`, `presence_penalty`, `frequency_penalty`, and `n` internally — do not send these parameters |
| **Response format** | Use `/v1/chat/completions` (Chat Completions API) instead of `/v1/responses` (Responses API) — NVIDIA NIM does not implement the Responses API. The proxy must translate between formats (see §6.18.4). |

#### 6.18.4 Chat Completions ↔ Responses API Translation

Since NVIDIA NIM exposes only the Chat Completions API (`/v1/chat/completions`) while Codex speaks the Responses API, the proxy must translate:

**Outbound (Codex → NVIDIA)**:
1. Convert `input` array items → `messages` array
2. Map `"type": "message"` → `{"role": "...", "content": "..."}`
3. Map `"type": "function_call"` → `{"role": "assistant", "tool_calls": [...]}`
4. Map `"type": "function_call_output"` → `{"role": "tool", "content": "...", "tool_call_id": "..."}`
5. Convert `tools` from Responses format to Chat Completions format
6. Map `"reasoning": {"effort": "..."}` → model-native reasoning parameter

**Inbound (NVIDIA → Codex)**:
1. Convert Chat Completions SSE chunks (`chat.completion.chunk`) → Responses API SSE events (`response.output_item.added`, `response.output_text.delta`, `response.output_item.done`, `response.completed`)
2. Map `choices[0].delta.tool_calls` → `"type": "function_call"` output items
3. Map `choices[0].delta.content` → `"type": "message"` output items with `"phase": "final_answer"`
4. Synthesize `response.created` and `response.completed` envelope events
5. Generate Responses API-compatible `id` fields (`resp_nvidia_<nanos>_<seq>`)

#### 6.18.5 NVIDIA Model Metadata Injection

Extend `inject_subagent_models_metadata` (§6.8) to register all four NVIDIA models in `models_cache.json` and `/backend-api/codex/models`:

```json
{
  "deepseek-ai/deepseek-v4.1-flash": {
    "context_window": 1048576,
    "max_context_window": 1048576,
    "default_max_tokens": 262144,
    "default_reasoning_level": "medium",
    "supported_reasoning_efforts": ["low", "medium", "high"],
    "prefer_websockets": false,
    "capabilities": ["text", "image", "tool_calls", "reasoning"]
  },
  "z-ai/glm-5.3-flash": {
    "context_window": 1048576,
    "max_context_window": 1048576,
    "default_max_tokens": 32768,
    "default_reasoning_level": "medium",
    "supported_reasoning_efforts": ["low", "medium", "high"],
    "prefer_websockets": false,
    "capabilities": ["text", "image", "tool_calls", "reasoning"]
  },
  "z-ai/glm-5.3": {
    "context_window": 1048576,
    "max_context_window": 1048576,
    "default_max_tokens": 32768,
    "default_reasoning_level": "high",
    "supported_reasoning_efforts": ["low", "medium", "high", "max"],
    "prefer_websockets": false,
    "capabilities": ["text", "tool_calls", "reasoning"]
  },
  "moonshotai/kimi-k3": {
    "context_window": 1048576,
    "max_context_window": 1048576,
    "default_max_tokens": 16384,
    "max_max_tokens": 65536,
    "default_reasoning_level": "high",
    "supported_reasoning_efforts": ["low", "high", "max"],
    "prefer_websockets": false,
    "capabilities": ["text", "image", "tool_calls", "reasoning"],
    "requires_reasoning_content_passback": true
  }
}
```

---

### 6.19 Provider Pool Gateway Engine

**Purpose**: Replace external 9Router dependency with a built-in OpenAI-compatible API gateway that pools multiple upstream LLM provider accounts with smart adaptive routing.

**Endpoint**: `http://127.0.0.1:20130` (configurable via `POOL_GATEWAY_PORT`)

**Exposed API Paths**:
- `POST /v1/chat/completions` — Chat Completions API (Claude Code, Cursor, Cline, etc.)
- `POST /v1/responses` — Responses API (Codex subagents)
- `GET /v1/models` — List available combos and models
- `GET /dashboard/` — Web Dashboard UI (static embedded assets)
- `WS /dashboard/ws` — WebSocket for real-time dashboard events
- `GET/POST/PUT/DELETE /api/*` — Dashboard management API (localhost only)

#### 6.19.1 Provider Registry

Pluggable providers, each implementing a common routing trait:

| Provider | Preset | Endpoint | API Format | Auth |
|:---|:---|:---|:---|:---|
| **Antigravity** | `antigravity` | `https://api.antigravity.dev/v1` | Chat Completions | `Bearer` |
| **NVIDIA NIM** | `nvidia` | `https://integrate.api.nvidia.com/v1` | Chat Completions | `Bearer nvapi-*` |
| **OpenRouter** | `openrouter` | `https://openrouter.ai/api/v1` | Chat Completions | `Bearer` |
| **Poolside** | `poolside` | `https://api.poolside.ai/v1` | Chat Completions | `Bearer` |
| **Ollama Cloud** | `ollama-cloud` | *(user-configured)* | Chat Completions | `Bearer` |
| **9Router** (legacy) | `9router` | `http://localhost:20128/v1` | Responses / Chat | `Bearer` |

Each provider can have **multiple accounts** (multiple API keys). Accounts are configured in `pool.toml` (§6.22).

#### 6.19.2 Smart Adaptive Routing

When a request arrives, the gateway scores all eligible accounts and picks the best one:

```
score(account) = w_health × health_score
               + w_quota × quota_remaining_pct
               + w_latency × (1 / avg_latency_ms)
               + w_cost × (1 / cost_per_token)
```

**Default Weights**:
```toml
[routing.weights]
health = 0.4     # Account health is most important
quota = 0.3      # Prefer accounts with remaining quota
latency = 0.2    # Prefer faster-responding accounts
cost = 0.1       # Prefer cheaper accounts (when comparable)
```

**Scoring Rules**:
- `health_score`: `1.0` for Healthy, `0.0` for Unhealthy/Disabled, `0.3` for Cooldown (degraded)
- `quota_remaining_pct`: Estimated from `X-RateLimit-Remaining` headers (default `1.0` if unknown)
- `avg_latency_ms`: Exponential moving average (EMA) over last 20 requests per account
- `cost_per_token`: From provider pricing table (§6.21)
- Accounts scoring below `min_score_threshold` (default `0.1`) are excluded
- Ties broken by least-recently-used timestamp

**Context-Length Aware Routing**:
Before scoring, the gateway estimates the request's total token count (messages + system prompt + tool definitions) and **disqualifies** any model whose `max_context_tokens` is smaller than the estimate. This prevents routing a 200K-token request to a model with 128K context.

- Token estimation uses a fast byte-based heuristic (`bytes / 4` for English text)
- If ALL models in a combo are disqualified by context length, the gateway returns an error with a compaction hint
- If `auto_compact = true` in `pool.toml`, the gateway automatically triggers §6.7 Subagent Remote Compaction, then retries the request with the reduced context

**Thread Affinity (Prompt Cache Exploitation)**:
To maximize prompt cache hit rates, the gateway tracks which **specific account** was used for the first turn of each subagent thread (by `thread_id`):

- Subsequent turns in the same thread receive a scoring bonus for the original account: `affinity_bonus = 0.15` (configurable)
- This keeps multi-turn conversations on the same provider/account, maximizing cached prefix token discounts (up to 90% on some providers)
- Affinity is **reset** when the thread undergoes compaction (§6.7), since the message history changes
- If the affinity account is unhealthy or rate-limited, the gateway falls back to normal scoring (cache benefit < availability)

```toml
[routing]
thread_affinity_bonus = 0.15    # Scoring bonus for same-account in same thread
auto_compact = true             # Auto-trigger compaction when context exceeds all models
```

**Multi-Key Auto-Striping**:
When multiple accounts exist for the same provider (e.g., `nvidia-1`, `nvidia-2`, `nvidia-3`), the scoring algorithm naturally distributes requests across them:

- Accounts with more `X-RateLimit-Remaining` score higher (higher `quota_remaining_pct`)
- As nvidia-1 approaches its 40 RPM limit, nvidia-2 and nvidia-3 are preferred
- 3 NVIDIA free accounts effectively provide 120 RPM combined (3× the single-account limit)
- The gateway logs effective multiplied rate limits in the Dashboard for visibility

#### 6.19.3 Combo System

Named model aliases with per-combo routing strategy:

```toml
[[combos]]
name = "fast-coder"
strategy = "roundrobin"  # Rotate through healthy models
models = [
  { provider = "nvidia", model = "deepseek-ai/deepseek-v4.1-flash", priority = 1 },
  { provider = "openrouter", model = "deepseek/deepseek-chat-v4.1-flash", priority = 2 },
  { provider = "antigravity", model = "gemini-2.5-flash", priority = 3 },
]

[[combos]]
name = "heavy-worker"
strategy = "fallback"  # Try in priority order until one succeeds
models = [
  { provider = "nvidia", model = "moonshotai/kimi-k3", priority = 1 },
  { provider = "openrouter", model = "moonshotai/kimi-k3", priority = 2 },
]

[[combos]]
name = "code-reviewer"
strategy = "adaptive"  # Use smart scoring within this combo's models
models = [
  { provider = "nvidia", model = "z-ai/glm-5.3", priority = 1 },
  { provider = "openrouter", model = "z-ai/glm-5.3", priority = 2 },
]

[[combos]]
name = "9router-subagent"  # backward compat — same model name as 9Router
strategy = "fallback"
models = [
  { provider = "nvidia", model = "deepseek-ai/deepseek-v4.1-flash", priority = 1 },
  { provider = "9router", model = "9router-subagent", priority = 2 },
]
```

**Combo Strategies**:

| Strategy | Behavior |
|:---|:---|
| `fallback` | Try models in priority order until one responds successfully |
| `roundrobin` | Rotate through healthy models evenly, skip unhealthy ones |
| `adaptive` | Use the smart scoring algorithm (§6.19.2) to pick the best account within this combo |

**Resolution**: When a client requests `model: "fast-coder"`, the gateway:
1. Looks up the combo by name
2. Filters to accounts that are healthy and enabled
3. Applies the combo's strategy to pick a provider+model+account triple
4. Translates the request to the provider's API format if needed
5. Forwards and translates the response back

#### 6.19.4 API Format Translation

The gateway accepts BOTH Chat Completions and Responses API requests, auto-detected from the request path. When the upstream provider uses a different format, the gateway translates bidirectionally:

```mermaid
flowchart TD
    CLIENT["Client Request"] --> DETECT{"Request Path?"}
    DETECT -->|"/v1/chat/completions"| CC["Chat Completions<br/>format"]
    DETECT -->|"/v1/responses"| RESP["Responses API<br/>format"]

    CC --> ROUTE["Pick Best Account<br/>(combo + scoring)"]
    RESP --> ROUTE

    ROUTE --> UPSTREAM{"Upstream<br/>Provider Format?"}

    UPSTREAM -->|"Chat Completions<br/>(NVIDIA, OpenRouter,<br/>Antigravity, Poolside)"| FWD_CC["Forward as<br/>Chat Completions"]
    UPSTREAM -->|"Responses API<br/>(9Router)"| FWD_RESP["Forward as<br/>Responses API"]

    FWD_CC --> TRANSLATE_BACK{"Client<br/>expects?"}
    FWD_RESP --> TRANSLATE_BACK

    TRANSLATE_BACK -->|"Chat Completions"| RET_CC["Return Chat<br/>Completions SSE"]
    TRANSLATE_BACK -->|"Responses API"| RET_RESP["Return Responses<br/>API SSE"]

    style ROUTE fill:#6c5ce7,stroke:#a29bfe,color:#fff
    style CLIENT fill:#00b894,stroke:#55efc4,color:#fff
```

**Translation details**: See §6.18.4 for the complete Chat Completions ↔ Responses API mapping specification.

#### 6.19.5 Hot-Reload

Configuration changes are applied in two tiers depending on which component owns the setting:

**Tier 1 — Instant (same Codex session, next request)**:

Changes detected via OS file watcher and applied without restart. These take effect on the next incoming request.

| Config File | Setting | Applies To |
|:---|:---|:---|
| `pool.toml` | Combos (add/remove/edit) | Pool Gateway routing |
| `pool.toml` | Accounts (add/remove/enable/disable) | Pool Gateway routing |
| `pool.toml` | API keys (add/revoke) | Pool Gateway auth |
| `pool.toml` | Routing weights | Pool Gateway scoring |
| `pool.toml` | Provider pricing | Cost estimation |
| `pool.toml` | Proxy toggle (`proxy_enabled`) | Passthrough mode |
| `pool.toml` | Passthrough model | Subagent model in passthrough |
| `pool.toml` | Thread affinity bonus | Routing optimization |

**Tier 2 — On Codex Restart (next session)**:

These settings are read by `codex.orig.exe` at startup. Changes are persisted immediately but take effect when Codex is restarted.

| Config File | Setting | Reason |
|:---|:---|:---|
| `config.toml` | Role → model mapping | Codex engine caches thread settings |
| `config.toml` | Approval policies | Codex engine reads once at startup |
| `agents/*.toml` | Per-role agent manifests | Codex engine reads once at startup |
| `models_cache.json` | Model metadata catalog | Codex engine reads once at startup |
| SQLite triggers | Thread provider enforcement | Applied on DB open |

**Implementation**:
- **File watcher**: Windows `ReadDirectoryChangesW` via `notify` crate
- **Debounce**: 500ms after last change before applying
- **Atomic swap**: New config parsed and validated first, then swapped atomically via `Arc::swap`
- **Validation**: Invalid config changes are rejected with a log warning; last valid config remains active
- **Non-reloadable**: Port changes (`CODEX_PROXY_PORT`, `POOL_GATEWAY_PORT`) and TLS cert paths require full restart

---

### 6.20 API Key Management & Authentication

**Purpose**: The gateway issues its own API keys so clients can authenticate against `localhost:20130/v1` without exposing upstream provider credentials.

#### 6.20.1 Key Format

```
pool-sk-<32-hex-chars>
```
Example: `pool-sk-a1b2c3d4e5f67890a1b2c3d4e5f67890`

Generated via cryptographically secure random bytes (`ring::rand::SystemRandom`).

#### 6.20.2 Key Properties

| Property | Type | Default | Description |
|:---|:---|:---|:---|
| `id` | `string` | Auto-generated UUID | Unique key identifier |
| `key` | `string` | Auto-generated | The bearer token (`pool-sk-...`) |
| `label` | `string` | `"default"` | Human-readable name (e.g., "codex-desktop", "cursor-work") |
| `default_combo` | `string` | `"9router-subagent"` | Default combo when model name not recognized as a combo |
| `allowed_combos` | `string[]` | `[]` (all) | Whitelist of accessible combos (empty = unrestricted) |
| `format_hint` | `enum` | `"auto"` | `"auto"`, `"chat"`, `"responses"` — preferred format for analytics grouping |
| `rate_limit_rpm` | `int?` | `null` | Per-key requests per minute (null = unlimited) |
| `rate_limit_rpd` | `int?` | `null` | Per-key requests per day (null = unlimited) |
| `expires_at` | `datetime?` | `null` | Key auto-disables after this time (null = never) |
| `ip_whitelist` | `string[]?` | `null` | Allowed source IPs (null = any; `["127.0.0.1", "::1"]` for local-only) |
| `enabled` | `bool` | `true` | Enable/disable toggle |
| `created_at` | `datetime` | Now | Creation timestamp |

#### 6.20.3 Key Generation

**Via CLI**:
```powershell
codex-9router-proxy --generate-key --label "cursor" --combo "fast-coder" --rpm 100
# Output: pool-sk-a1b2c3d4e5f67890a1b2c3d4e5f67890
```

**Via Dashboard**: "API Keys" page → "Generate New Key" button → fill form → copy key.

**Via Config** (add directly to `pool.toml`):
```toml
[[keys]]
id = "key-codex-desktop"
label = "Codex Desktop"
default_combo = "9router-subagent"
format_hint = "responses"
ip_whitelist = ["127.0.0.1", "::1"]
enabled = true
```

The `key` value is auto-generated on first load if not present.

**Reserved Internal Key (`pool-sk-internal-codex`)**:
When Layer 2 intercepts Codex subagents and forwards them to Layer 3 (:20130), it authenticates with a reserved internal key: `Authorization: Bearer pool-sk-internal-codex`.
- Auto-provisioned in memory on boot (no manual generation needed)
- Strictly bound to `ip_whitelist = ["127.0.0.1", "::1"]` (external network requests rejected)
- `format_hint = "responses"`
- Subagent requests appear in SQLite analytics and the Dashboard under the distinct label `"codex-internal"` for clean usage isolation alongside external clients (Cursor, Claude Code)

#### 6.20.4 Authentication Flow

```mermaid
flowchart TD
    REQ["Client sends<br/>Authorization: Bearer pool-sk-..."] --> LOOKUP{"Key exists?"}
    LOOKUP -->|No| R401["401 Unauthorized"]
    LOOKUP -->|Yes| ENABLED{"Key enabled?"}
    ENABLED -->|No| R403E["403 Key Disabled"]
    ENABLED -->|Yes| EXPIRED{"Key expired?"}
    EXPIRED -->|Yes| R403X["403 Key Expired"]
    EXPIRED -->|No| IPCHECK{"IP in whitelist?"}
    IPCHECK -->|No| R403IP["403 IP Not Allowed"]
    IPCHECK -->|Yes| RATE{"Rate limit<br/>exceeded?"}
    RATE -->|Yes| R429["429 Too Many Requests<br/>Retry-After header"]
    RATE -->|No| RESOLVE["Resolve combo<br/>Route request"]

    style RESOLVE fill:#00b894,stroke:#55efc4,color:#fff
    style R401 fill:#d63031,stroke:#ff7675,color:#fff
```

**Rate Limiter**: Sliding window counter using in-memory `DashMap<KeyId, SlidingWindow>`. Window size: 1 minute for RPM, 1 day for RPD.

#### 6.20.5 Key Storage Security

Upstream provider API keys and self-issued key secrets are stored **AES-256-GCM encrypted** in `pool.toml`:

```toml
[encryption]
method = "aes-256-gcm"
# Master key derived from Windows DPAPI (passwordless) or user password
# DPAPI ties encryption to the Windows user account — keys are unreadable
# by other users or if the disk is mounted elsewhere

[[accounts]]
id = "nvidia-1"
provider = "nvidia"
api_key_encrypted = "ENC[AES256:U2FsdGVkX1+ABC123...==]"
endpoint = "https://integrate.api.nvidia.com/v1"
enabled = true
```

**First-time setup**:
```powershell
codex-9router-proxy --init-pool
# Interactive: configures encryption, adds first accounts, generates first key
```

---

### 6.21 Usage Analytics & Cost Tracking

**Purpose**: Log all gateway requests to an embedded SQLite database for usage analytics, cost estimation, and operational visibility.

#### 6.21.1 Database

**Location**: `%LOCALAPPDATA%\OpenAI\Codex\custom\pool-analytics.sqlite`

**Schema**:

```sql
CREATE TABLE requests (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    timestamp       TEXT NOT NULL,           -- ISO 8601
    api_key_id      TEXT NOT NULL,           -- FK to issued key
    api_key_label   TEXT NOT NULL,           -- Denormalized for fast queries
    combo_name      TEXT NOT NULL,           -- Resolved combo
    provider        TEXT NOT NULL,           -- e.g., "nvidia", "openrouter"
    account_id      TEXT NOT NULL,           -- Specific account used
    model           TEXT NOT NULL,           -- Actual upstream model ID
    format          TEXT NOT NULL,           -- "chat" or "responses"
    input_tokens    INTEGER NOT NULL DEFAULT 0,
    output_tokens   INTEGER NOT NULL DEFAULT 0,
    reasoning_tokens INTEGER NOT NULL DEFAULT 0,
    latency_ms      INTEGER NOT NULL DEFAULT 0,
    status_code     INTEGER NOT NULL,
    error_message   TEXT,                    -- NULL on success
    estimated_cost  REAL NOT NULL DEFAULT 0.0, -- USD
    upstream_id     TEXT,                    -- Provider's response ID
    created_at      TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

CREATE INDEX idx_requests_timestamp ON requests(timestamp);
CREATE INDEX idx_requests_provider ON requests(provider);
CREATE INDEX idx_requests_combo ON requests(combo_name);
CREATE INDEX idx_requests_api_key ON requests(api_key_id);

CREATE TABLE provider_pricing (
    provider            TEXT NOT NULL,
    model               TEXT NOT NULL,
    input_cost_per_mtok REAL NOT NULL,       -- USD per million input tokens
    output_cost_per_mtok REAL NOT NULL,      -- USD per million output tokens
    updated_at          TEXT NOT NULL,
    PRIMARY KEY (provider, model)
);
```

#### 6.21.2 Cost Calculation

```
estimated_cost = (input_tokens × input_cost_per_mtok / 1,000,000)
               + (output_tokens × output_cost_per_mtok / 1,000,000)
```

**Default Pricing Table** (user-overridable in `pool.toml`):

```toml
[pricing]
# NVIDIA NIM (free tier — $0)
"nvidia/deepseek-ai/deepseek-v4.1-flash" = { input = 0.0, output = 0.0 }
"nvidia/z-ai/glm-5.3-flash" = { input = 0.0, output = 0.0 }
"nvidia/z-ai/glm-5.3" = { input = 0.0, output = 0.0 }
"nvidia/moonshotai/kimi-k3" = { input = 0.0, output = 0.0 }

# OpenRouter (example rates — user should update)
"openrouter/deepseek/deepseek-chat-v4.1-flash" = { input = 0.07, output = 0.28 }
"openrouter/moonshotai/kimi-k3" = { input = 1.50, output = 6.00 }

# Antigravity (example rates)
"antigravity/gemini-2.5-flash" = { input = 0.15, output = 0.60 }
```

#### 6.21.3 Aggregation & Queries

The dashboard and CLI can query aggregated usage data:

**CLI Access**:
```powershell
# Summary for last 24 hours
codex-9router-proxy --usage-report --since 24h

# Breakdown by provider, JSON output
codex-9router-proxy --usage-report --since 7d --by provider --json

# Breakdown by combo, CSV output
codex-9router-proxy --usage-report --since 30d --by combo --csv

# Breakdown by API key
codex-9router-proxy --usage-report --since 24h --by key
```

**Example Output**:
```
Pool Gateway Usage Report (last 24h)
═══════════════════════════════════════════════════
Total Requests:  1,247     Errors: 3 (0.2%)
Input Tokens:    2.4M      Output Tokens: 680K
Reasoning:       320K      Estimated Cost: $4.82

By Provider:
  nvidia        892 reqs   2.1M tok   $0.00
  openrouter    312 reqs   890K tok   $3.94
  antigravity    43 reqs   130K tok   $0.88

By Combo:
  fast-coder    578 reqs   1.2M tok   $1.20
  heavy-worker  420 reqs   1.5M tok   $3.10
  code-reviewer  89 reqs   410K tok   $0.52
```

#### 6.21.4 Data Retention

- Default: 90 days
- Configurable: `[analytics] retention_days = 90` in `pool.toml`
- Auto-prune: `DELETE FROM requests WHERE timestamp < datetime('now', '-90 days')` on startup and daily
- Export before prune: optional `--export-before-prune` flag

---

### 6.22 Quota Tracker & Account Health Manager

**Purpose**: Automatically detect, track, and manage provider account quotas. Disable exhausted accounts, re-enable after reset, and route around unhealthy providers.

#### 6.22.1 Account Configuration

```toml
[[accounts]]
id = "nvidia-1"
provider = "nvidia"
api_key_encrypted = "ENC[AES256:base64data...]"
endpoint = "https://integrate.api.nvidia.com/v1"
enabled = true
notes = "Personal NVIDIA account"

[[accounts]]
id = "nvidia-2"
provider = "nvidia"
api_key_encrypted = "ENC[AES256:base64data...]"
endpoint = "https://integrate.api.nvidia.com/v1"
enabled = true
notes = "Work NVIDIA account"

[[accounts]]
id = "openrouter-1"
provider = "openrouter"
api_key_encrypted = "ENC[AES256:base64data...]"
endpoint = "https://openrouter.ai/api/v1"
enabled = true

[[accounts]]
id = "antigravity-1"
provider = "antigravity"
api_key_encrypted = "ENC[AES256:base64data...]"
endpoint = "https://api.antigravity.dev/v1"
enabled = true

[[accounts]]
id = "9router-legacy"
provider = "9router"
api_key_encrypted = "ENC[AES256:base64data...]"
endpoint = "http://localhost:20128/v1"
enabled = true
notes = "Backward compat — remove after migration"
```

#### 6.22.2 Account Health State Machine

```mermaid
stateDiagram-v2
    [*] --> Healthy: Account added & enabled

    Healthy --> Cooldown: 429 Rate Limited
    Healthy --> Cooldown: 402 Quota Exhausted
    Healthy --> Unhealthy: 5xx / Timeout / Connection Error
    Healthy --> AuthError: 401 Unauthorized (Invalid Key)

    Cooldown --> Probing: Cooldown timer expires
    Unhealthy --> Probing: Probe timer expires

    Probing --> Healthy: Probe 200 OK
    Probing --> Cooldown: Probe fails (exponential backoff)
    Probing --> AuthError: Probe returns 401

    Healthy --> Disabled: User disables
    Cooldown --> Disabled: User disables
    Unhealthy --> Disabled: User disables
    Probing --> Disabled: User disables
    AuthError --> Disabled: Auto-disabled

    Disabled --> Probing: User re-enables (or edits key)
```

**State Descriptions**:

| State | Scoring Impact | Behavior |
|:---|:---|:---|
| **Healthy** | `health_score = 1.0` | Eligible for routing. Normal operation. |
| **Cooldown** | `health_score = 0.3` | Temporarily degraded. Short cooldown (30s for 503, 60s for 429, 3600s for 402). Still eligible if no healthy alternatives. |
| **Unhealthy** | `health_score = 0.0` | Excluded from routing. Connection failures, repeated 5xx. Auto-probed. |
| **AuthError** | `health_score = 0.0` | Excluded from routing. HTTP 401 (invalid/revoked key). **Auto-probing halted** to avoid spam. Triggers alert on Dashboard card & tray tooltip. |
| **Probing** | `health_score = 0.0` | Excluded from routing. Sending lightweight probe to check recovery. |
| **Disabled** | `health_score = 0.0` | Excluded from routing. Manually disabled by user or auto-disabled on AuthError. |

#### 6.22.3 Quota Detection & Proactive Rate-Limit Smoothing

| HTTP Status | Meaning | Action |
|:---|:---|:---|
| `401 Unauthorized` | Invalid / revoked API key | Transition immediately to **AuthError**; halt probes; alert user via UI. |
| `429 Too Many Requests` | RPM/RPD exceeded | Enter Cooldown. Use `Retry-After` header if present. Default 60s. |
| `402 Payment Required` | Credits exhausted | Enter Cooldown with long duration (3600s). Log warning. |
| `503 Service Unavailable` | Provider overloaded | Enter Cooldown with short duration (30s). |
| `5xx` (other) | Server error | Enter Unhealthy after 3 consecutive failures. |
| Timeout (>30s) | No response | Enter Unhealthy after 2 consecutive timeouts. |
| Connection refused | Service down | Enter Unhealthy immediately. |

**Proactive Rate-Limit Smoothing**:
Rather than waiting for a provider to return an HTTP `429` error, the routing engine continuously tracks returned quota headers (`X-RateLimit-Remaining-Requests`, `X-RateLimit-Reset-Requests`):

- **Danger Threshold**: When an account's reported remaining quota drops to $\le 2$ requests and the reset window has not passed, the scoring formula applies an exponential penalty:
  $$\text{quota\_penalty} = \left(1.0 - \frac{\text{remaining}}{3}\right) \times 0.7$$
- **Preemptive Switch**: This cleanly diverts incoming requests to alternative sibling accounts *before* an actual 429 occurs, achieving zero-error throughput across pooled free-tier keys.

#### 6.22.4 Auto-Heal Probe System

Background thread probes unhealthy/cooldown accounts:

| Setting | Default | Description |
|:---|:---|:---|
| `probe_interval_s` | `60` | Base interval between probe attempts |
| `probe_backoff_max_s` | `3600` | Maximum probe interval (exponential backoff) |
| `probe_request` | `GET /v1/models` | Lightweight probe (no token cost) |
| `consecutive_failures_to_unhealthy` | `3` | Failures before Cooldown → Unhealthy |

**Probe Logic**:
1. For each account in Cooldown or Unhealthy state:
2. Send `GET /v1/models` with the account's credentials
3. If `200 OK` → promote to Healthy, reset backoff
4. If error → stay in current state, multiply probe interval by 2 (max `probe_backoff_max_s`)

#### 6.22.5 Account Management

**Enable/Disable** via three interfaces:
- **Dashboard UI**: Toggle switch on account card
- **CLI**: `codex-9router-proxy --account nvidia-1 --disable` / `--enable`
- **Config file**: Set `enabled = false` in `pool.toml` (hot-reloaded)

**Dashboard Account Card**:
```
┌──── nvidia-1 ────────────────────────────────────┐
│ 🟢 Healthy              [Enable/Disable Toggle] │
│ Provider: NVIDIA NIM                              │
│ Endpoint: integrate.api.nvidia.com/v1             │
│ API Key: nvapi-****...****3f2a                    │
│                                                    │
│ RPM Usage:  ████████░░░░░░░░ 24/40 (60%)         │
│ RPD Usage:  ██░░░░░░░░░░░░░░ 1,200/10,000 (12%) │
│ Credits:    ████████████░░░░ 780/1,000 remaining  │
│                                                    │
│ Last Request: 12s ago  Avg Latency: 1,800ms      │
│ Requests Today: 1,200  Errors: 2 (0.2%)          │
│ Score: 0.87                                       │
│                                                    │
│ Recent Errors:                                     │
│  • 15:23:04 — 429 Rate limited (retried → OK)    │
│  • 14:45:12 — Timeout after 30s                   │
└────────────────────────────────────────────────────┘
```

---

### 6.23 Proxy Passthrough Toggle

**Purpose**: Allow users to instantly toggle the proxy between **pooled mode** (subagents route through the Pool Gateway) and **passthrough mode** (subagents use the signed-in ChatGPT account's official model) without restarting Codex or the proxy.

#### 6.23.1 Toggle Mechanism

The toggle flips a single `AtomicBool` flag checked at the entry point of both Layer 1 (JSON-RPC interception) and Layer 2 (HTTPS proxy):

```mermaid
stateDiagram-v2
    [*] --> Pooled: proxy_enabled = true (default)
    Pooled --> Passthrough: User toggles OFF
    Passthrough --> Pooled: User toggles ON

    state Pooled {
        [*] --> L1_Intercept: JSON-RPC rewriting active
        L1_Intercept --> L2_Route: Subagent → Pool Gateway (:20130)
    }

    state Passthrough {
        [*] --> L1_Pass: JSON-RPC passthrough (no rewriting)
        L1_Pass --> L2_Pass: Subagent → chatgpt.com (direct)
    }
```

**Controllable via**:
- **CLI**: `codex-9router-proxy --proxy-mode on|off|toggle|status`
- **Dashboard**: Prominent toggle switch in the sidebar header
- **System Tray**: Right-click menu → click "Proxy: ON" to toggle (see §6.24)

**State Persistence**: Stored in `pool.toml` as:
```toml
[passthrough]
proxy_enabled = true              # true = pooled (default), false = passthrough
model = "gpt-6-luna-max"          # Official model used in passthrough mode
```

Changes are Tier 1 hot-reloadable (§6.19.5) — take effect on the next request.

#### 6.23.2 Passthrough Model Selection

When the proxy is OFF, subagents use the official ChatGPT model from the user's signed-in account. The model is configurable:

**In `pool.toml`**:
```toml
[passthrough]
model = "gpt-6-luna-max"   # default for ChatGPT Plus/Pro
```

**In Dashboard sidebar (when proxy is OFF)**:
```
┌──────────────────────────┐
│ ○ Passthrough Mode       │
│   Model: [gpt-6-luna-max ▼]
│           ┌──────────────┐
│           │ gpt-6-luna-max│
│           │ gpt-6-luna   │
│           │ o4-mini      │
│           │ o3           │
│           └──────────────┘
└──────────────────────────┘
```

The model dropdown is populated from the official Codex model catalog (fetched from `chatgpt.com/backend-api/codex/models` on startup). Changes are instant (Tier 1 hot-reload).

#### 6.23.3 Drain-Then-Switch Behavior

When toggling from ON → OFF mid-session:

1. **In-flight subagent turns** (currently streaming an SSE response) are allowed to **finish** through the pool gateway
2. Any **new** `spawn_agent` / `turn/start` calls arriving after the toggle use the passthrough model
3. The transition is seamless — no errors, no truncated responses
4. Dashboard status changes from `● Online (pooled)` to `○ Passthrough (direct)`

When toggling from OFF → ON:
1. Takes effect immediately on the next subagent spawn/turn
2. No drain needed — passthrough requests don't hold pool resources

#### 6.23.4 Analytics in Passthrough Mode

Usage analytics (§6.21) **continue tracking requests** in passthrough mode:

- Requests are logged with `provider = "chatgpt-direct"` and `combo_name = "passthrough"`
- Token counts, latency, and error rates are tracked via Layer 2 SSE response inspection
- Cost estimation uses the ChatGPT model's pricing (if known) or marks as `estimated_cost = null`
- Dashboard shows passthrough requests in a distinct color/badge for visual separation

#### 6.23.5 Notification

On every toggle event:
- **Dashboard**: Sidebar status indicator updates (`● Pooled` ↔ `○ Passthrough`)
- **System Tray**: Icon color changes (green = pooled, gray = passthrough)
- **Windows Toast**: Subtle desktop notification: `"Proxy switched to passthrough mode — subagents now using gpt-6-luna-max"`
- **Console log**: `[INFO] Proxy mode: PASSTHROUGH (model: gpt-6-luna-max)`

---

### 6.24 System Tray Controller

**Purpose**: Provide a Windows notification area (system tray) icon for at-a-glance proxy status and quick control without opening a terminal or browser.

#### 6.24.1 Tray Icon

The system tray icon uses Windows Shell_NotifyIcon API (via `tray-item` or `winit` crate):

| Proxy State | Icon Color | Tooltip |
|:---|:---|:---|
| Pooled (ON), all healthy | 🟢 Green | `Pool Gateway v0.4.0 — Pooled (5 accounts, 4 healthy)` |
| Pooled (ON), some unhealthy | 🟡 Yellow | `Pool Gateway v0.4.0 — Pooled (3/5 accounts healthy)` |
| Passthrough (OFF) | ⚪ Gray | `Pool Gateway v0.4.0 — Passthrough (gpt-6-luna-max)` |
| Error (gateway down) | 🔴 Red | `Pool Gateway v0.4.0 — Error: gateway not responding` |

#### 6.24.2 Context Menu

Right-click the tray icon to show:

```
┌──────────────────────────────────┐
│ ⚡ Pool Gateway v0.4.0          │
├──────────────────────────────────┤
│ ● Proxy: ON (pooled)            │  ← Click to toggle
│ 🟢 Accounts: 4/5 healthy        │
│ Requests today: 1,247           │
│ Est. cost today: $4.82          │
├──────────────────────────────────┤
│ 🌐 Open Dashboard               │  ← Opens localhost:20130 in browser
│ 📋 Copy Gateway URL             │  ← Copies http://localhost:20130/v1
├──────────────────────────────────┤
│ ❌ Quit                          │  ← Stops proxy daemon
└──────────────────────────────────┘
```

**Interactions**:
- **Click "Proxy: ON"** → toggles to OFF, label changes to `○ Proxy: OFF (passthrough)`, toast notification
- **Left-click tray icon** → opens Dashboard in default browser
- **"Open Dashboard"** → opens `http://localhost:20130/dashboard/` in default browser
- **"Copy Gateway URL"** → copies `http://localhost:20130/v1` to clipboard (for pasting into Cursor/Claude Code settings)
- **"Quit"** → graceful shutdown of `--proxy-daemon` (with confirmation if subagents are active)

#### 6.24.3 Launch Behavior & Headless Support

- **Interactive Desktop Auto-Detection**: When `--proxy-daemon` starts, it automatically detects if an interactive desktop session is active (checking Win32 window station availability).
- **Default Behavior**: If an interactive user session is detected, the tray controller spawns automatically.
- **Headless Mode (`--no-tray`)**: When explicitly launched with `--no-tray`, or when running in a headless environment (Windows Service, SSH session, automated background task), tray icon creation is cleanly bypassed without warning or failure.
- **Dedicated Thread Isolation**: The tray runs on a dedicated OS thread (`std::thread::spawn`) driving a Win32 message pump (`GetMessageW` / `DispatchMessageW`). Tokio worker threads are never blocked by Windows GUI message processing. Communication between the tray thread and Tokio occurs via lock-free atomics and `tokio::sync::mpsc` channels.
- The startup hook (`Codex9RouterHookSync.cmd`) launches with the tray enabled by default for interactive desktop use.

---

### 6.25 Request Deduplication & Response Cache

**Purpose**: Cache responses to near-identical requests to eliminate redundant upstream API calls, saving tokens, cost, and latency when multiple subagents request similar completions within a short window.

#### 6.25.1 Cache Architecture

```mermaid
flowchart TD
    REQ["Incoming Request"] --> HASH["Compute request hash<br/>(xxHash64)"]
    HASH --> LOOKUP{"Cache hit?"}
    LOOKUP -->|"Hit (< TTL)"| CACHED["Return cached response<br/>● Zero latency<br/>● Zero tokens"]
    LOOKUP -->|"Miss"| ROUTE["Route to provider<br/>(normal pool engine)"]
    ROUTE --> STORE["Store response in cache<br/>with TTL"]
    STORE --> RETURN["Return response to client"]

    style CACHED fill:#00b894,stroke:#55efc4,color:#fff
    style ROUTE fill:#6c5ce7,stroke:#a29bfe,color:#fff
```

#### 6.25.2 Cache Key

The cache key is an `xxHash64` digest of the following normalized fields:

```
hash = xxh64(
    model +                    # Resolved model (after combo resolution)
    sorted(messages) +         # Full message array (system + user + assistant)
    sorted(tools) +            # Tool definitions (if present)
    temperature +              # Sampling parameters affect output
    max_tokens +
    reasoning_effort
)
```

**Normalization**:
- Messages are serialized deterministically (sorted keys within each message object)
- Whitespace-only differences are ignored
- `stream: true/false` is NOT part of the key — cached responses can serve both streaming and non-streaming requests

#### 6.25.3 Cache Configuration

```toml
[cache]
enabled = true
max_entries = 1000              # LRU eviction when full
ttl_seconds = 300               # 5-minute TTL (default)
max_response_bytes = 1048576    # Don't cache responses > 1 MB
exclude_models = []             # Models to never cache (e.g., always-fresh models)
```

#### 6.25.4 Cache Behavior

| Scenario | Behavior |
|:---|:---|
| Two subagents request the same file content analysis within 30s | Second request returns cached response (~0ms latency, 0 tokens) |
| Two subagents issue identical streaming request simultaneously | **Singleflight Coalescing**: Second subagent subscribes to first subagent's active token broadcast channel — both stream simultaneously from a single upstream connection |
| Same request but different `temperature` | Cache miss — different temperature = different key |
| Streaming request with cached response available | Cached response replayed as SSE chunks from memory |
| Response exceeds `max_response_bytes` | Not cached (too large to store efficiently) |
| Provider returns error (4xx/5xx) | Not cached (only successful 200 responses are cached) |

#### 6.25.5 Singleflight Streaming Coalescing

In multi-agent architectures, parallel subagents (e.g., worker and reviewer) frequently query identical context simultaneously. Simple post-completion caching suffers from cache misses during the 5–15 seconds a stream takes to complete.

**Mechanism**:
1. When a streaming request (`stream: true`) arrives, the gateway computes the `xxHash64` key and checks an active in-flight map (`Arc<DashMap<u64, broadcast::Sender<Bytes>>>`).
2. **If Miss**: The gateway initiates the upstream provider connection, registers a new broadcast channel, and streams incoming chunks to both the initiating client and the broadcast channel.
3. **If In-Flight Hit**: The secondary request subscribes to the existing broadcast channel without contacting the upstream provider, piping live SSE chunks directly to the second client.
4. **Completion**: When the stream terminates with `[DONE]`, the complete accumulated response is written to the LRU cache (for future sequential hits) and removed from the active in-flight map.

#### 6.25.6 Analytics Integration

Cache performance is tracked in the usage analytics (§6.21):

- `cache_hit` column in `requests` table (boolean)
- Dashboard shows cache hit rate, tokens saved, and estimated cost saved
- CLI: `codex-9router-proxy --cache-stats` shows hit rate, entry count, memory usage

---

## 7. Configuration Reference

### 7.1 Environment Variables

| Variable | Default | Description |
|:---|:---|:---|
| `CODEX_HOME` | `~/.codex` | Custom path to Codex home directory |
| `CODEX_CLI_PATH` | *(set by installer)* | Override path to `codex-9router-subagents.exe` |
| `CODEX_CA_CERTIFICATE` | *(auto-generated)* | Path to loopback TLS certificate (`bridge-cert.pem`) |
| `CODEX_PROXY_PORT` | `20129` | Loopback reverse proxy port |
| `NINEROUTER_KEY` | *(user-set)* | API authentication key for 9Router/target provider |
| `NVIDIA_API_KEY` | *(user-set)* | NVIDIA NIM API key (`nvapi-...`) for direct NVIDIA provider |
| `POOL_GATEWAY_PORT` | `20130` | Pool gateway HTTP server port |
| `POOL_CONFIG_PATH` | `~/.codex/pool.toml` | Path to pool gateway configuration |
| `CODEX_SUBAGENT_PROVIDER` | `9router` | Provider identifier |
| `CODEX_SUBAGENT_ENDPOINT` | `http://localhost:20128/v1` | Upstream subagent endpoint |
| `CODEX_DEFAULT_MODEL` | `9router-subagent` | Default subagent model |
| `CODEX_WORKER_MODEL` | *(from config)* | Worker role model override |
| `CODEX_EXPLORER_MODEL` | *(from config)* | Explorer role model override |
| `CODEX_REVIEWER_MODEL` | *(from config)* | Reviewer role model override |
| `CODEX_SUBAGENT_CONTEXT_WINDOW` | `872000` | Subagent context window size |
| `CODEX_CHATGPT_UPSTREAM_BASE` | `https://chatgpt.com` | Upstream ChatGPT URL |
| `AGENT_BROWSER_EXECUTABLE_PATH` | *(auto-detected)* | Path to system Chrome or Edge |
| `CODEX_INTERNAL_ORIGINATOR_OVERRIDE` | *(set by IDE)* | IDE extension origin indicator |

### 7.2 Configuration Files

#### `~/.codex/config.toml`

```toml
# Provider definition
[model_providers.9router]
name = "9router"
base_url = "http://localhost:20128/v1"
env_key = "NINEROUTER_KEY"

# Default subagent model
[agents]
default_subagent_model = "9router-subagent"

# Per-role model overrides (optional)
[subagent_models]
default = "9router-subagent"
worker = "9router-subagent"
explorer = "9router-subagent"
reviewer = "9router-subagent"

# Autonomous execution
[apps._default]
default_tools_approval_mode = "approve"

[approval_policy.granular]
# ... per-tool exemptions

[browser_use.default_origin_policy]
# ... CDP permissions
```

#### `~/.codex/agents/<role>.toml`

Per-role manifest files (e.g., `worker.toml`, `explorer.toml`, `reviewer.toml`, `default.toml`):

```toml
model = "9router-subagent"
model_provider = "9router"
# Optionally override base_url and env_key
```

#### `~/.codex/models_cache.json`

Auto-maintained cache of model metadata including `9router-subagent` with context window, reasoning levels, and capability flags. Updated atomically via `.tmp.<pid>` + `fs::rename`.

### 7.3 SQLite Triggers

Injected into all `~/.codex/state_*.sqlite` and `codex-dev.db` databases:

| Trigger | Event | Purpose |
|:---|:---|:---|
| `fix_subagent_provider_trigger` | `AFTER INSERT ON threads` | Enforce `model_provider = '9router'` on new subagent rows |
| `fix_subagent_provider_update_trigger` | `AFTER UPDATE OF model_provider, model ON threads` | Enforce `model_provider = '9router'` on updated subagent rows (e.g., `thread/resume`) |

---

## 8. HTTP Endpoints

| Method | Path | Response | Description |
|:---|:---|:---|:---|
| `GET` | `/health`, `/health/` | `200` `"codex-9router-proxy ok"` | Health check |
| `GET` | `/__codex_9router_proxy_healthz` | `200` `"codex-9router-proxy ok"` | Named health check (for standby probe) |
| `GET` | `/backend-api/codex/responses` | `426 Upgrade Required` | Reject WebSocket, force HTTPS POST |
| `GET` | `/backend-api/responses` | `426 Upgrade Required` | Reject WebSocket, force HTTPS POST |
| `GET` | `/codex/responses` | `426 Upgrade Required` | Reject WebSocket, force HTTPS POST |
| `GET` | `/responses` | `426 Upgrade Required` | Reject WebSocket, force HTTPS POST |
| `POST` | `/backend-api/codex/responses` | SSE stream | Main response routing (subagent detection + forwarding) |
| `POST` | `/backend-api/responses` | SSE stream | Alias for main response routing |
| `POST` | `/responses` | SSE stream | Alias for main response routing |
| `GET` | `/backend-api/codex/models` | JSON | Model catalog with injected subagent models |
| `GET` | `/backend-api/models` | JSON | Model catalog with injected subagent models |
| `*` | `/**` | Passthrough | Forward to `https://chatgpt.com/backend-api/*` |

**Layer 3 — Pool Gateway Endpoints** (`127.0.0.1:20130`):

| Method | Path | Auth | Description |
|:---|:---|:---|:---|
| `POST` | `/v1/chat/completions` | `Bearer pool-sk-*` | OpenAI Chat Completions API — pool routes to best provider |
| `POST` | `/v1/responses` | `Bearer pool-sk-*` | OpenAI Responses API — pool routes to best provider |
| `GET` | `/v1/models` | `Bearer pool-sk-*` | List available combos and models |
| `GET` | `/health` | None | Gateway health check |
| `GET` | `/dashboard/` | None (localhost only) | Web Dashboard UI (static HTML/JS/CSS) |
| `WS` | `/dashboard/ws` | None (localhost only) | WebSocket for real-time dashboard events |
| `GET` | `/api/accounts` | None (localhost only) | List provider accounts and health status |
| `PUT` | `/api/accounts/:id/toggle` | None (localhost only) | Enable/disable an account |
| `GET` | `/api/keys` | None (localhost only) | List issued API keys |
| `POST` | `/api/keys` | None (localhost only) | Generate a new API key |
| `DELETE` | `/api/keys/:id` | None (localhost only) | Revoke an API key |
| `GET` | `/api/usage` | None (localhost only) | Query usage analytics (with query params for period, groupBy) |
| `GET` | `/api/combos` | None (localhost only) | List configured combos |

---

## 9. Installation & Deployment

### 9.1 Installer (`install.ps1`)

**Size**: ~1,901 lines of PowerShell

**Installation Modes**:
- Interactive: Guided setup with provider selection prompts
- Preset: `-Preset 9router|ollama|lmstudio|openrouter|vllm|litellm`
- Doctor: `-Doctor` runs diagnostics only

**Installation Steps**:
1. Detect or locate `codex-9router-proxy.exe` (prebuilt from GitHub Release `.zip` or compiled)
2. Copy proxy binary to `%LOCALAPPDATA%\OpenAI\Codex\custom\codex-9router-subagents.exe`
3. Set `CODEX_CLI_PATH` in Windows User Environment (`HKCU\Environment`)
4. Prepend `custom\` to index 0 of User `Path`
5. Hook standalone CLI and IDE extension directories with `.orig.exe` backups
6. Configure `config.toml` with provider settings
7. Create per-role agent manifests in `agents/*.toml`
8. Inject SQLite database triggers
9. Sync `models_cache.json`
10. Generate TLS certificates
11. Create Windows startup hook (`Codex9RouterHookSync.cmd` → `hook-sync.ps1`)
12. Launch detached `--proxy-daemon`
13. Broadcast `WM_SETTINGCHANGE` for environment pickup

**Safety Features**:
- PE sanity checks before binary operations
- NTFS image path queries via Win32 `QueryFullProcessImageNameW`
- Ancestor PID exclusion for safe process termination
- TOML value escaping via `ConvertTo-TomlEscapedString`
- SQLite verification with `PRAGMA table_info(threads)` and `timeout=5.0`

### 9.2 Uninstaller (`uninstall.ps1`)

**Size**: ~509 lines of PowerShell

**Uninstallation Steps**:
1. Restore stock binaries from `.orig.exe` backups across all surfaces
2. Clean `CODEX_CLI_PATH`, `CODEX_CA_CERTIFICATE`, `CODEX_PROXY_PORT` from environment
3. Remove `custom\` from User `Path`
4. Remove SQLite triggers from all databases
5. Restore clean `config.toml` (remove all managed sections)
6. Remove `agents/*.toml` manifests
7. Remove injected `models_cache.json` entries
8. Remove startup hook files
9. Clean `%LOCALAPPDATA%\OpenAI\Codex\custom` directory

### 9.3 CI/CD Pipeline

**Workflow**: `.github/workflows/release.yml`

**Trigger**: Git tags matching `v*`

**Steps**:
1. Run `cargo test --release`
2. Compile release binary on `windows-latest`
3. Package `codex-9router-proxy-windows-amd64.zip` with installer, uninstaller, README, CHANGELOG, LICENSE
4. Publish via GitHub Releases API

### 9.4 Build from Source

```powershell
# Prerequisites: Rust 1.80+
cargo build --release
# Output: target\release\codex-9router-proxy.exe (~6.2 MB)
```

**Release Profile** (`Cargo.toml`):
- `opt-level = 3` (maximum optimization)
- `lto = true` (link-time optimization)
- `codegen-units = 1` (single codegen unit for best optimization)
- `strip = true` (strip debug symbols)

---

## 10. Compatibility Matrix

| Component | Verified Version | Notes |
|:---|:---|:---|
| **OS** | Windows 11 Pro (Build 26200) | Windows 10 & 11 x86_64 |
| **Codex Desktop App** | `26.929.21022.0` (Microsoft Store) | `OpenAI.Codex` & `OpenAI.CodexPrimaryRuntime` |
| **Codex CLI Engine** | `codex-cli 0.159.0-alpha.4` | `0.158.0-alpha.2+` |
| **Primary Model** | `gpt-6-luna` | ChatGPT Plus/Pro account |
| **9Router** | `0.5.91` | Default subagent endpoint |
| **PowerShell** | 7.6+ & Windows PowerShell 5.1 | Both `pwsh.exe` and `powershell.exe` |
| **Rust Toolchain** | `rustc 1.98.1` | Only needed for building from source |

**Supported LLM Providers** (via installer presets):

| Provider | Preset | Default Endpoint |
|:---|:---|:---|
| 9Router | `9router` | `http://localhost:20128/v1` |
| NVIDIA NIM | `nvidia` | `https://integrate.api.nvidia.com/v1` |
| Antigravity | `antigravity` | `https://api.antigravity.dev/v1` |
| OpenRouter | `openrouter` | `https://openrouter.ai/api/v1` |
| Poolside | `poolside` | `https://api.poolside.ai/v1` |
| Ollama Cloud | `ollama-cloud` | *(user-configured)* |
| Ollama (local) | `ollama` | `http://localhost:11434/v1` |
| LM Studio | `lmstudio` | `http://localhost:1234/v1` |
| vLLM | `vllm` | `http://localhost:8000/v1` |
| LiteLLM | `litellm` | `http://localhost:4000/v1` |
| Custom | *(interactive)* | *(user-specified)* |

---

## 11. Security Considerations

| Concern | Mitigation |
|:---|:---|
| **Credential exposure** | ChatGPT credentials, cookies, and OAuth tokens are never inspected, logged, or modified. They pass through transparently to `chatgpt.com`. |
| **TLS certificate scope** | Self-signed cert is bound to `127.0.0.1` only. Never exposed on external interfaces. |
| **API key handling** | Provider API keys (e.g., `NINEROUTER_KEY`) read from environment variables at runtime, never hardcoded or logged. |
| **Pool key encryption** | Upstream provider API keys in `pool.toml` are AES-256-GCM encrypted at rest. Master key derived via DPAPI (Windows) or user password. Decrypted only in memory. |
| **Self-issued key auth** | Pool gateway requires `Bearer pool-sk-*` authentication on all `/v1/` endpoints. Keys support expiry, IP whitelisting, and rate limiting. |
| **Dashboard access** | Dashboard `/dashboard/` and `/api/` management endpoints are localhost-only. Non-loopback requests rejected with 403. |
| **Binary integrity** | PE header validation + `--version` execution before adopting any stock binary. `LastWriteTimeUtc` preserved. |
| **Process isolation** | Ancestor process chain inspection prevents IDE launches from triggering Desktop healing or process termination. |
| **Recursive delegation** | `collaboration.spawn_agent` stripped from subagent tool declarations to prevent infinite delegation chains. |
| **Guardian preservation** | Internal safety classifiers (`guardian_classifier`, `guardian_review`) always routed to ChatGPT, never to external providers. |
| **WebSocket rejection** | HTTP 426 on WebSocket upgrades forces HTTPS POST where request inspection is possible. |
| **Quota exhaustion** | Automatic account disabling on 402/429 prevents charge accumulation on pay-per-token providers. |

---

## 12. Testing

**Test Suite**: 4,234 lines of unit tests embedded in `src/main.rs`

**Test Invocation**:
```powershell
cargo test --release
```

**Coverage Areas**:
- Subagent detection and classification logic
- Role-to-model resolution cascades
- Request sanitization transforms
- SSE stream transformation
- Compaction request handling and fallback
- Namespace tool bridging (flatten/unflatten)
- Multi-Agent V2 normalization
- Guardian classifier exclusion
- User fork preservation
- Rate limit sanitization
- Model metadata injection
- Gemini/Vertex AI schema sanitization
- NVIDIA NIM Chat Completions ↔ Responses API translation
- NVIDIA model-specific reasoning effort mapping
- NVIDIA vision input format conversion
- Pool gateway adaptive scoring algorithm
- Combo resolution and model aliasing
- API key validation (expiry, IP whitelist, rate limiting)
- Account health state machine transitions
- Quota detection from HTTP response headers
- Auto-heal probe scheduling and backoff
- AES-256-GCM key encryption/decryption
- Chat Completions ↔ Responses API bidirectional translation
- Usage analytics SQLite write and aggregation queries
- Cost estimation calculation accuracy
- Hot-reload config file watcher
- Per-key rate limiter (sliding window)

**CI**: Tests run automatically on every release tag via GitHub Actions.

---

## 13. Future Considerations

> [!NOTE]
> The following features are candidates for future development. Each section should be expanded into a full specification before implementation. When ready to implement, update this PRD first, then build against the updated spec.

### F-1 Web Dashboard / Admin UI

**Problem**: No visibility into proxy activity, routing decisions, latency, or errors beyond log output. Users cannot observe what the proxy is doing in real time, diagnose routing issues, or understand cost/performance tradeoffs without reading raw stdout.

**Proposed Solution**: Serve a local web dashboard on a secondary port (e.g., `:20130`) providing real-time observability, configuration management, and usage analytics.

**Technical Foundation**:
- Serve static HTML/CSS/JS from embedded Rust assets (`include_str!` / `include_bytes!`)
- WebSocket endpoint (`ws://127.0.0.1:20130/ws`) for real-time event push
- In-memory ring buffer of recent events (configurable depth, default: 10,000 entries)
- Persistent storage in `%LOCALAPPDATA%\OpenAI\Codex\custom\proxy-dashboard.sqlite`
- Zero external dependencies — no npm/node/electron, pure embedded
- Dark theme by default (matching Codex Desktop aesthetic), with light theme toggle
- Responsive layout — usable on 1280px+ screens, gracefully degraded on smaller

**Priority**: Medium · **Complexity**: Medium-High

#### F-1.1 Information Architecture & Navigation

The dashboard uses a fixed left sidebar navigation with 9 primary pages:

```mermaid
flowchart LR
    NAV["Left Sidebar"] --> LIVE["🔴 Live Feed"]
    NAV --> SUB["👥 Subagents"]
    NAV --> PROV["🔌 Providers & Accounts"]
    NAV --> USAGE["📊 Usage"]
    NAV --> KEYS["🔑 API Keys"]
    NAV --> COMBOS["🎯 Combos"]
    NAV --> CONFIG["⚙️ Config"]
    NAV --> DOCTOR["🩺 Doctor"]

    LIVE --> DETAIL["Request Detail<br/>(slide-over panel)"]
    SUB --> TIMELINE["Subagent Timeline<br/>(expanded row)"]
    PROV --> ACCT["Account Cards<br/>(enable/disable, quota)"]
    KEYS --> KEYGEN["Generate Key<br/>(modal)"]
    COMBOS --> COMBODETAIL["Combo Editor<br/>(inline)"]
    USAGE --> EXPORT["Export Report<br/>(modal)"]
```

**Sidebar Layout**:
```
┌──────────────────────┐
│ ⚡ Pool Gateway      │  ← Logo/title
│    v0.3.0            │  ← Version badge
│ ● Online             │  ← Status indicator (green dot)
├──────────────────────┤
│ 🔴 Live Feed         │  ← Active page highlighted
│ 👥 Subagents         │
│ 🔌 Providers         │  ← Shows account health badges
│ 📊 Usage             │
│ 🔑 API Keys          │  ← NEW: manage issued keys
│ 🎯 Combos            │  ← NEW: manage model aliases
│ ⚙️ Config            │
│ 🩺 Doctor            │
├──────────────────────┤
│ Uptime: 4h 23m       │  ← Footer stats
│ Requests: 1,247      │
│ Accounts: 5 (4🟢 1🟡)│  ← Account health summary
│ Errors: 3            │
└──────────────────────┘
```

#### F-1.2 Page: Live Feed (Default Landing)

**Purpose**: Real-time stream of all requests flowing through the proxy.

**Layout**:
```
┌─────────────────────────────────────────────────────────────────────┐
│  🔴 Live Feed                              ▶ Auto-scroll  ⏸ Pause │
├─────────────────────────────────────────────────────────────────────┤
│  Filter: [All ▼] [Subagent ▼] [Parent ▼] [Error ▼]   🔍 Search   │
├───────┬───────────┬──────────┬─────────┬────────┬──────┬───────────┤
│ Time  │ Direction │ Role     │ Model   │ Provider│ ms  │ Status    │
├───────┼───────────┼──────────┼─────────┼────────┼──────┼───────────┤
│ 15:23 │ → subagent│ worker   │ kimi-k3 │ nvidia │ 2340│ ✅ 200    │
│ 15:22 │ → parent  │ —        │ gpt-6-l │ chatgpt│ 890 │ ✅ 200    │
│ 15:21 │ → subagent│ explorer │ glm-5-3f│ nvidia │ 1120│ ✅ 200    │
│ 15:20 │ → subagent│ compact  │ ds-v4.1f│ nvidia │ 3200│ ✅ 200    │
│ 15:18 │ → subagent│ worker   │ kimi-k3 │ nvidia │ —   │ ❌ 502    │
│ 15:17 │ → guardian│ —        │ gpt-6-l │ chatgpt│ 450 │ ✅ 200    │
└───────┴───────────┴──────────┴─────────┴────────┴──────┴───────────┘
```

**Interactions**:
- Click any row → opens **Request Detail** slide-over panel from the right
- Rows animate in from top with subtle fade (new request arrives)
- Error rows highlighted with red-tinted background
- Guardian rows shown in muted gray
- Pause button freezes the feed (buffered events shown on resume)
- Filter chips: `All`, `Subagent Only`, `Parent Only`, `Errors Only`, `Compaction`
- Search box: filters by model name, role, request ID, or error message

**Request Detail Panel** (right slide-over, 40% width):
```
┌─────────────────────────────────────┐
│  Request Detail              ✕ Close│
├─────────────────────────────────────┤
│  ID: resp_nvidia_1696341823_001     │
│  Time: 15:23:04.231                 │
│  Duration: 2,340ms                  │
│  Direction: Subagent                │
│  Role: worker                       │
│  Model: moonshotai/kimi-k3          │
│  Provider: nvidia                   │
│  Status: 200 OK                     │
├─────────────────────────────────────┤
│  Tokens                             │
│  ├ Input:  12,450                   │
│  ├ Output:  3,280                   │
│  ├ Reasoning: 1,950                 │
│  └ Total: 17,680                    │
├─────────────────────────────────────┤
│  Transforms Applied                 │
│  ☑ Namespace unflatten (3 tools)    │
│  ☑ Gemini schema sanitize           │
│  ☑ Chat Completions translation     │
│  ☑ Final answer phase inject        │
├─────────────────────────────────────┤
│  [Request Headers ▼]               │
│  [Request Body (JSON) ▼]           │
│  [Response Headers ▼]              │
│  [Response Body (SSE) ▼]           │
└─────────────────────────────────────┘
```

#### F-1.3 Page: Subagents

**Purpose**: Track all subagent sessions, their lifecycle, and accumulated activity.

**Layout**:
```
┌─────────────────────────────────────────────────────────────────────┐
│  👥 Subagents                                     Active: 3  Total: 47│
├─────────────────────────────────────────────────────────────────────┤
│  Active Sessions                                                    │
│  ┌─────────────────────────────────────────────────────────────┐   │
│  │ 🟢 worker · kimi-k3 · thread_abc123                        │   │
│  │    Started 2m ago · 5 turns · 42,300 tokens · $0.038       │   │
│  │    ████████████░░░░░░░░ 62% context used                   │   │
│  ├─────────────────────────────────────────────────────────────┤   │
│  │ 🟢 explorer · glm-5-3-flash · thread_def456                │   │
│  │    Started 45s ago · 2 turns · 8,100 tokens · $0.004       │   │
│  │    ██░░░░░░░░░░░░░░░░░░ 8% context used                   │   │
│  ├─────────────────────────────────────────────────────────────┤   │
│  │ 🟡 compact · ds-v4.1-flash · thread_ghi789                 │   │
│  │    Compacting... · summarizing 128K tokens                 │   │
│  └─────────────────────────────────────────────────────────────┘   │
│                                                                     │
│  Recent Completed (click to expand)                                 │
│  ┌─────────────────────────────────────────────────────────────┐   │
│  │ ✅ reviewer · glm-5-3 · 4m ago · 8 turns · $0.12          │   │
│  │ ✅ worker · kimi-k3 · 12m ago · 15 turns · $0.31          │   │
│  │ ❌ worker · kimi-k3 · 18m ago · ERROR after 3 turns        │   │
│  └─────────────────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────────────────┘
```

**Expanded Subagent Timeline** (click a session):
```
┌─────────────────────────────────────────────────────────────────────┐
│  Timeline: worker · kimi-k3 · thread_abc123                        │
├─────────────────────────────────────────────────────────────────────┤
│  15:21:04 ● Turn 1 · input: 4,200 tok · output: 1,800 tok · 1.2s │
│           └ Tool: exec_command("rg -l 'TODO' src/")               │
│  15:21:08 ● Turn 2 · input: 6,100 tok · output: 3,400 tok · 2.8s │
│           └ Tool: write_file("src/config.rs")                     │
│  15:21:14 ● Turn 3 · input: 9,500 tok · output: 2,100 tok · 1.9s │
│           └ Tool: exec_command("cargo test --lib")                │
│  15:21:19 ● Turn 4 (active) · streaming...                        │
│           └ Tool: read_file("src/main.rs") → thinking...          │
└─────────────────────────────────────────────────────────────────────┘
```

#### F-1.4 Page: Providers & Accounts

**Purpose**: Monitor health, latency, and availability of all configured providers.

**Layout**:
```
┌─────────────────────────────────────────────────────────────────────┐
│  🔌 Providers                                                       │
├─────────────────────────────────────────────────────────────────────┤
│                                                                     │
│  ┌──── 9Router ─────────────┐  ┌──── NVIDIA NIM ───────────────┐  │
│  │ 🟢 Healthy               │  │ 🟢 Healthy                    │  │
│  │ localhost:20128/v1        │  │ integrate.api.nvidia.com/v1   │  │
│  │                           │  │                                │  │
│  │ Latency (P50): 1,200ms   │  │ Latency (P50): 1,800ms        │  │
│  │ Latency (P95): 3,400ms   │  │ Latency (P95): 4,200ms        │  │
│  │ Latency (P99): 8,100ms   │  │ Latency (P99): 12,400ms       │  │
│  │                           │  │                                │  │
│  │ Requests: 892             │  │ Requests: 355                  │  │
│  │ Errors: 2 (0.2%)         │  │ Errors: 1 (0.3%)              │  │
│  │ Last seen: 12s ago       │  │ Last seen: 45s ago             │  │
│  │                           │  │                                │  │
│  │ Models:                   │  │ Models:                        │  │
│  │  · 9router-subagent       │  │  · deepseek-v4.1-flash         │  │
│  │                           │  │  · glm-5-3-flash               │  │
│  │                           │  │  · glm-5-3                     │  │
│  │                           │  │  · kimi-k3                     │  │
│  │ [Test Connection]         │  │ [Test Connection]              │  │
│  └───────────────────────────┘  └────────────────────────────────┘  │
│                                                                     │
│  ┌──── ChatGPT (Parent) ────┐  ┌──── Ollama ────────────────────┐  │
│  │ 🟢 Healthy               │  │ ⚫ Not Configured              │  │
│  │ chatgpt.com              │  │                                │  │
│  │ Latency (P50): 890ms     │  │                                │  │
│  │ Requests: 128            │  │                                │  │
│  │ (passthrough only)       │  │                                │  │
│  └───────────────────────────┘  └────────────────────────────────┘  │
│                                                                     │
│  Latency Over Time (last 1h)                                        │
│  ┌─────────────────────────────────────────────────────────────┐   │
│  │     ╭╮                                                      │   │
│  │    ╭╯╰╮  ╭─╮    ╭╮                                         │   │
│  │ ───╯   ╰──╯ ╰────╯╰──── 9Router (avg 1.4s)                │   │
│  │      ╭──╮     ╭──╮                                          │   │
│  │  ────╯  ╰─────╯  ╰────── NVIDIA (avg 2.1s)                │   │
│  │  ─────────────────────── ChatGPT (avg 0.9s)                │   │
│  └─────────────────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────────────────┘
```

**Interactions**:
- "Test Connection" button: fires a health check and shows result inline
- Provider cards auto-update via WebSocket (status dot flips red on failure)
- Latency chart updates in real time, scrollable time range selector

#### F-1.5 Page: Usage & Analytics

**Purpose**: Aggregate token usage, cost estimates, and performance metrics per role, model, and time period.

**Layout**:
```
┌─────────────────────────────────────────────────────────────────────┐
│  📊 Usage & Analytics          Period: [Last 24h ▼]  [Export JSON] │
├─────────────────────────────────────────────────────────────────────┤
│                                                                     │
│  Summary Cards                                                      │
│  ┌──────────┐ ┌──────────┐ ┌──────────┐ ┌──────────┐ ┌──────────┐│
│  │ Total    ││ Input    ││ Output   ││ Reasoning││ Est. Cost││
│  │ Requests ││ Tokens   ││ Tokens   ││ Tokens   ││          ││
│  │   1,247  ││ 2.4M     ││ 680K     ││ 320K     ││  $4.82   ││
│  └──────────┘ └──────────┘ └──────────┘ └──────────┘ └──────────┘│
│                                                                     │
│  Usage by Role                          Usage by Model              │
│  ┌─────────────────────────┐           ┌─────────────────────────┐ │
│  │ ████████████ worker 62% │           │ ████████ kimi-k3    45% │ │
│  │ ████████ explorer   28% │           │ ██████ ds-v4.1-fl   30% │ │
│  │ ████ reviewer        8% │           │ ███ glm-5-3-fl      15% │ │
│  │ ██ compact           2% │           │ ██ glm-5-3          10% │ │
│  └─────────────────────────┘           └─────────────────────────┘ │
│                                                                     │
│  Token Usage Over Time (stacked area chart)                         │
│  ┌─────────────────────────────────────────────────────────────┐   │
│  │  ▓▓▓▓▓▓                                                     │   │
│  │  ▓▓▓▓▓▓▓▓▓                                                  │   │
│  │  ▓▓▓▓▓▓▓▓▓▓▓▓▓▓                                             │   │
│  │  ░░░░░░░░░░░░░░░░░░░                                        │   │
│  │  ░░░░░░░░░░░░░░░░░░░░░░░░                                   │   │
│  │  12:00     13:00     14:00     15:00                         │   │
│  │  ■ Input  ■ Output  ■ Reasoning                              │   │
│  └─────────────────────────────────────────────────────────────┘   │
│                                                                     │
│  Detailed Breakdown                                                 │
│  ┌──────────┬──────────┬────────┬────────┬────────┬────────────┐  │
│  │ Role     │ Model    │ Reqs   │ In Tok │ Out Tok│ Est. Cost  │  │
│  ├──────────┼──────────┼────────┼────────┼────────┼────────────┤  │
│  │ worker   │ kimi-k3  │ 234    │ 1.1M   │ 340K   │ $2.98      │  │
│  │ explorer │ glm-5-3f │ 189    │ 620K   │ 180K   │ $0.72      │  │
│  │ reviewer │ glm-5-3  │ 45     │ 380K   │ 95K    │ $0.84      │  │
│  │ default  │ ds-v4.1f │ 312    │ 290K   │ 62K    │ $0.18      │  │
│  │ compact  │ ds-v4.1f │ 28     │ 48K    │ 3K     │ $0.10      │  │
│  └──────────┴──────────┴────────┴────────┴────────┴────────────┘  │
└─────────────────────────────────────────────────────────────────────┘
```

**Export**: JSON or CSV download with full per-request breakdown. CLI companion: `codex --usage-report [--since 24h] [--json]`.

#### F-1.6 Page: API Keys

**Purpose**: Manage self-issued API keys for external clients (Claude Code, Cursor, Cline, etc.).

**Layout**:
```
┌─────────────────────────────────────────────────────────────────────┐
│  🔑 API Keys                                    [+ Generate New Key]│
├─────────────────────────────────────────────────────────────────────┤
│                                                                     │
│  ┌─────────────────────────────────────────────────────────────┐   │
│  │ 🟢 cursor-work · pool-sk-a1b2...3f2a                         │   │
│  │    Combo: fast-coder · Format: chat · Rate: 100 RPM         │   │
│  │    Created: 2d ago · Last used: 12s ago · Requests: 3,420    │   │
│  │    IPs: 127.0.0.1, ::1 (local only)                         │   │
│  │    [Copy Key]  [Regenerate]  [Revoke]  [Disable]            │   │
│  ├─────────────────────────────────────────────────────────────┤   │
│  │ 🟢 codex-desktop · pool-sk-c4d5...8e9b                      │   │
│  │    Combo: 9router-subagent · Format: responses · Unlimited   │   │
│  │    Created: 5d ago · Last used: 45s ago · Requests: 8,120    │   │
│  │    IPs: any                                                  │   │
│  │    [Copy Key]  [Regenerate]  [Revoke]  [Disable]            │   │
│  ├─────────────────────────────────────────────────────────────┤   │
│  │ ⚫ claude-code-test · pool-sk-e6f7...1a2b                   │   │
│  │    Combo: heavy-worker · Format: chat · EXPIRED 1d ago       │   │
│  │    Created: 8d ago · Requests: 450                           │   │
│  │    [Delete]                                                 │   │
│  └─────────────────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────────────────┘
```

**Generate Key Modal**:
```
┌──── Generate New API Key ──────────────────────────┐
│                                                    │
│  Label:          [ cursor-laptop                 ] │
│  Default Combo:  [ fast-coder                  ▼ ] │
│  Format Hint:    (●) Auto  ( ) Chat  ( ) Responses │
│  Allowed Combos: [x] fast-coder  [x] heavy-worker  │
│                  [ ] code-reviewer                 │
│  Rate Limit RPM: [ 60    ] (empty = unlimited)     │
│  Rate Limit RPD: [ 5000  ] (empty = unlimited)     │
│  Expiry:         [ 30 days                     ▼ ] │
│  IP Whitelist:   [ 127.0.0.1, ::1                ] │
│                                                    │
│  [Cancel]                         [Generate Key]   │
└────────────────────────────────────────────────────┘
```

#### F-1.7 Page: Combos

**Purpose**: Manage model aliases and their routing strategies.

**Layout**:
```
┌─────────────────────────────────────────────────────────────────────┐
│  🎯 Combos                                           [+ New Combo] │
├─────────────────────────────────────────────────────────────────────┤
│                                                                     │
│  ┌──── fast-coder ─────────────────────────────────────────────┐   │
│  │ Strategy: 🔄 Round-robin                                    │   │
│  │ Description: Fast, cheap models for everyday coding          │   │
│  │                                                             │   │
│  │ Models in Pool (priority order):                            │   │
│  │   1. 🟢 nvidia / deepseek-ai/deepseek-v4.1-flash  (P50: 1.2s)│   │
│  │   2. 🟢 openrouter / deepseek/deepseek-chat-v4.1  (P50: 1.8s)│   │
│  │   3. 🟢 antigravity / gemini-2.5-flash            (P50: 0.9s)│   │
│  │                                                             │   │
│  │ Stats: 578 requests · 1.2M tokens · $1.20 · 0% errors      │   │
│  │ [+ Add Model]  [Edit Strategy]  [Delete]                    │   │
│  └─────────────────────────────────────────────────────────────┘   │
│                                                                     │
│  ┌──── heavy-worker ───────────────────────────────────────────┐   │
│  │ Strategy: ⬇️ Fallback                                       │   │
│  │ Description: Deep reasoning models for complex implementation│   │
│  │                                                             │   │
│  │ Models in Pool (priority order):                            │   │
│  │   1. 🟢 nvidia / moonshotai/kimi-k3               (P50: 3.2s)│   │
│  │   2. 🟢 openrouter / moonshotai/kimi-k3           (P50: 4.1s)│   │
│  │                                                             │   │
│  │ Stats: 420 requests · 1.5M tokens · $3.10 · 0.2% errors    │   │
│  │ [+ Add Model]  [Edit Strategy]  [Delete]                    │   │
│  └─────────────────────────────────────────────────────────────┘   │
│                                                                     │
│  ┌──── 9router-subagent (backward compat) ─────────────────────┐   │
│  │ Strategy: ⬇️ Fallback                                       │   │
│  │ Models: nvidia/deepseek-v4.1-flash → 9router/9router-subagent│   │
│  └─────────────────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────────────────┘
```

#### F-1.8 Page: Configuration

**Purpose**: View and (future: edit) the active proxy configuration without opening TOML files.

**Layout**:
```
┌─────────────────────────────────────────────────────────────────────┐
│  ⚙️ Configuration                              [Reload Config]     │
├─────────────────────────────────────────────────────────────────────┤
│                                                                     │
│  General                                                            │
│  ┌─────────────────────────────────────────────────────────────┐   │
│  │ Proxy Port:          20129                                  │   │
│  │ Gateway Port:        20130                                  │   │
│  │ Upstream:            https://chatgpt.com                    │   │
│  │ Primary Model:       gpt-6-luna                             │   │
│  │ Daemon PID:          12345                                  │   │
│  │ TLS Cert:            ✅ bridge-cert.pem (valid)             │   │
│  └─────────────────────────────────────────────────────────────┘   │
│                                                                     │
│  Active Provider Accounts (5 configured)                            │
│  ┌─────────────────────────────────────────────────────────────┐   │
│  │ nvidia-1       integrate.api.nvidia.com  🟢 Healthy         │   │
│  │ nvidia-2       integrate.api.nvidia.com  🟢 Healthy         │   │
│  │ openrouter-1   openrouter.ai             🟢 Healthy         │   │
│  │ antigravity-1  api.antigravity.dev       🟢 Healthy         │   │
│  │ 9router-leg    localhost:20128           🟡 Cooldown        │   │
│  └─────────────────────────────────────────────────────────────┘   │
│                                                                     │
│  Role → Model Mapping                                               │
│  ┌──────────────┬────────────────────────────────┬────────────┐   │
│  │ Role         │ Model                          │ Provider   │   │
│  ├──────────────┼────────────────────────────────┼────────────┤   │
│  │ default      │ deepseek-ai/deepseek-v4.1-flash│ nvidia     │   │
│  │ worker       │ moonshotai/kimi-k3             │ nvidia     │   │
│  │ explorer     │ z-ai/glm-5-3-flash             │ nvidia     │   │
│  │ reviewer     │ z-ai/glm-5.3                   │ nvidia     │   │
│  └──────────────┴────────────────────────────────┴────────────┘   │
│                                                                     │
│  Active Policies                                                    │
│  ┌─────────────────────────────────────────────────────────────┐   │
│  │ Approval Mode:          approve (autonomous)                │   │
│  │ Browser CDP:            enabled (Chrome 130.0.6723)         │   │
│  │ Spawn Agent Stripped:   ✅ (recursive delegation blocked)   │   │
│  │ Guardian Routing:       → ChatGPT (preserved)               │   │
│  └─────────────────────────────────────────────────────────────┘   │
│                                                                     │
│  Raw pool.toml  [View ▼]                                           │
│  Raw config.toml  [View ▼]                                         │
│  Raw agents/*.toml  [View ▼]                                       │
└─────────────────────────────────────────────────────────────────────┘
```

#### F-1.9 Page: Doctor

**Purpose**: Interactive version of `codex --doctor` with auto-refresh and fix suggestions.

**Layout**:
```
┌─────────────────────────────────────────────────────────────────────┐
│  🩺 Doctor                                [Run Full Check]          │
├─────────────────────────────────────────────────────────────────────┤
│  Last checked: 15s ago (auto-refreshes every 60s)                   │
│                                                                     │
│  ✅ Binary Hooks              codex-9router-subagents.exe in place  │
│  ✅ Stock Engine               codex.orig.exe v0.159.0-alpha.4      │
│  ✅ TLS Certificate            bridge-cert.pem valid, expires 2027  │
│  ✅ Loopback Proxy             :20129 bound, healthy                │
│  ✅ Proxy Daemon               PID 12345, uptime 4h 23m            │
│  ✅ CODEX_CLI_PATH             set in HKCU\Environment             │
│  ✅ NVIDIA_API_KEY             present (nvapi-****)                 │
│  ✅ NVIDIA Endpoint            integrate.api.nvidia.com reachable   │
│  ✅ 9Router Endpoint           localhost:20128 reachable            │
│  ✅ SQLite INSERT Trigger      state_*.sqlite ✓                    │
│  ✅ SQLite UPDATE Trigger      state_*.sqlite ✓                    │
│  ✅ models_cache.json          5 models synced                     │
│  ⚠️ Hooked Surfaces           5/6 (Windsurf ext not found)        │
│  ✅ Config.toml                valid, nvidia provider configured    │
│  ✅ Agent Manifests            4 roles configured                  │
│                                                                     │
│  Surface Status                                                     │
│  ┌─────────────┬──────────┬───────────────────┐                    │
│  │ Surface     │ Status   │ Engine Version    │                    │
│  ├─────────────┼──────────┼───────────────────┤                    │
│  │ Desktop App │ ✅ Hooked│ 0.159.0-alpha.4   │                    │
│  │ CLI         │ ✅ Hooked│ 0.159.0-alpha.4   │                    │
│  │ VS Code     │ ✅ Hooked│ 0.159.0-alpha.4   │                    │
│  │ Cursor      │ ✅ Hooked│ 0.159.0-alpha.4   │                    │
│  │ Antigravity │ ✅ Hooked│ 0.159.0-alpha.4   │                    │
│  │ Windsurf    │ ⚠️ N/A  │ Extension absent  │                    │
│  └─────────────┴──────────┴───────────────────┘                    │
└─────────────────────────────────────────────────────────────────────┘
```

**Interactions**:
- "Run Full Check" re-executes all diagnostic checks
- Warning/error items show expandable "Suggested Fix" text
- Auto-refresh every 60 seconds via WebSocket

#### F-1.10 Dashboard UI/UX Flow Summary

```mermaid
flowchart TD
    OPEN["User opens<br/>localhost:20130"] --> LIVE["Live Feed<br/>(default landing)"]

    LIVE -->|Click request row| DETAIL["Request Detail<br/>(slide-over panel)"]
    DETAIL -->|Close| LIVE

    LIVE -->|Nav: Subagents| SUB["Subagents Page"]
    SUB -->|Click session| TIMELINE["Session Timeline<br/>(inline expand)"]
    TIMELINE -->|Collapse| SUB

    LIVE -->|Nav: Providers| PROV["Providers &<br/>Accounts Page"]
    PROV -->|Toggle account| TOGGLE_ACCT["Enable/Disable account"]
    PROV -->|Test Connection| HEALTHCHECK["Inline health result"]

    LIVE -->|Nav: Usage| USAGE["Usage & Analytics"]
    USAGE -->|Change period| REFRESH["Re-query data"]
    USAGE -->|Export| DOWNLOAD["JSON/CSV download"]

    LIVE -->|Nav: API Keys| KEYS["API Keys Page"]
    KEYS -->|Generate New| KEYGEN["Key generation modal"]
    KEYS -->|Revoke/Disable| KEYMOD["Key state change"]

    LIVE -->|Nav: Combos| COMBOS["Combos Page"]
    COMBOS -->|Edit strategy| EDITCOMBO["Inline combo editor"]
    COMBOS -->|Add model| ADDMODEL["Model picker modal"]

    LIVE -->|Nav: Config| CONFIG["Config Page"]
    CONFIG -->|Reload| RELOAD["Re-read TOML files"]

    LIVE -->|Nav: Doctor| DOC["Doctor Page"]
    DOC -->|Run Full Check| RECHECK["Execute diagnostics"]

    LIVE -->|Sidebar toggle| PASSTHROUGH["Toggle Proxy<br/>ON ↔ OFF"]
    PASSTHROUGH -->|"ON → OFF"| DRAIN["Drain in-flight,<br/>switch to ChatGPT"]
    PASSTHROUGH -->|"OFF → ON"| POOL["Resume pool routing"]

    style LIVE fill:#6c5ce7,stroke:#a29bfe,color:#fff
    style DETAIL fill:#fd79a8,stroke:#e84393,color:#fff
    style USAGE fill:#00b894,stroke:#55efc4,color:#fff
    style KEYS fill:#fdcb6e,stroke:#ffeaa7,color:#2d3436
    style COMBOS fill:#e17055,stroke:#fab1a0,color:#fff
    style PASSTHROUGH fill:#636e72,stroke:#b2bec3,color:#fff
```

**Key UX Principles**:
1. **Zero setup** — dashboard starts automatically with `--proxy-daemon`, no extra install
2. **Real-time first** — WebSocket-driven, no manual refresh needed
3. **Non-intrusive** — dashboard is optional; proxy works identically without it
4. **Information density** — power-user oriented; show data, not decorations
5. **Keyboard navigable** — `1`–`8` keys jump between pages, `Esc` closes panels, `P` toggles proxy
6. **Mobile-excluded** — this is a developer tool on localhost, optimize for ≥1280px monitors
7. **Passthrough awareness** — sidebar always shows current proxy mode with visual indicator

---

### F-2 Provider-Specific Prompt Optimization

**Problem**: Different model backends have different strengths, prompt format preferences, and reasoning behaviors. A one-size-fits-all system prompt may not extract optimal performance from each model.

**Proposed Solution**: Auto-adapt system prompts, reasoning parameters, and request formatting per model:

```toml
[prompt_optimization.deepseek-v4-1-flash]
system_prompt_style = "concise"          # Minimal system prompts — model handles context well
reasoning_mode = "budget"                # Use numeric reasoning_effort (1-100)
default_reasoning_effort = 100           # Full reasoning for delegation tasks
max_output_tokens = 262144
strip_verbose_instructions = true        # Remove redundant Codex boilerplate from subagent prompts

[prompt_optimization.kimi-k3]
system_prompt_style = "detailed"         # Kimi benefits from explicit step-by-step instructions
reasoning_mode = "always_on"             # Reasoning is always enabled; only effort level changes
default_reasoning_effort = "high"        # Default to high for implementation tasks
preserve_reasoning_content = true        # Pass back reasoning_content in multi-turn history
max_output_tokens = 65536                # Kimi supports long outputs
tool_call_format = "openai_strict"       # Strict OpenAI function calling format

[prompt_optimization.glm-5-3-flash]
system_prompt_style = "structured"       # GLM responds well to structured XML-like prompts
reasoning_mode = "standard"              # Standard reasoning_effort parameter
max_output_tokens = 32768
clear_thinking = true                    # Separate thinking from final answer in chat
prefer_parallel_tool_calls = true        # GLM-Flash handles parallel tool calls efficiently

[prompt_optimization.glm-5-3]
system_prompt_style = "structured"
reasoning_mode = "standard"
default_reasoning_effort = "high"        # Complex tasks warrant deep reasoning
max_output_tokens = 32768
clear_thinking = true
security_audit_instructions = true       # Inject security-focused review instructions for reviewer role
```

**Scope**:
- System prompt formatting (XML tags, markdown headers, plain text, concise vs. detailed)
- Reasoning parameter adaptation (budget vs. effort vs. always-on)
- Output length calibration per model
- Tool call format preferences
- Context window utilization strategy (aggressive vs. conservative compaction thresholds)
- Role-specific instruction injection (security focus for reviewer, speed focus for explorer)

**Priority**: Medium · **Complexity**: High

---

### F-3 Proxy Self-Update

**Problem**: Users must manually download and run the installer to update the proxy binary.

**Proposed Solution**: Built-in self-update capability:

```powershell
codex --proxy-update           # Check and update
codex --proxy-update --check   # Check only, don't install
codex --proxy-update --rollback # Revert to previous binary
```

**Behavior**:
1. Query GitHub Releases API for latest version tag
2. Compare with current binary version
3. If newer: download `codex-9router-proxy-windows-amd64.zip`
4. Verify SHA-256 checksum (from release assets)
5. Extract and replace binary in `custom\` directory
6. Restart `--proxy-daemon` if running
7. Report update result

**Safety**:
- Backup current binary as `.prev.exe` before replacement
- Rollback on failed health check after update
- `--proxy-update --rollback` to manually revert

**Priority**: Medium · **Complexity**: Medium

---

## Appendix A: Glossary

| Term | Definition |
|:---|:---|
| **9Router** | Local LLM routing gateway (`npm i -g 9router`) that dispatches requests to configured AI model providers |
| **Codex Desktop App** | Official OpenAI Codex application distributed via Microsoft Store (`OpenAI.Codex`) |
| **Codex CLI** | Command-line interface for OpenAI Codex (`codex-cli`) |
| **codex.orig.exe** | The original unmodified Codex engine binary, preserved as a backup |
| **Guardian** | Internal Codex safety classifier (`guardian_classifier`, `guardian_review`) that reviews tool calls |
| **gpt-6-luna** | The primary ChatGPT model used for main conversation sessions |
| **IPC** | Inter-Process Communication — here, JSON-RPC 2.0 over stdio between Electron and the Codex engine |
| **Layer 1** | JSON-RPC stdio interception between Codex GUI and engine |
| **Layer 2** | Embedded HTTPS reverse proxy on `:20129` |
| **Layer 3** | Provider Pool Gateway on `:20130` — OpenAI-compatible API gateway with smart routing |
| **Combo** | Named model alias (e.g., `fast-coder`) mapping to a priority-ordered list of provider+model pairs |
| **Adaptive Routing** | Scoring algorithm that selects the optimal account per request based on health, quota, latency, and cost |
| **Self-Issued Key** | Bearer token (`pool-sk-*`) generated by the gateway for authenticating external clients |
| **Quota Tracking** | Implicit detection and management of account limits (RPM/RPD/credits) via HTTP response codes and headers |
| **Passthrough Mode** | Proxy state where Layers 1 & 2 stop intercepting — subagents route directly to ChatGPT using the user's signed-in account model |
| **Thread Affinity** | Routing optimization that keeps multi-turn subagent conversations on the same provider account to maximize prompt cache hits |
| **Request Deduplication** | In-memory LRU cache that serves identical responses to near-duplicate requests, saving tokens and latency |
| **System Tray Controller** | Windows notification area icon providing at-a-glance proxy status and quick toggle/dashboard access |
| **Antigravity** | Google Gemini provider integration via Antigravity API |
| **Poolside** | Coding-focused LLM provider integration |
| **Ollama Cloud** | Cloud-hosted Ollama-compatible endpoint integration |
| **Multi-Agent V2** | OpenAI's second-generation multi-agent protocol using `agent_message` types and `/root/<nickname>` paths |
| **Namespace bridging** | Flattening Codex plugin `"type": "namespace"` containers into `<ns>__<tool>` functions for provider compatibility |
| **NVIDIA NIM** | NVIDIA's hosted inference microservice API at `integrate.api.nvidia.com/v1`, OpenAI-compatible, authenticated via `nvapi-` keys |
| **DeepSeek-V4.1-Flash** | 552B MoE model (8B active) with 1M context, multimodal input, and controllable reasoning — cost-efficient default subagent |
| **GLM-5.3-Flash** | 320B MoE model (18B active) by Z.ai with hybrid KDA/sparse attention, multimodal, optimized for high-throughput serving |
| **GLM-5.3** | 753B MoE model (~40B active) by Z.ai with sparse attention, text-only, SOTA on coding and security benchmarks |
| **Kimi-K3** | 2.8T MoE model (104B active) by Moonshot AI with always-on reasoning, 1M context, multimodal, strongest implementation model |
| **Chat Completions API** | The `/v1/chat/completions` endpoint format used by NVIDIA NIM, distinct from the Responses API used by Codex |
| **SSE** | Server-Sent Events — the streaming protocol used for Responses API |
| **Surface** | A specific installation point for the Codex engine (Desktop, CLI, VS Code, Cursor, Windsurf, Antigravity) |
| **Subagent** | A child agent thread spawned by the primary coordinator via `spawn_agent` or `collaboration.spawn_agent` |

---

> **Change Process**: When modifying or adding functionality, update the relevant section of this PRD **before** implementing. Create or update the feature specification, adjust architecture diagrams if needed, and note any new configuration or environment variables.
