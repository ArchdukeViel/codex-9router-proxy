#Requires -Version 5.1
<#
.SYNOPSIS
    Interactive Installer for codex-9router-proxy.
    Routes OpenAI Codex Desktop & CLI subagents natively to 9Router or any custom OpenAI-compatible endpoint.

.DESCRIPTION
    Hooks official Codex Desktop & CLI binaries to intercept JSON-RPC messages and transparently
    route child subagent threads to 9Router, Ollama, LM Studio, OpenRouter, or vLLM while preserving
    your primary ChatGPT session on gpt-6-luna with zero proxy latency.

.PARAMETER Provider
    Model provider identifier (default: "9router").

.PARAMETER Endpoint
    Base URL for the provider endpoint (default: "http://localhost:20128/v1").

.PARAMETER ApiKey
    API Key for the provider. If omitted, reads from existing environment or registry.

.PARAMETER DefaultModel
    Default subagent model name (default: "9router-subagent").

.PARAMETER WorkerModel
    Subagent model for 'worker' role (implementation tasks).

.PARAMETER ExplorerModel
    Subagent model for 'explorer' role (codebase exploration & research).

.PARAMETER ReviewerModel
    Subagent model for 'reviewer' role (code review & auditing).

.PARAMETER Preset
    Preconfigured provider template: '9router', 'ollama', 'lmstudio', 'openrouter', 'vllm', 'litellm'.

.PARAMETER Doctor
    Runs diagnostics doctor check on active installation without modifying files.

.PARAMETER NonInteractive
    Suppresses interactive prompts, using parameter values, preset, and defaults.
#>

[CmdletBinding()]
param(
    [string]$Provider,
    [string]$Endpoint,
    [string]$ApiKey,
    [string]$DefaultModel,
    [string]$WorkerModel,
    [string]$ExplorerModel,
    [string]$ReviewerModel,
    [ValidateSet("read-only", "workspace-write", "danger-full-access")]
    [string]$SandboxMode = "danger-full-access",
    [ValidateSet("9router", "ollama", "lmstudio", "openrouter", "vllm", "litellm")]
    [string]$Preset,
    [switch]$Doctor,
    [switch]$NonInteractive
)

$ErrorActionPreference = "Stop"
$utf8NoBom = New-Object System.Text.UTF8Encoding($false)
$scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$targetBinary = Join-Path $scriptDir "target\release\codex-9router-proxy.exe"
$bundledBinary = Join-Path $scriptDir "codex-9router-proxy.exe"
$releaseBinary = if (Test-Path $targetBinary) { $targetBinary } elseif (Test-Path $bundledBinary) { $bundledBinary } else { $targetBinary }

function Clear-HiddenDesktopCodexGui {
    $chatGptPids = [int[]]@(Get-Process -Name "ChatGPT*" -ErrorAction SilentlyContinue | Select-Object -ExpandProperty Id)
    if ($chatGptPids.Count -gt 0) {
        if (-not ("CodexDesktopCheck" -as [type])) {
            Add-Type @"
using System;
using System.Text;
using System.Runtime.InteropServices;
using System.Collections.Generic;
public class CodexDesktopCheck {
    [UnmanagedFunctionPointer(CallingConvention.StdCall, CharSet = CharSet.Unicode)]
    public delegate bool EnumDesktopsDelegate([MarshalAs(UnmanagedType.LPWStr)] string desktop, IntPtr lParam);
    public delegate bool EnumWindowsProc(IntPtr hWnd, IntPtr lParam);
    [DllImport("user32.dll")] public static extern IntPtr GetProcessWindowStation();
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern bool EnumDesktopsW(IntPtr hwinsta, EnumDesktopsDelegate lpEnumFunc, IntPtr lParam);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern IntPtr OpenDesktopW(string lpszDesktop, uint dwFlags, bool fInherit, uint dwDesiredAccess);
    [DllImport("user32.dll")] public static extern bool CloseDesktop(IntPtr hDesktop);
    [DllImport("user32.dll")] public static extern bool EnumDesktopWindows(IntPtr hDesktop, EnumWindowsProc lpfn, IntPtr lParam);
    [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr hWnd, out uint lpdwProcessId);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetClassNameW(IntPtr hWnd, StringBuilder lpClassName, int nMaxCount);
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr hWnd);

    public static void ClassifyChatGptPids(int[] targetPids, out int[] defaultVisiblePids, out int[] defaultAnyPids, out int[] hiddenDesktopPids) {
        HashSet<int> targetSet = new HashSet<int>(targetPids ?? new int[0]);
        HashSet<int> defVis = new HashSet<int>();
        HashSet<int> defAny = new HashSet<int>();
        HashSet<int> hidden = new HashSet<int>();
        List<string> desktops = new List<string>();
        EnumDesktopsDelegate dCb = (d, l) => { desktops.Add(d); return true; };
        EnumDesktopsW(GetProcessWindowStation(), dCb, IntPtr.Zero);
        foreach (string dName in desktops) {
            IntPtr hDesk = OpenDesktopW(dName, 0, false, 0x01FF);
            if (hDesk == IntPtr.Zero) continue;
            bool isDefault = string.Equals(dName, "Default", StringComparison.OrdinalIgnoreCase);
            bool isChromiumSbox = dName.StartsWith("sbox_alternate_desktop", StringComparison.OrdinalIgnoreCase);
            EnumWindowsProc wCb = (hWnd, l) => {
                uint wpid = 0;
                GetWindowThreadProcessId(hWnd, out wpid);
                if (targetSet.Contains((int)wpid)) {
                    StringBuilder cls = new StringBuilder(256);
                    GetClassNameW(hWnd, cls, 256);
                    string clsName = cls.ToString();
                    if (clsName.StartsWith("Chrome_WidgetWin_", StringComparison.Ordinal)) {
                        if (isDefault) {
                            defAny.Add((int)wpid);
                            if (clsName == "Chrome_WidgetWin_1" && IsWindowVisible(hWnd)) {
                                defVis.Add((int)wpid);
                            }
                        } else if (!isChromiumSbox) {
                            hidden.Add((int)wpid);
                        }
                    }
                }
                return true;
            };
            EnumDesktopWindows(hDesk, wCb, IntPtr.Zero);
            CloseDesktop(hDesk);
        }
        hidden.ExceptWith(defAny);
        defaultVisiblePids = new List<int>(defVis).ToArray();
        defaultAnyPids = new List<int>(defAny).ToArray();
        hiddenDesktopPids = new List<int>(hidden).ToArray();
    }
}
"@ -ErrorAction SilentlyContinue
        }
        [int[]]$defaultVisiblePids = @()
        [int[]]$defaultAnyPids = @()
        [int[]]$hiddenDesktopPids = @()
        [CodexDesktopCheck]::ClassifyChatGptPids($chatGptPids, [ref]$defaultVisiblePids, [ref]$defaultAnyPids, [ref]$hiddenDesktopPids)
        if ($hiddenDesktopPids.Count -gt 0) {
            if ($defaultVisiblePids.Count -eq 0) {
                Write-Host "[WARN] Stopping hidden-desktop ChatGPT.exe instances (PIDs: $($hiddenDesktopPids -join ', ')) blocking Default desktop launch..." -ForegroundColor Yellow
                Get-Process -Name "ChatGPT*","codex-computer-use*" -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
                Start-Sleep -Milliseconds 500
            } else {
                Write-Host "[WARN] Stopping stray hidden-desktop ChatGPT.exe instances (PIDs: $($hiddenDesktopPids -join ', ')) while preserving Default desktop session..." -ForegroundColor Yellow
                foreach ($hpid in $hiddenDesktopPids) {
                    & taskkill.exe /F /PID $hpid /T 2>&1 | Out-Null
                }
                Start-Sleep -Milliseconds 300
            }
        }
    }
    if (-not (Get-Process -Name "ChatGPT*" -ErrorAction SilentlyContinue)) {
        $lockCandidates = @("$env:APPDATA\Codex\web\Codex\lockfile")
        $pkgLocks = Get-ChildItem "$env:LOCALAPPDATA\Packages\OpenAI.Codex*\LocalCache\Roaming\Codex\web\Codex\lockfile" -File -ErrorAction SilentlyContinue
        if ($pkgLocks) { $lockCandidates += $pkgLocks.FullName }
        $lockCandidates | Where-Object { Test-Path $_ } | Remove-Item -Force -ErrorAction SilentlyContinue
    }
}

function Check-McpServerStatuses {
    $codexCmd = Get-Command codex -ErrorAction SilentlyContinue
    if ($codexCmd) {
        try {
            $mcpList = & codex mcp list 2>&1
            $hasAuthIssue = $false
            foreach ($line in $mcpList) {
                if ($line -match "(\S+)\s+.*?\s+(OAuth|Not logged in)\s*$") {
                    $serverName = $Matches[1]
                    $authStatus = $Matches[2]
                    Write-Host "  [!] MCP Server '$serverName' auth status: $authStatus" -ForegroundColor Yellow
                    Write-Host "      -> Run: codex mcp login $serverName" -ForegroundColor Yellow
                    $hasAuthIssue = $true
                }
            }
            if (-not $hasAuthIssue) {
                Write-Host "[OK] All registered MCP servers report supported/ready auth." -ForegroundColor Green
            }
        } catch {}
    }
}

# If -Doctor requested, run diagnostic check immediately
if ($Doctor) {
    Clear-HiddenDesktopCodexGui
    if (Test-Path $releaseBinary) {
        & $releaseBinary --doctor
    } else {
        $customShim = Join-Path $env:LOCALAPPDATA "OpenAI\Codex\custom\codex-9router-subagents.exe"
        if (Test-Path $customShim) {
            & $customShim --doctor
        } else {
            Write-Error "No compiled codex-9router-proxy binary found. Run .\install.ps1 first to install."
        }
    }
    Check-McpServerStatuses
    return
}

Write-Host "===================================================================" -ForegroundColor Cyan
Write-Host "         Codex 9Router Proxy - Native GUI Subagents Installer      " -ForegroundColor Cyan
Write-Host "===================================================================" -ForegroundColor Cyan
Write-Host ""

# Apply presets if specified
if ($Preset) {
    switch ($Preset.ToLower()) {
        "9router" {
            if (-not $Provider) { $Provider = "9router" }
            if (-not $Endpoint) { $Endpoint = "http://localhost:20128/v1" }
            if (-not $DefaultModel) { $DefaultModel = "9router-subagent" }
            if (-not $WorkerModel) { $WorkerModel = $DefaultModel }
            if (-not $ExplorerModel) { $ExplorerModel = $DefaultModel }
            if (-not $ReviewerModel) { $ReviewerModel = $DefaultModel }
        }
        "ollama" {
            if (-not $Provider) { $Provider = "ollama" }
            if (-not $Endpoint) { $Endpoint = "http://localhost:11434/v1" }
            if (-not $DefaultModel) { $DefaultModel = "qwen2.5-coder:32b" }
            if (-not $WorkerModel) { $WorkerModel = $DefaultModel }
            if (-not $ExplorerModel) { $ExplorerModel = $DefaultModel }
            if (-not $ReviewerModel) { $ReviewerModel = $DefaultModel }
        }
        "lmstudio" {
            if (-not $Provider) { $Provider = "lmstudio" }
            if (-not $Endpoint) { $Endpoint = "http://localhost:1234/v1" }
            if (-not $DefaultModel) { $DefaultModel = "local-model" }
            if (-not $WorkerModel) { $WorkerModel = $DefaultModel }
            if (-not $ExplorerModel) { $ExplorerModel = $DefaultModel }
            if (-not $ReviewerModel) { $ReviewerModel = $DefaultModel }
        }
        "openrouter" {
            if (-not $Provider) { $Provider = "openrouter" }
            if (-not $Endpoint) { $Endpoint = "https://openrouter.ai/api/v1" }
            if (-not $DefaultModel) { $DefaultModel = "anthropic/claude-3.5-sonnet" }
            if (-not $WorkerModel) { $WorkerModel = $DefaultModel }
            if (-not $ExplorerModel) { $ExplorerModel = $DefaultModel }
            if (-not $ReviewerModel) { $ReviewerModel = $DefaultModel }
        }
        "vllm" {
            if (-not $Provider) { $Provider = "vllm" }
            if (-not $Endpoint) { $Endpoint = "http://localhost:8000/v1" }
            if (-not $DefaultModel) { $DefaultModel = "default-model" }
            if (-not $WorkerModel) { $WorkerModel = $DefaultModel }
            if (-not $ExplorerModel) { $ExplorerModel = $DefaultModel }
            if (-not $ReviewerModel) { $ReviewerModel = $DefaultModel }
        }
        "litellm" {
            if (-not $Provider) { $Provider = "litellm" }
            if (-not $Endpoint) { $Endpoint = "http://localhost:4000/v1" }
            if (-not $DefaultModel) { $DefaultModel = "default-model" }
            if (-not $WorkerModel) { $WorkerModel = $DefaultModel }
            if (-not $ExplorerModel) { $ExplorerModel = $DefaultModel }
            if (-not $ReviewerModel) { $ReviewerModel = $DefaultModel }
        }
    }
}

# 1. Gather Configuration (Interactive or Headless)
if (-not $NonInteractive) {
    if (-not $Provider) {
        $pInput = Read-Host "[?] Enter Model Provider name [default: 9router]"
        $Provider = if ([string]::IsNullOrWhiteSpace($pInput)) { "9router" } else { $pInput.Trim() }
    }

    if (-not $Endpoint) {
        $eInput = Read-Host "[?] Enter API Endpoint / Base URL [default: http://localhost:20128/v1]"
        $Endpoint = if ([string]::IsNullOrWhiteSpace($eInput)) { "http://localhost:20128/v1" } else { $eInput.Trim() }
    }

    if (-not $ApiKey) {
        $regKey = [System.Environment]::GetEnvironmentVariable("NINEROUTER_KEY", "User")
        $envKey = $env:NINEROUTER_KEY
        $existingKey = if (-not [string]::IsNullOrWhiteSpace($regKey)) { $regKey } else { $envKey }

        if (-not [string]::IsNullOrWhiteSpace($existingKey)) {
            $keyInput = Read-Host "[?] Enter API Key [press Enter to keep existing key]" -AsSecureString
            $bstr = [System.Runtime.InteropServices.Marshal]::SecureStringToBSTR($keyInput)
            $plain = [System.Runtime.InteropServices.Marshal]::PtrToStringAuto($bstr)
            [System.Runtime.InteropServices.Marshal]::ZeroFreeBSTR($bstr)
            $ApiKey = if ([string]::IsNullOrWhiteSpace($plain)) { $existingKey } else { $plain.Trim() }
        } else {
            $keyInput = Read-Host "[?] Enter API Key (optional)" -AsSecureString
            $bstr = [System.Runtime.InteropServices.Marshal]::SecureStringToBSTR($keyInput)
            $plain = [System.Runtime.InteropServices.Marshal]::PtrToStringAuto($bstr)
            [System.Runtime.InteropServices.Marshal]::ZeroFreeBSTR($bstr)
            $ApiKey = $plain.Trim()
        }
    }

    # Query upstream endpoint for available models to assist user selection
    $discoveredModels = @()
    try {
        $modelsEndpoint = "$($Endpoint.TrimEnd('/'))/models"
        $mHeaders = @{}
        if (-not [string]::IsNullOrWhiteSpace($ApiKey)) {
            $mHeaders["Authorization"] = "Bearer $ApiKey"
        }
        $mResp = Invoke-RestMethod -Uri $modelsEndpoint -Headers $mHeaders -TimeoutSec 3 -ErrorAction SilentlyContinue
        $mArr = if ($mResp.data) { $mResp.data } elseif ($mResp.models) { $mResp.models } else { @() }
        foreach ($item in $mArr) {
            $mid = if ($item.id) { $item.id } elseif ($item.slug) { $item.slug } else { $null }
            if ($mid) { $discoveredModels += $mid }
        }
    } catch {}

    if ($discoveredModels.Count -gt 0) {
        Write-Host ""
        Write-Host "[INFO] Discovered $($discoveredModels.Count) upstream models from $Endpoint/models." -ForegroundColor Cyan
        $sampleModels = ($discoveredModels | Select-Object -First 6) -join ", "
        Write-Host "       Available samples: $sampleModels" -ForegroundColor DarkGray
        Write-Host ""
    }

    # Model Configuration Prompts per Role
    if (-not $DefaultModel) {
        $dmInput = Read-Host "[?] Enter Default Subagent Model [default: 9router-subagent]"
        $DefaultModel = if ([string]::IsNullOrWhiteSpace($dmInput)) { "9router-subagent" } else { $dmInput.Trim() }
    }

    if (-not $WorkerModel) {
        $defaultWorker = $DefaultModel
        $wmInput = Read-Host "[?] Enter Worker Subagent Model [default: $defaultWorker]"
        $WorkerModel = if ([string]::IsNullOrWhiteSpace($wmInput)) { $defaultWorker } else { $wmInput.Trim() }
    }

    if (-not $ExplorerModel) {
        $defaultExplorer = $DefaultModel
        $emInput = Read-Host "[?] Enter Explorer Subagent Model [default: $defaultExplorer]"
        $ExplorerModel = if ([string]::IsNullOrWhiteSpace($emInput)) { $defaultExplorer } else { $emInput.Trim() }
    }

    if (-not $ReviewerModel) {
        $defaultReviewer = $DefaultModel
        $rmInput = Read-Host "[?] Enter Reviewer Subagent Model [default: $defaultReviewer]"
        $ReviewerModel = if ([string]::IsNullOrWhiteSpace($rmInput)) { $defaultReviewer } else { $rmInput.Trim() }
    }
} else {
    if (-not $Provider) { $Provider = "9router" }
    if (-not $Endpoint) { $Endpoint = "http://localhost:20128/v1" }
    if (-not $ApiKey) {
        $ApiKey = [System.Environment]::GetEnvironmentVariable("NINEROUTER_KEY", "User")
    }
    if (-not $DefaultModel) { $DefaultModel = "9router-subagent" }
    if (-not $WorkerModel) { $WorkerModel = $DefaultModel }
    if (-not $ExplorerModel) { $ExplorerModel = $DefaultModel }
    if (-not $ReviewerModel) { $ReviewerModel = $DefaultModel }
}

$Endpoint = $Endpoint.TrimEnd('/')

Write-Host ""
Write-Host "[+] Target Configuration:" -ForegroundColor Green
Write-Host "    Provider       : $Provider"
Write-Host "    Endpoint       : $Endpoint"
Write-Host "    API Key        : [PROTECTED / CONFIGURED]"
Write-Host "    Default Model  : $DefaultModel"
Write-Host "    Worker Model   : $WorkerModel"
Write-Host "    Explorer Model : $ExplorerModel"
Write-Host "    Reviewer Model : $ReviewerModel"
Write-Host "    Sandbox Mode   : $SandboxMode"
Write-Host ""

# 2. Persist Environment Variables & Registry
function Set-FastUserEnv {
    param([string]$Name, [string]$Value)
    Set-ItemProperty -Path "HKCU:\Environment" -Name $Name -Value $Value -Force
    Set-Item -Path "env:$Name" -Value $Value
}

# 2. Persist Environment Variables & Registry
if (-not [string]::IsNullOrWhiteSpace($ApiKey)) {
    Set-FastUserEnv "NINEROUTER_KEY" $ApiKey
    Write-Host "[OK] Saved NINEROUTER_KEY to Windows User Environment (Registry HKCU\Environment)." -ForegroundColor Green
}
Set-FastUserEnv "CODEX_SUBAGENT_PROVIDER" $Provider
Set-FastUserEnv "CODEX_SUBAGENT_ENDPOINT" $Endpoint
Set-FastUserEnv "CODEX_DEFAULT_MODEL" $DefaultModel
Set-FastUserEnv "CODEX_WORKER_MODEL" $WorkerModel
Set-FastUserEnv "CODEX_EXPLORER_MODEL" $ExplorerModel
Set-FastUserEnv "CODEX_REVIEWER_MODEL" $ReviewerModel

$customDir = Join-Path $env:LOCALAPPDATA "OpenAI\Codex\custom"
$customShim = Join-Path $customDir "codex-9router-subagents.exe"
Set-FastUserEnv "CODEX_CLI_PATH" $customShim
Write-Host "[OK] Saved CODEX_CLI_PATH -> $customShim (User Environment)." -ForegroundColor Green

# Detect system browser (Chrome or Edge across standard 64-bit, 32-bit, and per-user local appdata paths)
$browserCandidate = @(
    $env:AGENT_BROWSER_EXECUTABLE_PATH,
    (Join-Path $env:ProgramFiles "Google\Chrome\Application\chrome.exe"),
    (Join-Path ${env:ProgramFiles(x86)} "Google\Chrome\Application\chrome.exe"),
    (Join-Path $env:LOCALAPPDATA "Google\Chrome\Application\chrome.exe"),
    (Join-Path ${env:ProgramFiles(x86)} "Microsoft\Edge\Application\msedge.exe"),
    (Join-Path $env:ProgramFiles "Microsoft\Edge\Application\msedge.exe")
) | Where-Object { $_ -and (Test-Path $_) } | Select-Object -First 1

if ($browserCandidate) {
    Set-FastUserEnv "AGENT_BROWSER_EXECUTABLE_PATH" $browserCandidate
    Write-Host "[OK] Detected system browser: $browserCandidate (User Environment)." -ForegroundColor Green
}

# Ensure %LOCALAPPDATA%\OpenAI\Codex\custom is at index 0 of User PATH (ahead of Programs\OpenAI\Codex\bin and .cargo\bin)
$normalizedCustomDir = $customDir.TrimEnd('\')
$userPath = (Get-ItemProperty -Path "HKCU:\Environment" -Name "Path" -ErrorAction SilentlyContinue).Path
$userPathEntries = if ([string]::IsNullOrWhiteSpace($userPath)) {
    @()
} else {
    @($userPath -split ';' | Where-Object { -not [string]::IsNullOrWhiteSpace($_) })
}
$remainingUserPathEntries = @(
    $userPathEntries | Where-Object { $_.Trim().TrimEnd('\') -ine $normalizedCustomDir }
)
$newUserPath = (@($customDir) + $remainingUserPathEntries) -join ';'
if ($newUserPath -ne $userPath) {
    Set-FastUserEnv "Path" $newUserPath
    Write-Host "[OK] Placed $customDir at index 0 of Windows User PATH." -ForegroundColor Green
} else {
    Write-Host "[OK] $customDir is already at index 0 of Windows User PATH." -ForegroundColor Green
}
$procPathEntries = @(
    ($env:Path -split ';') | Where-Object { -not [string]::IsNullOrWhiteSpace($_) -and ($_.Trim().TrimEnd('\') -ine $normalizedCustomDir) }
)
$env:Path = (@($customDir) + $procPathEntries) -join ';'

$parsedPort = 0
$userPortRaw = [System.Environment]::GetEnvironmentVariable("CODEX_PROXY_PORT", "User")
$proxyPort = if (-not [string]::IsNullOrWhiteSpace($env:CODEX_PROXY_PORT) -and [int]::TryParse($env:CODEX_PROXY_PORT.Trim(), [ref]$parsedPort) -and $parsedPort -gt 0) {
    $parsedPort
} elseif (-not [string]::IsNullOrWhiteSpace($userPortRaw) -and [int]::TryParse($userPortRaw.Trim(), [ref]$parsedPort) -and $parsedPort -gt 0) {
    $parsedPort
} else {
    20129
}

function ConvertTo-TomlEscapedString {
    param([string]$Value)
    if ($null -eq $Value) { return "" }
    return $Value.Replace('\', '\\').Replace('"', '\"')
}

# 3. Configure ~/.codex/config.toml (BOM-Free UTF-8)
$codexDir = if (-not [string]::IsNullOrWhiteSpace($env:CODEX_HOME)) { $env:CODEX_HOME.Trim() } else { Join-Path $env:USERPROFILE ".codex" }
if (-not (Test-Path $codexDir)) {
    New-Item -ItemType Directory -Path $codexDir -Force | Out-Null
}
$configFile = Join-Path $codexDir "config.toml"
$configContent = if (Test-Path $configFile) {
    [System.IO.File]::ReadAllText($configFile, [System.Text.Encoding]::UTF8)
} else {
    ""
}

$escapedProvider = ConvertTo-TomlEscapedString $Provider
$escapedEndpoint = ConvertTo-TomlEscapedString $Endpoint
$escapedDefaultModel = ConvertTo-TomlEscapedString $DefaultModel
$providerRegexKey = [regex]::Escape($Provider)

# Update or insert [model_providers.<Provider>]
$providerSection = @"
[model_providers.$Provider]
name = "$escapedProvider"
base_url = "$escapedEndpoint"
env_key = "NINEROUTER_KEY"
"@
$pattern = "(?ms)\[model_providers\.$providerRegexKey\].*?(?=\n\[|\z)"
if ($configContent -match $pattern) {
    $configContent = [System.Text.RegularExpressions.Regex]::Replace($configContent, $pattern, { $providerSection.Trim() })
} else {
    $configContent = $configContent.TrimEnd() + "`r`n`r`n" + $providerSection.Trim() + "`r`n"
}

# Ensure [agents] default_subagent_model is set
$agentsSection = @"
[agents]
default_subagent_model = "$escapedDefaultModel"
"@
if ($configContent -match "(?m)^\[agents\]") {
    if ($configContent -match "(?m)^default_subagent_model\s*=") {
        $configContent = [System.Text.RegularExpressions.Regex]::Replace($configContent, "(?m)^default_subagent_model\s*=.*", { "default_subagent_model = `"$escapedDefaultModel`"" })
    } else {
        $configContent = [System.Text.RegularExpressions.Regex]::Replace($configContent, "(?m)^\[agents\]", { "[agents]`r`ndefault_subagent_model = `"$escapedDefaultModel`"" })
    }
} else {
    $configContent = $configContent.TrimEnd() + "`r`n`r`n" + $agentsSection.Trim() + "`r`n"
}

# Remove legacy [subagent_models] section if present (avoids codex config warning; role models live in ~/.codex/agents/*.toml)
$patternSubModels = "(?ms)\r?\n?\[subagent_models\].*?(?=\r?\n\[|\z)"
if ($configContent -match $patternSubModels) {
    $configContent = [System.Text.RegularExpressions.Regex]::Replace($configContent, $patternSubModels, "")
}

# Remove legacy [mcp_servers.subagent_9router] section if present (superseded by native GUI subagents via collaboration.spawn_agent)
$patternMcpSubagent = "(?ms)\r?\n?\[mcp_servers\.subagent_9router\].*?(?=\r?\n\[|\z)"
if ($configContent -match $patternMcpSubagent) {
    $configContent = [System.Text.RegularExpressions.Regex]::Replace($configContent, $patternMcpSubagent, "")
}

# Link Role Manifests in config.toml
$agentsDir = Join-Path $codexDir "agents"
if (-not (Test-Path $agentsDir)) {
    New-Item -ItemType Directory -Path $agentsDir -Force | Out-Null
}

$roleConfigs = @{
    "default"  = @{ Desc = "General-purpose subagent"; Model = $DefaultModel }
    "worker"   = @{ Desc = "Implementation-focused subagent"; Model = $WorkerModel }
    "explorer" = @{ Desc = "Research and exploration subagent"; Model = $ExplorerModel }
    "reviewer" = @{ Desc = "Code review subagent"; Model = $ReviewerModel }
}

foreach ($roleName in $roleConfigs.Keys) {
    $roleInfo = $roleConfigs[$roleName]
    $roleTomlPath = (Join-Path $agentsDir "$roleName.toml") -replace "\\", "/"
    $escapedRoleName = ConvertTo-TomlEscapedString $roleName
    $escapedRoleDesc = ConvertTo-TomlEscapedString $roleInfo.Desc
    $escapedRoleModel = ConvertTo-TomlEscapedString $roleInfo.Model
    $escapedSandboxMode = ConvertTo-TomlEscapedString $SandboxMode
    $escapedRoleTomlPath = ConvertTo-TomlEscapedString $roleTomlPath
    $roleRegexKey = [regex]::Escape($roleName)

    # Write ~/.codex/agents/<role>.toml
    $roleTomlContent = @"
name = "$escapedRoleName"
description = "$escapedRoleDesc"
model = "$escapedRoleModel"
model_provider = "$escapedProvider"
model_reasoning_effort = "high"
sandbox_mode = "$escapedSandboxMode"
developer_instructions = """You are a dedicated worker subagent executing tasks on behalf of /root.
You have full shell execution capabilities via exec_command in PowerShell.
IMPORTANT: For browser automation, use the configured AGENT_BROWSER_EXECUTABLE_PATH:
  if (-not `$env:AGENT_BROWSER_EXECUTABLE_PATH) { `$env:AGENT_BROWSER_EXECUTABLE_PATH = @((Join-Path `$env:ProgramFiles 'Google\\Chrome\\Application\\chrome.exe'), (Join-Path `${env:ProgramFiles(x86)} 'Google\\Chrome\\Application\\chrome.exe'), (Join-Path `$env:LOCALAPPDATA 'Google\\Chrome\\Application\\chrome.exe'), (Join-Path `${env:ProgramFiles(x86)} 'Microsoft\\Edge\\Application\\msedge.exe'), (Join-Path `$env:ProgramFiles 'Microsoft\\Edge\\Application\\msedge.exe')) | Where-Object { Test-Path `$_ } | Select-Object -First 1 }
- To perform browser checks, use agent-browser CLI via exec_command:
  agent-browser open https://example.com && agent-browser get title
- Alternatively, use Node.js Playwright with channel chrome:
  node -e \"const { chromium } = require('playwright'); (async () => { const b = await chromium.launch({channel:'chrome'}); const p = await b.newPage(); await p.goto('https://example.com'); console.log(await p.title()); await b.close(); })();\"
- To perform Git checks, execute git commands via exec_command, preserving exact requested chaining (e.g. &&).
- To perform health or network queries, query endpoints via exec_command (e.g. Invoke-RestMethod or curl.exe).
Directly execute assigned tasks and report your complete findings in your final answer."""
"@
    $targetToml = Join-Path $agentsDir "$roleName.toml"
    [System.IO.File]::WriteAllText($targetToml, $roleTomlContent, $utf8NoBom)

    # Link in config.toml
    $roleBlock = @"
[agents.$roleName]
description = "$escapedRoleDesc"
config_file = "$escapedRoleTomlPath"
"@
    $patternRole = "(?ms)\[agents\.$roleRegexKey\].*?(?=\n\[|\z)"
    if ($configContent -match $patternRole) {
        $configContent = [System.Text.RegularExpressions.Regex]::Replace($configContent, $patternRole, { $roleBlock.Trim() })
    } else {
        $configContent = $configContent.TrimEnd() + "`r`n`r`n" + $roleBlock.Trim() + "`r`n"
    }
}

# 3b. Configure Always-Allow Policies, Browser CDP Access & MCP Approvals
if ($configContent -match "(?m)^approvals_reviewer\s*=") {
    $configContent = [System.Text.RegularExpressions.Regex]::Replace($configContent, "(?m)^approvals_reviewer\s*=.*", 'approvals_reviewer = "auto_review"')
} else {
    $configContent = "approvals_reviewer = `"auto_review`"`r`n" + $configContent
}

$appsDefaultSection = @"
[apps._default]
default_tools_approval_mode = "approve"
"@
$patternApps = "(?ms)\[apps\._default\].*?(?=\r?\n\[|\z)"
if ($configContent -match $patternApps) {
    $configContent = [System.Text.RegularExpressions.Regex]::Replace($configContent, $patternApps, { $appsDefaultSection.Trim() })
} else {
    $configContent = $configContent.TrimEnd() + "`r`n`r`n" + $appsDefaultSection.Trim() + "`r`n"
}

# Remove legacy/conflicting [approval_policy.granular] section if present (violates TOML syntax when approval_policy is defined as a string)
$patternGranular = "(?ms)\r?\n?\[approval_policy\.granular\].*?(?=\r?\n\[|\z)"
if ($configContent -match $patternGranular) {
    $configContent = [System.Text.RegularExpressions.Regex]::Replace($configContent, $patternGranular, "")
}

# Fix invalid tool approval_mode = "never" -> "auto" (expected one of 'auto', 'prompt', 'writes', 'approve')
$configContent = [System.Text.RegularExpressions.Regex]::Replace($configContent, '(?m)^approval_mode\s*=\s*"never"', 'approval_mode = "auto"')
$configContent = [System.Text.RegularExpressions.Regex]::Replace($configContent, '(?m)^default_tools_approval_mode\s*=\s*"never"', 'default_tools_approval_mode = "auto"')


$browserUseSection = @"
[browser_use]
allow_history_access = true

[browser_use.default_origin_policy]
access = "allow"
full_cdp_access = "allow"
downloads = "allow"
uploads = "allow"
"@
$patternBrowser = "(?ms)\[browser_use(?:\.[^\]]+)?\].*?(?=\r?\n\[(?!browser_use)|\z)"
if ($configContent -match $patternBrowser) {
    $configContent = [System.Text.RegularExpressions.Regex]::Replace($configContent, $patternBrowser, { $browserUseSection.Trim() })
} else {
    $configContent = $configContent.TrimEnd() + "`r`n`r`n" + $browserUseSection.Trim() + "`r`n"
}

# Ensure all [mcp_servers.*] blocks have default_tools_approval_mode = "approve"
$configContent = [System.Text.RegularExpressions.Regex]::Replace(
    $configContent,
    "(?ms)(\[mcp_servers\.[^\]]+\])(.*?)(?=\r?\n\[|\z)",
    {
        param($m)
        $header = $m.Groups[1].Value
        $body = $m.Groups[2].Value
        if ($body -match "(?m)^default_tools_approval_mode\s*=") {
            $body = [System.Text.RegularExpressions.Regex]::Replace($body, "(?m)^default_tools_approval_mode\s*=.*", 'default_tools_approval_mode = "approve"')
        } else {
            $body = $body.TrimEnd() + "`r`ndefault_tools_approval_mode = `"approve`"`r`n"
        }
        if ($header -match "playwright") {
            $body = [System.Text.RegularExpressions.Regex]::Replace($body, ',?\s*"--(?:test-type|silent-debugger-extension-api)"', '')
        }
        return "$header$body"
    }
)

# Ensure chatgpt_base_url is clean / direct
if ($configContent -match "(?m)^chatgpt_base_url\s*=") {
    $configContent = [System.Text.RegularExpressions.Regex]::Replace($configContent, "(?m)^chatgpt_base_url\s*=.*`r?`n", "")
}

[System.IO.File]::WriteAllText($configFile, $configContent, $utf8NoBom)
Write-Host "[OK] Configured $configFile and role manifests in $agentsDir (BOM-Free UTF-8)." -ForegroundColor Green

# 4. Inject SQLite Automatic Provider Trigger into Databases
$dbCandidateList = New-Object System.Collections.Generic.List[string]
foreach ($searchDir in @($codexDir, (Join-Path $codexDir "sqlite"))) {
    if (Test-Path $searchDir) {
        Get-ChildItem -Path $searchDir -Filter "state_*.sqlite" -File -ErrorAction SilentlyContinue | ForEach-Object {
            if (-not $dbCandidateList.Contains($_.FullName)) {
                $dbCandidateList.Add($_.FullName)
            }
        }
        $devDb = Join-Path $searchDir "codex-dev.db"
        if ((Test-Path $devDb) -and -not $dbCandidateList.Contains($devDb)) {
            $dbCandidateList.Add($devDb)
        }
    }
}
$dbCandidates = @($dbCandidateList)

$hasPython = $false
if (Get-Command python -ErrorAction SilentlyContinue) {
    try {
        & python -c "import sqlite3" 2>$null | Out-Null
        if ($LASTEXITCODE -eq 0) { $hasPython = $true }
    } catch {}
}
if ($hasPython) {
    $pyScript = @'
import sqlite3, sys
db_path = sys.argv[1]
provider = sys.argv[2]
models = list(sys.argv[3:]) + ['implement', 'explore', 'review']
def q(s):
    return "'" + s.replace("'", "''") + "'"
unique_models = []
for m in models:
    m_clean = m.strip()
    if m_clean and m_clean not in unique_models:
        unique_models.append(m_clean)
models_in_clause = ", ".join(q(m) for m in unique_models)
provider_lit = q(provider.strip() or '9router')
required_cols = {'id', 'model', 'model_provider', 'agent_role', 'thread_source'}
try:
    conn = sqlite3.connect(db_path, timeout=5.0)
    cur = conn.cursor()
    cur.execute("DROP TRIGGER IF EXISTS fix_subagent_provider_trigger;")
    cur.execute("DROP TRIGGER IF EXISTS fix_subagent_provider_update_trigger;")
    cur.execute("SELECT 1 FROM sqlite_master WHERE type='table' AND name='threads'")
    if cur.fetchone() is not None:
        cur.execute("PRAGMA table_info(threads);")
        existing_cols = {row[1] for row in cur.fetchall()}
        if required_cols.issubset(existing_cols):
            cur.execute(f"""
            CREATE TRIGGER fix_subagent_provider_trigger
            AFTER INSERT ON threads
            FOR EACH ROW
            WHEN COALESCE(NEW.agent_role, '') NOT IN ('guardian_classifier', 'guardian_review')
             AND (NEW.agent_role IS NOT NULL OR NEW.thread_source = 'subagent' OR NEW.thread_source LIKE '%subagent%' OR NEW.model LIKE '%9router%' OR NEW.model IN ({models_in_clause}))
            BEGIN
                UPDATE threads SET model_provider = {provider_lit} WHERE id = NEW.id;
            END;
            """)
            cur.execute(f"""
            CREATE TRIGGER fix_subagent_provider_update_trigger
            AFTER UPDATE OF model_provider, model ON threads
            FOR EACH ROW
            WHEN NEW.model_provider != {provider_lit}
             AND COALESCE(NEW.agent_role, '') NOT IN ('guardian_classifier', 'guardian_review')
             AND (NEW.agent_role IS NOT NULL OR NEW.thread_source = 'subagent' OR NEW.thread_source LIKE '%subagent%' OR NEW.model LIKE '%9router%' OR NEW.model IN ({models_in_clause}))
            BEGIN
                UPDATE threads SET model_provider = {provider_lit} WHERE id = NEW.id;
            END;
            """)
    conn.commit()
    conn.close()
    sys.exit(0)
except Exception as e:
    sys.stderr.write(str(e))
    sys.exit(1)
'@
    foreach ($db in $dbCandidates) {
        if (Test-Path $db) {
            $pyScript | & python - $db $Provider $DefaultModel $WorkerModel $ExplorerModel $ReviewerModel
            if ($LASTEXITCODE -eq 0) {
                Write-Host "[OK] Injected SQLite subagent trigger into $db." -ForegroundColor Green
            } else {
                Write-Warning "Failed to inject trigger into $db via python."
            }
        }
    }
} else {
    Write-Host "[i] Python not detected; SQLite trigger injection skipped (handled by proxy layer)." -ForegroundColor Gray
}

# 5. Locate Prebuilt Release Binary or Compile from Source
$cargoToml = Join-Path $scriptDir "Cargo.toml"
$hasCargo = [bool](Get-Command cargo -ErrorAction SilentlyContinue)
if ((Test-Path $cargoToml) -and $hasCargo) {
    Write-Host "[*] Compiling release binary with cargo..." -ForegroundColor Yellow
    Push-Location $scriptDir
    try {
        & cargo build --release
        if ($LASTEXITCODE -ne 0) {
            throw "Cargo build failed with exit code $LASTEXITCODE"
        }
    } finally {
        Pop-Location
    }
    $releaseBinary = $targetBinary
} elseif (Test-Path $bundledBinary) {
    $releaseBinary = $bundledBinary
    Write-Host "[OK] Found prebuilt release binary ($bundledBinary)." -ForegroundColor Green
} elseif (Test-Path $targetBinary) {
    $releaseBinary = $targetBinary
    Write-Host "[OK] Found compiled release binary ($targetBinary)." -ForegroundColor Green
} else {
    throw "No prebuilt codex-9router-proxy.exe found in $scriptDir, and Rust/Cargo is not installed to build from source."
}

if (-not (Test-Path $releaseBinary)) {
    throw "Release binary not found at $releaseBinary"
}
Write-Host "[OK] Release binary ready ($((Get-Item $releaseBinary).Length) bytes)." -ForegroundColor Green

# 6. Stop running codex processes (excluding ancestor PIDs in ParentProcessId chain), hidden-desktop ChatGPT.exe instances, and verified port listeners before hooking
if (-not ("CodexProcessNative" -as [type])) {
    Add-Type @"
using System;
using System.Text;
using System.Runtime.InteropServices;
public class CodexProcessNative {
    [DllImport("kernel32.dll", SetLastError = true)]
    public static extern IntPtr OpenProcess(uint dwDesiredAccess, bool bInheritHandle, int dwProcessId);
    [DllImport("kernel32.dll", SetLastError = true)]
    public static extern bool CloseHandle(IntPtr hObject);
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    public static extern bool QueryFullProcessImageNameW(IntPtr hProcess, uint dwFlags, StringBuilder lpExeName, ref uint lpdwSize);

    public static string GetProcessImagePath(int pid) {
        if (pid <= 0) return null;
        IntPtr hProc = OpenProcess(0x1000, false, pid);
        if (hProc == IntPtr.Zero) return null;
        try {
            StringBuilder sb = new StringBuilder(2048);
            uint size = (uint)sb.Capacity;
            if (QueryFullProcessImageNameW(hProc, 0, sb, ref size) && size > 0) {
                return sb.ToString();
            }
            return null;
        } finally {
            CloseHandle(hProc);
        }
    }

    public static string GetLiveImagePath(int pid) {
        return GetProcessImagePath(pid);
    }
}
"@ -ErrorAction SilentlyContinue
}

function Get-LiveProcessImagePath {
    param($Process)
    if (-not $Process) { return $null }
    try {
        if ("CodexProcessNative" -as [type]) {
            $livePath = [CodexProcessNative]::GetProcessImagePath([int]$Process.Id)
            if (-not [string]::IsNullOrWhiteSpace($livePath)) {
                return $livePath
            }
        }
    } catch {}
    try { return $Process.Path } catch { return $null }
}

function Test-ProxyDaemonRunning {
    param([string]$CustomDir, [int]$Port)
    $lockFile = Join-Path $CustomDir "proxy-daemon-$Port.lock"
    if (Test-Path $lockFile) {
        try {
            $fs = [System.IO.File]::Open($lockFile, [System.IO.FileMode]::Open, [System.IO.FileAccess]::Read, [System.IO.FileShare]::None)
            $fs.Close()
            $fs.Dispose()
        } catch {
            return $true
        }
    }
    $daemonProc = Get-CimInstance Win32_Process -Filter "Name = 'codex-9router-subagents.exe' OR Name = 'codex-9router-proxy.exe' OR Name = 'codex.exe'" -ErrorAction SilentlyContinue |
        Where-Object { $_.CommandLine -match '--proxy-daemon' }
    return [bool]$daemonProc
}

function Get-AncestorProcessIds {
    $ancestors = New-Object 'System.Collections.Generic.HashSet[int]'
    $curPid = $PID
    for ($depth = 0; $depth -lt 32 -and $curPid -gt 0; $depth++) {
        [void]$ancestors.Add([int]$curPid)
        try {
            $procInfo = Get-CimInstance Win32_Process -Filter "ProcessId = $curPid" -ErrorAction SilentlyContinue
            if ($procInfo -and $procInfo.ParentProcessId -gt 0 -and -not $ancestors.Contains([int]$procInfo.ParentProcessId)) {
                $curPid = [int]$procInfo.ParentProcessId
            } else {
                break
            }
        } catch {
            break
        }
    }
    return $ancestors
}

$ancestorPids = Get-AncestorProcessIds
$exactCodexProcessNames = @(
    "codex",
    "codex.orig",
    "codex-9router-proxy",
    "codex-9router-subagents",
    "codex-9router-subagents.orig",
    "codex-command-runner",
    "codex-code-mode-host",
    "codex-windows-sandbox-setup",
    "codex-windows-sandbox-service"
)
Write-Host "[*] Checking for running codex processes..." -ForegroundColor Gray
Clear-HiddenDesktopCodexGui
Get-NetTCPConnection -LocalPort $proxyPort -ErrorAction SilentlyContinue | ForEach-Object {
    $procId = [int]$_.OwningProcess
    if ($procId -gt 0 -and -not $ancestorPids.Contains($procId)) {
        $ownerProc = Get-Process -Id $procId -ErrorAction SilentlyContinue
        $ownerLivePath = Get-LiveProcessImagePath $ownerProc
        if ($ownerProc -and ($exactCodexProcessNames -contains $ownerProc.Name -or $ownerProc.Name -like "codex*.old*" -or ($ownerLivePath -and $ownerLivePath -like "*.old.*"))) {
            Stop-Process -Id $procId -Force -ErrorAction SilentlyContinue
        }
    }
}
Get-Process -ErrorAction SilentlyContinue | Where-Object {
    (-not $ancestorPids.Contains([int]$_.Id)) -and (
        $exactCodexProcessNames -contains $_.Name -or
        $_.Name -like "codex*.old*" -or
        ($_.Name -like "codex*" -and ((Get-LiveProcessImagePath $_) -like "*.old.*"))
    )
} | ForEach-Object {
    Write-Host "    Stopping process $($_.Name) (PID $($_.Id))..." -ForegroundColor Gray
    Stop-Process -Id $_.Id -Force -ErrorAction SilentlyContinue
}
Start-Sleep -Milliseconds 500

function Test-StockCodexHealthy {
    param([string]$ExePath)
    if (-not (Test-Path $ExePath)) { return $false }
    try {
        $psi = New-Object System.Diagnostics.ProcessStartInfo
        $psi.FileName = $ExePath
        $psi.Arguments = "--version"
        $psi.UseShellExecute = $false
        $psi.CreateNoWindow = $true
        $psi.RedirectStandardOutput = $true
        $psi.RedirectStandardError = $true
        $proc = [System.Diagnostics.Process]::Start($psi)
        if ($proc.WaitForExit(3000)) {
            return ($proc.ExitCode -eq 0)
        }
        try { $proc.Kill() } catch {}
        return $false
    } catch {
        return $false
    }
}

function Safe-CopyExecutable {
    param(
        [string]$Source,
        [string]$Destination
    )
    $destDir = Split-Path -Parent $Destination
    $fileName = Split-Path -Leaf $Destination
    $srcTimeUtc = $null
    if (Test-Path $Source) {
        $srcTimeUtc = (Get-Item $Source).LastWriteTimeUtc
    }
    if (-not (Test-Path $Destination)) {
        Copy-Item -Path $Source -Destination $Destination -Force
        if ($srcTimeUtc -and (Test-Path $Destination)) {
            (Get-Item $Destination).LastWriteTimeUtc = $srcTimeUtc
        }
        return $true
    }
    $maxRetries = 5
    for ($attempt = 1; $attempt -le $maxRetries; $attempt++) {
        try {
            Copy-Item -Path $Source -Destination $Destination -Force -ErrorAction Stop
            if ($srcTimeUtc -and (Test-Path $Destination)) {
                (Get-Item $Destination).LastWriteTimeUtc = $srcTimeUtc
            }
            return $true
        } catch {
            if ($attempt -lt $maxRetries) {
                Start-Sleep -Milliseconds 600
            } else {
                $tempOld = Join-Path $destDir "$fileName.old.$([Guid]::NewGuid().ToString('N').Substring(0,6))"
                try {
                    Move-Item -Path $Destination -Destination $tempOld -Force -ErrorAction Stop
                    Copy-Item -Path $Source -Destination $Destination -Force -ErrorAction Stop
                    if ($srcTimeUtc -and (Test-Path $Destination)) {
                        (Get-Item $Destination).LastWriteTimeUtc = $srcTimeUtc
                    }
                    return $true
                } catch {
                    Write-Warning "Could not replace ${Destination}: $_"
                    return $false
                }
            }
        }
    }
    return $false
}

function Get-CodexStoreResourceDirectories {
    $dirs = New-Object System.Collections.Generic.List[string]
    $appModelKey = "HKCU:\Software\Classes\Local Settings\Software\Microsoft\Windows\CurrentVersion\AppModel\Repository\Packages"
    if (Test-Path $appModelKey) {
        Get-ChildItem -Path $appModelKey -ErrorAction SilentlyContinue |
            Where-Object { $_.PSChildName -like "OpenAI.Codex*" -or $_.PSChildName -like "OpenAI.CodexPrimaryRuntime*" } |
            Sort-Object {
                if ($_.PSChildName -match '_(\d+\.\d+\.\d+\.\d+)_') {
                    try { [version]$Matches[1] } catch { [version]"0.0.0.0" }
                } else {
                    [version]"0.0.0.0"
                }
            } -Descending |
            ForEach-Object {
                $rootFolder = (Get-ItemProperty -Path $_.PSPath -Name "PackageRootFolder" -ErrorAction SilentlyContinue).PackageRootFolder
                if ($rootFolder -and (Test-Path $rootFolder)) {
                    $appRes = Join-Path $rootFolder "app\resources"
                    if ((Test-Path $appRes) -and -not $dirs.Contains($appRes)) { $dirs.Add($appRes) }
                    if (-not $dirs.Contains($rootFolder)) { $dirs.Add($rootFolder) }
                }
            }
    }
    $storePkgs = Get-AppxPackage -Name "*OpenAI.Codex*" -ErrorAction SilentlyContinue |
        Sort-Object { try { [version]$_.Version } catch { [version]"0.0.0.0" } } -Descending
    foreach ($pkg in $storePkgs) {
        if ($pkg.InstallLocation -and (Test-Path $pkg.InstallLocation)) {
            $appRes = Join-Path $pkg.InstallLocation "app\resources"
            if ((Test-Path $appRes) -and -not $dirs.Contains($appRes)) { $dirs.Add($appRes) }
            if (-not $dirs.Contains($pkg.InstallLocation)) { $dirs.Add($pkg.InstallLocation) }
        }
    }
    $winApps = Get-ChildItem "C:\Program Files\WindowsApps\OpenAI.Codex*" -Directory -ErrorAction SilentlyContinue |
        Sort-Object {
            if ($_.Name -match '_(\d+\.\d+\.\d+\.\d+)_') {
                try { [version]$Matches[1] } catch { [version]"0.0.0.0" }
            } else {
                [version]"0.0.0.0"
            }
        }, LastWriteTime -Descending
    foreach ($wa in $winApps) {
        $appRes = Join-Path $wa.FullName "app\resources"
        if ((Test-Path $appRes) -and -not $dirs.Contains($appRes)) { $dirs.Add($appRes) }
        if (-not $dirs.Contains($wa.FullName)) { $dirs.Add($wa.FullName) }
    }
    return $dirs
}

# Locate Microsoft Store package resources for stock codex.exe and companion helper binaries
$storeCandidateDirs = Get-CodexStoreResourceDirectories
$storeResDir = $storeCandidateDirs | Where-Object { Test-Path (Join-Path $_ "codex.exe") } | Select-Object -First 1

# IMPORTANT: Do NOT include rg.exe in $binHelpers for OpenAI\Codex\bin\<hash>!
# Electron's ripgrep relocation (an -> Tn) deletes any 16-hex-char directory under OpenAI\Codex\bin
# whose name != rg's hash if that directory contains rg.exe.
$binHelpers = @(
    "codex-code-mode-host.exe",
    "codex-command-runner.exe",
    "codex-windows-sandbox-service.exe",
    "codex-windows-sandbox-setup.exe"
)
$customHelpers = @(
    "codex-code-mode-host.exe",
    "codex-command-runner.exe",
    "codex-windows-sandbox-service.exe",
    "codex-windows-sandbox-setup.exe",
    "rg.exe"
)

# 7. Ensure Desktop App Binaries in bin\<hash> Remain Stock (Electron verifies size & SHA-256; CODEX_CLI_PATH handles routing)
$localAppData = $env:LOCALAPPDATA
$desktopBinRoot = Join-Path $localAppData "OpenAI\Codex\bin"
$hookedCount = 0

if ($storeResDir) {
    $hashTargets = @(
        "codex.exe",
        "codex-code-mode-host.exe",
        "codex-windows-sandbox-setup.exe",
        "codex-command-runner.exe"
    )
    $allPresent = $true
    foreach ($ht in $hashTargets) {
        if (-not (Test-Path (Join-Path $storeResDir $ht))) {
            $allPresent = $false
            break
        }
    }
    if ($allPresent) {
        $sha256 = [System.Security.Cryptography.SHA256]::Create()
        $combinedStream = New-Object System.IO.MemoryStream
        foreach ($ht in $hashTargets) {
            $fileBytes = [System.IO.File]::ReadAllBytes((Join-Path $storeResDir $ht))
            $fileDigest = ($sha256.ComputeHash($fileBytes) | ForEach-Object { $_.ToString("x2") }) -join ""
            $nameBytes = [System.Text.Encoding]::UTF8.GetBytes("$ht`0$fileDigest`0")
            $combinedStream.Write($nameBytes, 0, $nameBytes.Length)
        }
        $combinedHash = (($sha256.ComputeHash($combinedStream.ToArray()) | ForEach-Object { $_.ToString("x2") }) -join "").Substring(0, 16)
        $combinedStream.Dispose()
        $sha256.Dispose()

        $defaultBinHashDir = Join-Path $desktopBinRoot $combinedHash
        if (-not (Test-Path $defaultBinHashDir)) {
            New-Item -ItemType Directory -Path $defaultBinHashDir -Force | Out-Null
        }
        $storeCodex = Join-Path $storeResDir "codex.exe"
        $defaultBinCodex = Join-Path $defaultBinHashDir "codex.exe"
        if ((Test-Path $storeCodex) -and (-not (Test-Path $defaultBinCodex) -or (Get-Item $defaultBinCodex).Length -lt 10000000)) {
            Safe-CopyExecutable -Source $storeCodex -Destination $defaultBinCodex | Out-Null
            Write-Host "[OK] Restored stock Microsoft Store binary to $defaultBinCodex" -ForegroundColor Green
        }
    }
}

if (Test-Path $desktopBinRoot) {
    Get-ChildItem -Path $desktopBinRoot -Directory | ForEach-Object {
        $targetDir = $_.FullName
        $codexExe = Join-Path $targetDir "codex.exe"
        $codexOrig = Join-Path $targetDir "codex.orig.exe"

        if ((Test-Path $codexExe) -or (Test-Path $codexOrig)) {
            if ((Test-Path $codexExe) -and ((Get-Item $codexExe).Length -lt 10000000)) {
                if ((Test-Path $codexOrig) -and ((Get-Item $codexOrig).Length -gt 10000000)) {
                    Safe-CopyExecutable -Source $codexOrig -Destination $codexExe | Out-Null
                    Write-Host "[OK] Restored stock binary at $codexExe from $codexOrig" -ForegroundColor Green
                } elseif ($storeResDir -and (Test-Path (Join-Path $storeResDir "codex.exe"))) {
                    Safe-CopyExecutable -Source (Join-Path $storeResDir "codex.exe") -Destination $codexExe | Out-Null
                    Write-Host "[OK] Restored stock binary at $codexExe from Microsoft Store resources" -ForegroundColor Green
                }
            } elseif (-not (Test-Path $codexExe) -and (Test-Path $codexOrig) -and ((Get-Item $codexOrig).Length -gt 10000000)) {
                Safe-CopyExecutable -Source $codexOrig -Destination $codexExe | Out-Null
                Write-Host "[OK] Restored stock binary at $codexExe from $codexOrig" -ForegroundColor Green
            } elseif (-not (Test-Path $codexExe) -and $storeResDir -and (Test-Path (Join-Path $storeResDir "codex.exe"))) {
                Safe-CopyExecutable -Source (Join-Path $storeResDir "codex.exe") -Destination $codexExe | Out-Null
                Write-Host "[OK] Restored stock binary at $codexExe from Microsoft Store resources" -ForegroundColor Green
            }

            if ((Test-Path $codexExe) -and ((Get-Item $codexExe).Length -gt 10000000) -and (Test-Path $codexOrig)) {
                Remove-Item -Path $codexOrig -Force -ErrorAction SilentlyContinue
            }

            # Remove any accidental rg.exe in a codex.exe hash directory so Electron's Tn() never purges it
            $strayRg = Join-Path $targetDir "rg.exe"
            if (Test-Path $strayRg) {
                Remove-Item -Path $strayRg -Force -ErrorAction SilentlyContinue
            }
            if ($storeResDir) {
                foreach ($helper in $binHelpers) {
                    $hSrc = Join-Path $storeResDir $helper
                    $hDst = Join-Path $targetDir $helper
                    if ((Test-Path $hSrc) -and -not (Test-Path $hDst)) {
                        Safe-CopyExecutable -Source $hSrc -Destination $hDst | Out-Null
                    }
                }
            }
        }
    }
}

# 8. Hook Daemon Binaries, Standalone CLI (Programs\OpenAI\Codex\bin), and VS Code / Cursor / Windsurf Extensions
$daemonReleases = Join-Path $codexDir "packages\app-server-daemon\releases"
if (Test-Path $daemonReleases) {
    Get-ChildItem -Path $daemonReleases -Directory | ForEach-Object {
        $dBin = Join-Path $_.FullName "bin"
        $codexExe = Join-Path $dBin "codex.exe"
        $codexOrig = Join-Path $dBin "codex.orig.exe"

        if ((Test-Path $codexExe) -or (Test-Path $codexOrig)) {
            if (Test-Path $codexExe) {
                $len = (Get-Item $codexExe).Length
                if ($len -gt 10000000) {
                    Safe-CopyExecutable -Source $codexExe -Destination $codexOrig | Out-Null
                    Write-Host "[OK] Backed up daemon binary to $codexOrig" -ForegroundColor Green
                }
            }
            if (Safe-CopyExecutable -Source $releaseBinary -Destination $codexExe) {
                Write-Host "[OK] Installed proxy hook to $codexExe" -ForegroundColor Green
                $hookedCount++
            }
        }
    }
}

$extraHookDirs = New-Object System.Collections.Generic.List[string]
$standaloneCliBin = Join-Path $localAppData "Programs\OpenAI\Codex\bin"
if (Test-Path $standaloneCliBin) {
    $extraHookDirs.Add($standaloneCliBin)
}
foreach ($ideRoot in @(
    (Join-Path $env:USERPROFILE ".vscode\extensions"),
    (Join-Path $env:USERPROFILE ".vscode-insiders\extensions"),
    (Join-Path $env:USERPROFILE ".cursor\extensions"),
    (Join-Path $env:USERPROFILE ".windsurf\extensions"),
    (Join-Path $env:USERPROFILE ".antigravity\extensions"),
    (Join-Path $env:USERPROFILE ".antigravity-ide\extensions")
)) {
    if (Test-Path $ideRoot) {
        Get-ChildItem -Path $ideRoot -Directory -Filter "openai.chatgpt-*" -ErrorAction SilentlyContinue | ForEach-Object {
            foreach ($archSub in @("bin\windows-x86_64", "bin\windows-aarch64", "bin\windows-arm64")) {
                $extBin = Join-Path $_.FullName $archSub
                if (Test-Path $extBin) { $extraHookDirs.Add($extBin) }
            }
        }
    }
}

foreach ($hDir in $extraHookDirs) {
    $cExe = Join-Path $hDir "codex.exe"
    $cOrig = Join-Path $hDir "codex.orig.exe"
    if ((Test-Path $cExe) -or (Test-Path $cOrig)) {
        if (Test-Path $cExe) {
            $cItem = Get-Item $cExe
            if ($cItem.Length -gt 10000000) {
                $needsBackup = (-not (Test-Path $cOrig))
                if (-not $needsBackup) {
                    $oItem = Get-Item $cOrig
                    if (($oItem.Length -ne $cItem.Length) -or ($oItem.LastWriteTimeUtc -ne $cItem.LastWriteTimeUtc)) {
                        $needsBackup = $true
                    }
                }
                if ($needsBackup) {
                    Safe-CopyExecutable -Source $cExe -Destination $cOrig | Out-Null
                    Write-Host "[OK] Backed up official binary to $cOrig" -ForegroundColor Green
                }
            }
        }
        if (Safe-CopyExecutable -Source $releaseBinary -Destination $cExe) {
            Write-Host "[OK] Installed proxy hook to $cExe" -ForegroundColor Green
            $hookedCount++
        }
    }
}

# 9. Deploy Custom Standalone Copy & Synchronize Store / Local Stock Binaries
$customDir = Join-Path $localAppData "OpenAI\Codex\custom"
if (-not (Test-Path $customDir)) {
    New-Item -ItemType Directory -Path $customDir -Force | Out-Null
}
$customShim = Join-Path $customDir "codex-9router-subagents.exe"
if (Safe-CopyExecutable -Source $releaseBinary -Destination $customShim) {
    Write-Host "[OK] Deployed standalone shim to $customShim" -ForegroundColor Green
}
$customCodex = Join-Path $customDir "codex.exe"
if (Safe-CopyExecutable -Source $releaseBinary -Destination $customCodex) {
    Write-Host "[OK] Deployed standalone proxy copy to $customCodex" -ForegroundColor Green
}

# Locate newest stock codex.exe / codex.orig.exe (> 10 MB) across Microsoft Store, desktop bin, custom/*.orig.exe, daemon releases, Programs, and IDE extensions
$stockCandidates = @()
if ($storeResDir -and (Test-Path (Join-Path $storeResDir "codex.exe"))) {
    $stockCandidates += Get-Item (Join-Path $storeResDir "codex.exe") -ErrorAction SilentlyContinue
}
if (Test-Path $desktopBinRoot) {
    $stockCandidates += Get-ChildItem -Path "$desktopBinRoot\*\codex.orig.exe", "$desktopBinRoot\*\codex.exe" -File -ErrorAction SilentlyContinue
}
foreach ($ehDir in $extraHookDirs) {
    $stockCandidates += Get-ChildItem -Path "$ehDir\codex.orig.exe", "$ehDir\codex.exe" -File -ErrorAction SilentlyContinue
}
if (Test-Path $customDir) {
    $stockCandidates += Get-ChildItem -Path "$customDir\codex.orig.exe", "$customDir\codex-9router-subagents.orig.exe" -File -ErrorAction SilentlyContinue
}
if (Test-Path $daemonReleases) {
    $stockCandidates += Get-ChildItem -Path "$daemonReleases\*\bin\codex.orig.exe", "$daemonReleases\*\bin\codex.exe" -File -ErrorAction SilentlyContinue
}
$bestStockItem = $stockCandidates | Where-Object { $_.Length -gt 10000000 -and (Test-StockCodexHealthy $_.FullName) } | Sort-Object LastWriteTimeUtc, Length -Descending | Select-Object -First 1
if ($bestStockItem) {
    $stockDir = $bestStockItem.DirectoryName
    foreach ($origName in @("codex-9router-subagents.orig.exe", "codex.orig.exe")) {
        $origDst = Join-Path $customDir $origName
        $needsSync = (-not (Test-Path $origDst))
        if (-not $needsSync) {
            $dstItem = Get-Item $origDst
            if (($dstItem.Length -lt 10000000) -or ($dstItem.LastWriteTimeUtc -lt $bestStockItem.LastWriteTimeUtc) -or (($dstItem.LastWriteTimeUtc -eq $bestStockItem.LastWriteTimeUtc) -and ($dstItem.Length -ne $bestStockItem.Length))) {
                $needsSync = $true
            }
        }
        if (($bestStockItem.FullName -ne $origDst) -and $needsSync) {
            if (Safe-CopyExecutable -Source $bestStockItem.FullName -Destination $origDst) {
                Write-Host "[OK] Synchronized $origName from newest stock binary ($($bestStockItem.Length) bytes)." -ForegroundColor Green
            }
        }
    }
    $helperSearchDirs = New-Object System.Collections.Generic.List[string]
    if ($stockDir -and (Test-Path $stockDir)) { $helperSearchDirs.Add($stockDir) }
    foreach ($scDir in $storeCandidateDirs) {
        if ($scDir -and (Test-Path $scDir) -and -not $helperSearchDirs.Contains($scDir)) { $helperSearchDirs.Add($scDir) }
    }
    foreach ($ehDir in $extraHookDirs) {
        if ($ehDir -and (Test-Path $ehDir) -and -not $helperSearchDirs.Contains($ehDir)) { $helperSearchDirs.Add($ehDir) }
    }
    foreach ($helper in $customHelpers) {
        $bestHelper = $helperSearchDirs | ForEach-Object { Join-Path $_ $helper } | Where-Object { Test-Path $_ } | ForEach-Object { Get-Item $_ -ErrorAction SilentlyContinue } | Where-Object { $_.Length -gt 0 } | Sort-Object LastWriteTimeUtc, Length -Descending | Select-Object -First 1
        $hDst = Join-Path $customDir $helper
        if ($bestHelper -and ($bestHelper.FullName -ne $hDst)) {
            $needsHelperSync = (-not (Test-Path $hDst))
            if (-not $needsHelperSync) {
                $dstItem = Get-Item $hDst
                if (($dstItem.Length -eq 0) -or ($dstItem.LastWriteTimeUtc -lt $bestHelper.LastWriteTimeUtc) -or (($dstItem.LastWriteTimeUtc -eq $bestHelper.LastWriteTimeUtc) -and ($dstItem.Length -ne $bestHelper.Length))) {
                    $needsHelperSync = $true
                }
            }
            if ($needsHelperSync -and (Safe-CopyExecutable -Source $bestHelper.FullName -Destination $hDst)) {
                Write-Host "[OK] Synchronized helper $helper to $customDir ($($bestHelper.Length) bytes)." -ForegroundColor Green
            }
        }
    }
}

# Propagate freshest custom\codex.orig.exe and companion helpers to hooked CLI / IDE extension / daemon directories
$customOrigStock = Join-Path $customDir "codex.orig.exe"
if ((Test-Path $customOrigStock) -and ((Get-Item $customOrigStock).Length -gt 10000000)) {
    $customOrigItem = Get-Item $customOrigStock
    $hookTargetDirs = New-Object System.Collections.Generic.List[string]
    foreach ($ehDir in $extraHookDirs) {
        if (-not $hookTargetDirs.Contains($ehDir)) { $hookTargetDirs.Add($ehDir) }
    }
    if (Test-Path $daemonReleases) {
        Get-ChildItem -Path $daemonReleases -Directory -ErrorAction SilentlyContinue | ForEach-Object {
            $dBin = Join-Path $_.FullName "bin"
            if ((Test-Path $dBin) -and -not $hookTargetDirs.Contains($dBin)) { $hookTargetDirs.Add($dBin) }
        }
    }
    foreach ($hDir in $hookTargetDirs) {
        $cExe = Join-Path $hDir "codex.exe"
        $cOrig = Join-Path $hDir "codex.orig.exe"
        if ((Test-Path $cExe) -or (Test-Path $cOrig)) {
            $needsOrigSync = (-not (Test-Path $cOrig))
            if (-not $needsOrigSync) {
                $oItem = Get-Item $cOrig
                if (($oItem.Length -lt 10000000) -or ($oItem.LastWriteTimeUtc -lt $customOrigItem.LastWriteTimeUtc) -or (($oItem.LastWriteTimeUtc -eq $customOrigItem.LastWriteTimeUtc) -and ($oItem.Length -ne $customOrigItem.Length))) {
                    $needsOrigSync = $true
                }
            }
            if ($needsOrigSync -and ($customOrigItem.FullName -ne $cOrig)) {
                if (Safe-CopyExecutable -Source $customOrigStock -Destination $cOrig) {
                    Write-Host "[OK] Refreshed hooked engine $cOrig from $customOrigStock ($($customOrigItem.Length) bytes)." -ForegroundColor Green
                }
            }
            foreach ($helper in $binHelpers) {
                $hSrc = Join-Path $customDir $helper
                $hDst = Join-Path $hDir $helper
                if ((Test-Path $hSrc) -and ($hSrc -ne $hDst)) {
                    $srcItem = Get-Item $hSrc
                    $needsHelperSync = (-not (Test-Path $hDst))
                    if (-not $needsHelperSync) {
                        $dstItem = Get-Item $hDst
                        if (($dstItem.Length -eq 0) -or ($dstItem.LastWriteTimeUtc -lt $srcItem.LastWriteTimeUtc) -or (($dstItem.LastWriteTimeUtc -eq $srcItem.LastWriteTimeUtc) -and ($dstItem.Length -ne $srcItem.Length))) {
                            $needsHelperSync = $true
                        }
                    }
                    if ($needsHelperSync) {
                        Safe-CopyExecutable -Source $hSrc -Destination $hDst | Out-Null
                    }
                }
            }
        }
    }
}

# Synchronize ~/.codex/models_cache.json in place with active parent model subagent metadata
$modelsCache = Join-Path $codexDir "models_cache.json"
if (Test-Path $modelsCache) {
    & $releaseBinary --doctor 2>&1 | Out-Null
    Write-Host "[OK] Synchronized subagent model metadata in $modelsCache." -ForegroundColor Green
}

# 10. Register Self-Healing Startup Hook
$syncScriptPath = Join-Path $customDir "hook-sync.ps1"
$syncScript = @"
`$ErrorActionPreference = 'SilentlyContinue'
`$customDir = Join-Path `$env:LOCALAPPDATA 'OpenAI\Codex\custom'
`$proxyPath = Join-Path `$customDir 'codex-9router-subagents.exe'
`$customCodex = Join-Path `$customDir 'codex.exe'
`$binRoot = Join-Path `$env:LOCALAPPDATA 'OpenAI\Codex\bin'
`$codexDir = if (`$env:CODEX_HOME -and `$env:CODEX_HOME.Trim()) { `$env:CODEX_HOME.Trim() } else { Join-Path `$env:USERPROFILE '.codex' }
`$daemonReleases = Join-Path `$codexDir 'packages\app-server-daemon\releases'
`$parsedPort = 0
`$userPortRaw = [System.Environment]::GetEnvironmentVariable('CODEX_PROXY_PORT', 'User')
`$proxyPort = if (`$env:CODEX_PROXY_PORT -and [int]::TryParse(`$env:CODEX_PROXY_PORT.Trim(), [ref]`$parsedPort) -and `$parsedPort -gt 0) {
    `$parsedPort
} elseif (`$userPortRaw -and [int]::TryParse(`$userPortRaw.Trim(), [ref]`$parsedPort) -and `$parsedPort -gt 0) {
    `$parsedPort
} else {
    $proxyPort
}

function Sync-Executable {
    param(
        [string]`$Source,
        [string]`$Destination
    )
    if (-not (Test-Path `$Source)) { return `$false }
    `$destDir = Split-Path -Parent `$Destination
    `$fileName = Split-Path -Leaf `$Destination
    `$srcTimeUtc = $null
    if (Test-Path `$Source) {
        `$srcTimeUtc = (Get-Item `$Source).LastWriteTimeUtc
    }
    if (-not (Test-Path `$Destination)) {
        Copy-Item -Path `$Source -Destination `$Destination -Force
        if (`$srcTimeUtc -and (Test-Path `$Destination)) {
            (Get-Item `$Destination).LastWriteTimeUtc = `$srcTimeUtc
        }
        return (Test-Path `$Destination)
    }
    try {
        Copy-Item -Path `$Source -Destination `$Destination -Force -ErrorAction Stop
        if (`$srcTimeUtc -and (Test-Path `$Destination)) {
            (Get-Item `$Destination).LastWriteTimeUtc = `$srcTimeUtc
        }
        return `$true
    } catch {
        `$tempOld = Join-Path `$destDir "`$fileName.old.`$([Guid]::NewGuid().ToString('N').Substring(0,6))"
        try {
            Move-Item -Path `$Destination -Destination `$tempOld -Force -ErrorAction Stop
            Copy-Item -Path `$Source -Destination `$Destination -Force -ErrorAction Stop
            if (`$srcTimeUtc -and (Test-Path `$Destination)) {
                (Get-Item `$Destination).LastWriteTimeUtc = `$srcTimeUtc
            }
            return `$true
        } catch {
            return `$false
        }
    }
}

function Test-StockCodexHealthy {
    param([string]`$ExePath)
    if (-not (Test-Path `$ExePath)) { return `$false }
    try {
        `$psi = New-Object System.Diagnostics.ProcessStartInfo
        `$psi.FileName = `$ExePath
        `$psi.Arguments = '--version'
        `$psi.UseShellExecute = `$false
        `$psi.CreateNoWindow = `$true
        `$psi.RedirectStandardOutput = `$true
        `$psi.RedirectStandardError = `$true
        `$proc = [System.Diagnostics.Process]::Start(`$psi)
        if (`$proc.WaitForExit(3000)) {
            return (`$proc.ExitCode -eq 0)
        }
        try { `$proc.Kill() } catch {}
        return `$false
    } catch {
        return `$false
    }
}

function Get-PackageSortVersion {
    param([string]`$Text)
    if (`$Text -match '_(\d+\.\d+\.\d+\.\d+)_') {
        `$v = `$null
        if ([version]::TryParse(`$Matches[1], [ref]`$v)) { return `$v }
    }
    `$v2 = `$null
    if ([version]::TryParse(`$Text, [ref]`$v2)) { return `$v2 }
    return [version]'0.0.0.0'
}

# 1. Ensure CODEX_CLI_PATH User environment override and User/Process PATH index-0 priority point to our custom shim
if (Test-Path `$proxyPath) {
    `$curCli = [System.Environment]::GetEnvironmentVariable('CODEX_CLI_PATH', 'User')
    if (`$curCli -ne `$proxyPath) {
        [System.Environment]::SetEnvironmentVariable('CODEX_CLI_PATH', `$proxyPath, 'User')
    }
    `$env:CODEX_CLI_PATH = `$proxyPath
    if ((-not (Test-Path `$customCodex)) -or ((Get-Item `$customCodex).Length -ne (Get-Item `$proxyPath).Length) -or ((Get-FileHash `$customCodex -Algorithm SHA256).Hash -ne (Get-FileHash `$proxyPath -Algorithm SHA256).Hash)) {
        Sync-Executable -Source `$proxyPath -Destination `$customCodex | Out-Null
    }
    `$normCustom = `$customDir.TrimEnd('\')
    `$uPath = [System.Environment]::GetEnvironmentVariable('Path', 'User')
    `$uEntries = if (`$uPath) { @(`$uPath -split ';' | Where-Object { `$_.Trim() }) } else { @() }
    `$rest = @(`$uEntries | Where-Object { `$_.Trim().TrimEnd('\') -ine `$normCustom })
    `$desiredUserPath = (@(`$customDir) + `$rest) -join ';'
    if (`$uPath -ne `$desiredUserPath) {
        [System.Environment]::SetEnvironmentVariable('Path', `$desiredUserPath, 'User')
    }
    `$pEntries = if (`$env:Path) { @(`$env:Path -split ';' | Where-Object { `$_.Trim() -and (`$_.Trim().TrimEnd('\') -ine `$normCustom) }) } else { @() }
    `$env:Path = (@(`$customDir) + `$pEntries) -join ';'
}

# 2. Locate latest Microsoft Store OpenAI.Codex & OpenAI.CodexPrimaryRuntime resource directories (sorted by parsed [version] descending)
`$candDirs = New-Object System.Collections.Generic.List[string]
`$appModelKey = 'HKCU:\Software\Classes\Local Settings\Software\Microsoft\Windows\CurrentVersion\AppModel\Repository\Packages'
if (Test-Path `$appModelKey) {
    Get-ChildItem -Path `$appModelKey | Where-Object { `$_.PSChildName -like 'OpenAI.Codex*' } | Sort-Object { Get-PackageSortVersion `$_.PSChildName } -Descending | ForEach-Object {
        `$rf = (Get-ItemProperty -Path `$_.PSPath -Name 'PackageRootFolder').PackageRootFolder
        if (`$rf -and (Test-Path `$rf)) {
            `$ar = Join-Path `$rf 'app\resources'
            if ((Test-Path `$ar) -and -not `$candDirs.Contains(`$ar)) { `$candDirs.Add(`$ar) }
            if (-not `$candDirs.Contains(`$rf)) { `$candDirs.Add(`$rf) }
        }
    }
}
Get-AppxPackage -Name '*OpenAI.Codex*' | Sort-Object { Get-PackageSortVersion `$_.Version } -Descending | ForEach-Object {
    if (`$_.InstallLocation -and (Test-Path `$_.InstallLocation)) {
        `$ar = Join-Path `$_.InstallLocation 'app\resources'
        if ((Test-Path `$ar) -and -not `$candDirs.Contains(`$ar)) { `$candDirs.Add(`$ar) }
        if (-not `$candDirs.Contains(`$_.InstallLocation)) { `$candDirs.Add(`$_.InstallLocation) }
    }
}
`$winApps = 'C:\Program Files\WindowsApps'
if ((`$candDirs.Count -eq 0) -and (Test-Path `$winApps)) {
    Get-ChildItem -Path `$winApps -Directory -Filter 'OpenAI.Codex*' | Sort-Object { Get-PackageSortVersion `$_.Name } -Descending | ForEach-Object {
        `$ar = Join-Path `$_.FullName 'app\resources'
        if ((Test-Path `$ar) -and -not `$candDirs.Contains(`$ar)) { `$candDirs.Add(`$ar) }
        if (-not `$candDirs.Contains(`$_.FullName)) { `$candDirs.Add(`$_.FullName) }
    }
}
`$resDir = `$candDirs | Where-Object { Test-Path (Join-Path `$_ 'codex.exe') } | Select-Object -First 1

`$binHelpers = @('codex-code-mode-host.exe', 'codex-command-runner.exe', 'codex-windows-sandbox-service.exe', 'codex-windows-sandbox-setup.exe')
`$customHelpers = @('codex-code-mode-host.exe', 'codex-command-runner.exe', 'codex-windows-sandbox-service.exe', 'codex-windows-sandbox-setup.exe', 'rg.exe')

# 3. Hook Standalone CLI (Programs\OpenAI\Codex\bin) and VS Code / Cursor / Windsurf / Antigravity Extensions on update
`$extraDirs = New-Object System.Collections.Generic.List[string]
`$progBin = Join-Path `$env:LOCALAPPDATA 'Programs\OpenAI\Codex\bin'
if (Test-Path `$progBin) { `$extraDirs.Add(`$progBin) }
foreach (`$ideRoot in @(
    (Join-Path `$env:USERPROFILE '.vscode\extensions'),
    (Join-Path `$env:USERPROFILE '.vscode-insiders\extensions'),
    (Join-Path `$env:USERPROFILE '.cursor\extensions'),
    (Join-Path `$env:USERPROFILE '.windsurf\extensions'),
    (Join-Path `$env:USERPROFILE '.antigravity\extensions'),
    (Join-Path `$env:USERPROFILE '.antigravity-ide\extensions')
)) {
    if (Test-Path `$ideRoot) {
        Get-ChildItem -Path `$ideRoot -Directory -Filter 'openai.chatgpt-*' | ForEach-Object {
            foreach (`$archSub in @('bin\windows-x86_64', 'bin\windows-aarch64', 'bin\windows-arm64')) {
                `$eb = Join-Path `$_.FullName `$archSub
                if (Test-Path `$eb) { `$extraDirs.Add(`$eb) }
            }
        }
    }
}
if (Test-Path `$proxyPath) {
    `$proxyItem = Get-Item `$proxyPath
    foreach (`$ed in `$extraDirs) {
        `$ce = Join-Path `$ed 'codex.exe'
        `$co = Join-Path `$ed 'codex.orig.exe'
        if (Test-Path `$ce) {
            `$ci = Get-Item `$ce
            if (`$ci.Length -gt 10000000) {
                if ((-not (Test-Path `$co)) -or ((Get-Item `$co).Length -ne `$ci.Length) -or ((Get-Item `$co).LastWriteTimeUtc -ne `$ci.LastWriteTimeUtc)) {
                    Sync-Executable -Source `$ce -Destination `$co | Out-Null
                }
            }
            if ((`$ci.Length -ne `$proxyItem.Length) -or (`$ci.LastWriteTimeUtc -ne `$proxyItem.LastWriteTimeUtc)) {
                Sync-Executable -Source `$proxyPath -Destination `$ce | Out-Null
            }
        }
    }
}

# 4. Refresh custom\codex.orig.exe, custom\codex-9router-subagents.orig.exe, and custom\ helpers when MS Store or CLI updates
`$allStockCandidates = @()
if (`$resDir -and (Test-Path (Join-Path `$resDir 'codex.exe'))) {
    `$allStockCandidates += Get-Item (Join-Path `$resDir 'codex.exe')
}
if (Test-Path `$binRoot) {
    `$allStockCandidates += Get-ChildItem -Path "`$binRoot\*\codex.orig.exe", "`$binRoot\*\codex.exe" -File
}
foreach (`$ed in `$extraDirs) {
    `$allStockCandidates += Get-ChildItem -Path "`$ed\codex.orig.exe", "`$ed\codex.exe" -File
}
if (Test-Path `$customDir) {
    `$allStockCandidates += Get-ChildItem -Path "`$customDir\codex.orig.exe", "`$customDir\codex-9router-subagents.orig.exe" -File
}
if (Test-Path `$daemonReleases) {
    `$allStockCandidates += Get-ChildItem -Path "`$daemonReleases\*\bin\codex.orig.exe", "`$daemonReleases\*\bin\codex.exe" -File
}
`$bestStock = `$allStockCandidates | Where-Object { `$_.Length -gt 10000000 -and (Test-StockCodexHealthy `$_.FullName) } | Sort-Object LastWriteTimeUtc, Length -Descending | Select-Object -First 1
if (`$bestStock) {
    foreach (`$origName in @('codex-9router-subagents.orig.exe', 'codex.orig.exe')) {
        `$origDst = Join-Path `$customDir `$origName
        if (`$bestStock.FullName -ne `$origDst) {
            `$needsOrig = (-not (Test-Path `$origDst))
            if (-not `$needsOrig) {
                `$di = Get-Item `$origDst
                if ((`$di.Length -lt 10000000) -or (`$di.LastWriteTimeUtc -lt `$bestStock.LastWriteTimeUtc) -or ((`$di.LastWriteTimeUtc -eq `$bestStock.LastWriteTimeUtc) -and (`$di.Length -ne `$bestStock.Length))) {
                    `$needsOrig = `$true
                }
            }
            if (`$needsOrig) {
                Sync-Executable -Source `$bestStock.FullName -Destination `$origDst | Out-Null
            }
        }
    }
    `$hSearchDirs = New-Object System.Collections.Generic.List[string]
    if (`$bestStock.DirectoryName -and (Test-Path `$bestStock.DirectoryName)) { `$hSearchDirs.Add(`$bestStock.DirectoryName) }
    foreach (`$cd in `$candDirs) {
        if (`$cd -and (Test-Path `$cd) -and -not `$hSearchDirs.Contains(`$cd)) { `$hSearchDirs.Add(`$cd) }
    }
    foreach (`$ed in `$extraDirs) {
        if (`$ed -and (Test-Path `$ed) -and -not `$hSearchDirs.Contains(`$ed)) { `$hSearchDirs.Add(`$ed) }
    }
    foreach (`$h in `$customHelpers) {
        `$bestH = `$hSearchDirs | ForEach-Object { Join-Path `$_ `$h } | Where-Object { Test-Path `$_ } | ForEach-Object { Get-Item `$_ } | Where-Object { `$_.Length -gt 0 } | Sort-Object LastWriteTimeUtc, Length -Descending | Select-Object -First 1
        `$hd = Join-Path `$customDir `$h
        if (`$bestH -and (`$bestH.FullName -ne `$hd)) {
            `$needsH = (-not (Test-Path `$hd))
            if (-not `$needsH) {
                `$hdi = Get-Item `$hd
                if ((`$hdi.Length -eq 0) -or (`$hdi.LastWriteTimeUtc -lt `$bestH.LastWriteTimeUtc) -or ((`$hdi.LastWriteTimeUtc -eq `$bestH.LastWriteTimeUtc) -and (`$hdi.Length -ne `$bestH.Length))) {
                    `$needsH = `$true
                }
            }
            if (`$needsH) {
                Sync-Executable -Source `$bestH.FullName -Destination `$hd | Out-Null
            }
        }
    }
}

# 4b. Propagate freshest custom\codex.orig.exe and companion helpers to hooked CLI / IDE extension / daemon directories
`$customOrigStock = Join-Path `$customDir 'codex.orig.exe'
if ((Test-Path `$customOrigStock) -and ((Get-Item `$customOrigStock).Length -gt 10000000)) {
    `$customOrigItem = Get-Item `$customOrigStock
    `$hookTargetDirs = New-Object System.Collections.Generic.List[string]
    foreach (`$ed in `$extraDirs) {
        if (-not `$hookTargetDirs.Contains(`$ed)) { `$hookTargetDirs.Add(`$ed) }
    }
    if (Test-Path `$daemonReleases) {
        Get-ChildItem -Path `$daemonReleases -Directory | ForEach-Object {
            `$dBin = Join-Path `$_.FullName 'bin'
            if ((Test-Path `$dBin) -and -not `$hookTargetDirs.Contains(`$dBin)) { `$hookTargetDirs.Add(`$dBin) }
        }
    }
    foreach (`$hDir in `$hookTargetDirs) {
        `$cExe = Join-Path `$hDir 'codex.exe'
        `$cOrig = Join-Path `$hDir 'codex.orig.exe'
        if ((Test-Path `$cExe) -or (Test-Path `$cOrig)) {
            if (`$customOrigItem.FullName -ne `$cOrig) {
                `$needsCO = (-not (Test-Path `$cOrig))
                if (-not `$needsCO) {
                    `$coi = Get-Item `$cOrig
                    if ((`$coi.Length -lt 10000000) -or (`$coi.LastWriteTimeUtc -lt `$customOrigItem.LastWriteTimeUtc) -or ((`$coi.LastWriteTimeUtc -eq `$customOrigItem.LastWriteTimeUtc) -and (`$coi.Length -ne `$customOrigItem.Length))) {
                        `$needsCO = `$true
                    }
                }
                if (`$needsCO) {
                    Sync-Executable -Source `$customOrigStock -Destination `$cOrig | Out-Null
                }
            }
            foreach (`$h in `$binHelpers) {
                `$hs = Join-Path `$customDir `$h
                `$hd = Join-Path `$hDir `$h
                if ((Test-Path `$hs) -and (`$hs -ne `$hd)) {
                    `$hsItem = Get-Item `$hs
                    `$needsBH = (-not (Test-Path `$hd))
                    if (-not `$needsBH) {
                        `$hdi = Get-Item `$hd
                        if ((`$hdi.Length -eq 0) -or (`$hdi.LastWriteTimeUtc -lt `$hsItem.LastWriteTimeUtc) -or ((`$hdi.LastWriteTimeUtc -eq `$hsItem.LastWriteTimeUtc) -and (`$hdi.Length -ne `$hsItem.Length))) {
                            `$needsBH = `$true
                        }
                    }
                    if (`$needsBH) {
                        Sync-Executable -Source `$hs -Destination `$hd | Out-Null
                    }
                }
            }
        }
    }
}

# 5. Keep bin\<hash>\codex.exe as the unmodified stock binary so Electron never deletes bin\<hash>
if (Test-Path `$binRoot) {
    Get-ChildItem -Path `$binRoot -Directory | ForEach-Object {
        `$c = Join-Path `$_.FullName 'codex.exe'
        `$o = Join-Path `$_.FullName 'codex.orig.exe'
        if ((Test-Path `$c) -or (Test-Path `$o)) {
            if ((Test-Path `$c) -and ((Get-Item `$c).Length -lt 10000000)) {
                if ((Test-Path `$o) -and ((Get-Item `$o).Length -gt 10000000)) {
                    Sync-Executable -Source `$o -Destination `$c | Out-Null
                } elseif (`$resDir -and (Test-Path (Join-Path `$resDir 'codex.exe'))) {
                    Sync-Executable -Source (Join-Path `$resDir 'codex.exe') -Destination `$c | Out-Null
                }
            } elseif ((-not (Test-Path `$c)) -and (Test-Path `$o) -and ((Get-Item `$o).Length -gt 10000000)) {
                Sync-Executable -Source `$o -Destination `$c | Out-Null
            } elseif ((-not (Test-Path `$c)) -and `$resDir -and (Test-Path (Join-Path `$resDir 'codex.exe'))) {
                Sync-Executable -Source (Join-Path `$resDir 'codex.exe') -Destination `$c | Out-Null
            }
            if ((Test-Path `$c) -and ((Get-Item `$c).Length -gt 10000000) -and (Test-Path `$o)) {
                Remove-Item `$o -Force
            }
            `$strayRg = Join-Path `$_.FullName 'rg.exe'
            if (Test-Path `$strayRg) {
                Remove-Item `$strayRg -Force
            }
            if (`$resDir -and (Test-Path `$resDir)) {
                foreach (`$h in `$binHelpers) {
                    `$hs = `$candDirs | ForEach-Object { Join-Path `$_ `$h } | Where-Object { Test-Path `$_ } | Select-Object -First 1
                    `$hd = Join-Path `$_.FullName `$h
                    if (`$hs -and (Test-Path `$hs) -and -not (Test-Path `$hd)) {
                        Sync-Executable -Source `$hs -Destination `$hd | Out-Null
                    }
                }
            }
        }
    }
}

# 6. Terminate any stale .old.* processes (via Win32 QueryFullProcessImageNameW), hidden-desktop ChatGPT.exe instances, and clean up leftover .old.* / lockfile files
if (-not ('CodexProcessNative' -as [type])) {
    `$csProc = 'using System; using System.Text; using System.Runtime.InteropServices; public class CodexProcessNative { [DllImport("kernel32.dll", SetLastError = true)] public static extern IntPtr OpenProcess(uint dwDesiredAccess, bool bInheritHandle, int dwProcessId); [DllImport("kernel32.dll", SetLastError = true)] public static extern bool CloseHandle(IntPtr hObject); [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)] public static extern bool QueryFullProcessImageNameW(IntPtr hProcess, uint dwFlags, StringBuilder lpExeName, ref uint lpdwSize); public static string GetProcessImagePath(int pid) { if (pid <= 0) return null; IntPtr hProc = OpenProcess(0x1000, false, pid); if (hProc == IntPtr.Zero) return null; try { StringBuilder sb = new StringBuilder(2048); uint size = (uint)sb.Capacity; if (QueryFullProcessImageNameW(hProc, 0, sb, ref size) && size > 0) return sb.ToString(); return null; } finally { CloseHandle(hProc); } } public static string GetLiveImagePath(int pid) { return GetProcessImagePath(pid); } }'
    Add-Type -TypeDefinition `$csProc
}
function Get-LiveProcessImagePath {
    param(`$Process)
    if (-not `$Process) { return `$null }
    try {
        if ('CodexProcessNative' -as [type]) {
            `$lp = [CodexProcessNative]::GetProcessImagePath([int]`$Process.Id)
            if (-not [string]::IsNullOrWhiteSpace(`$lp)) { return `$lp }
        }
    } catch {}
    try { return `$Process.Path } catch { return `$null }
}
function Get-AncestorProcessIds {
    `$ancestors = New-Object 'System.Collections.Generic.HashSet[int]'
    `$curPid = `$PID
    for (`$depth = 0; `$depth -lt 32 -and `$curPid -gt 0; `$depth++) {
        [void]`$ancestors.Add([int]`$curPid)
        try {
            `$procInfo = Get-CimInstance Win32_Process -Filter "ProcessId = `$curPid"
            if (`$procInfo -and `$procInfo.ParentProcessId -gt 0 -and -not `$ancestors.Contains([int]`$procInfo.ParentProcessId)) {
                `$curPid = [int]`$procInfo.ParentProcessId
            } else { break }
        } catch { break }
    }
    return `$ancestors
}
`$ancestorPids = Get-AncestorProcessIds
`$staleOldProcs = @(Get-Process | Where-Object {
    (-not `$ancestorPids.Contains([int]`$_.Id)) -and (
        `$_.Name -like 'codex*.old*' -or
        (`$_.Name -like 'codex*' -and ((Get-LiveProcessImagePath `$_) -like '*.old.*'))
    )
})
if (`$staleOldProcs.Count -gt 0) {
    `$staleOldProcs | Stop-Process -Force
    Start-Sleep -Milliseconds 500
}
Get-ChildItem "`$binRoot\*\*.old.*", "`$customDir\*.old.*" | Remove-Item -Force
foreach (`$ed in `$extraDirs) {
    if (Test-Path `$ed) {
        Get-ChildItem (Join-Path `$ed '*.old.*') | Remove-Item -Force
    }
}

`$chatGptPids = [int[]]@(Get-Process -Name 'ChatGPT*' | Select-Object -ExpandProperty Id)
if (`$chatGptPids.Count -gt 0) {
    if (-not ('CodexDesktopCheck' -as [type])) {
        `$cs = 'using System; using System.Text; using System.Runtime.InteropServices; using System.Collections.Generic; public class CodexDesktopCheck { [UnmanagedFunctionPointer(CallingConvention.StdCall, CharSet = CharSet.Unicode)] public delegate bool EnumDesktopsDelegate([MarshalAs(UnmanagedType.LPWStr)] string desktop, IntPtr lParam); public delegate bool EnumWindowsProc(IntPtr hWnd, IntPtr lParam); [DllImport("user32.dll")] public static extern IntPtr GetProcessWindowStation(); [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern bool EnumDesktopsW(IntPtr hwinsta, EnumDesktopsDelegate lpEnumFunc, IntPtr lParam); [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern IntPtr OpenDesktopW(string lpszDesktop, uint dwFlags, bool fInherit, uint dwDesiredAccess); [DllImport("user32.dll")] public static extern bool CloseDesktop(IntPtr hDesktop); [DllImport("user32.dll")] public static extern bool EnumDesktopWindows(IntPtr hDesktop, EnumWindowsProc lpfn, IntPtr lParam); [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr hWnd, out uint lpdwProcessId); [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetClassNameW(IntPtr hWnd, StringBuilder lpClassName, int nMaxCount); [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr hWnd); public static void ClassifyChatGptPids(int[] targetPids, out int[] defaultVisiblePids, out int[] defaultAnyPids, out int[] hiddenDesktopPids) { HashSet<int> targetSet = new HashSet<int>(targetPids ?? new int[0]); HashSet<int> defVis = new HashSet<int>(); HashSet<int> defAny = new HashSet<int>(); HashSet<int> hidden = new HashSet<int>(); List<string> desktops = new List<string>(); EnumDesktopsDelegate dCb = (d, l) => { desktops.Add(d); return true; }; EnumDesktopsW(GetProcessWindowStation(), dCb, IntPtr.Zero); foreach (string dName in desktops) { IntPtr hDesk = OpenDesktopW(dName, 0, false, 0x01FF); if (hDesk == IntPtr.Zero) continue; bool isDefault = string.Equals(dName, "Default", StringComparison.OrdinalIgnoreCase); bool isChromiumSbox = dName.StartsWith("sbox_alternate_desktop", StringComparison.OrdinalIgnoreCase); EnumWindowsProc wCb = (hWnd, l) => { uint wpid = 0; GetWindowThreadProcessId(hWnd, out wpid); if (targetSet.Contains((int)wpid)) { StringBuilder cls = new StringBuilder(256); GetClassNameW(hWnd, cls, 256); string clsName = cls.ToString(); if (clsName.StartsWith("Chrome_WidgetWin_", StringComparison.Ordinal)) { if (isDefault) { defAny.Add((int)wpid); if (clsName == "Chrome_WidgetWin_1" && IsWindowVisible(hWnd)) { defVis.Add((int)wpid); } } else if (!isChromiumSbox) { hidden.Add((int)wpid); } } } return true; }; EnumDesktopWindows(hDesk, wCb, IntPtr.Zero); CloseDesktop(hDesk); } hidden.ExceptWith(defAny); defaultVisiblePids = new List<int>(defVis).ToArray(); defaultAnyPids = new List<int>(defAny).ToArray(); hiddenDesktopPids = new List<int>(hidden).ToArray(); } }'
        Add-Type -TypeDefinition `$cs
    }
    [int[]]`$defaultVisiblePids = @()
    [int[]]`$defaultAnyPids = @()
    [int[]]`$hiddenDesktopPids = @()
    [CodexDesktopCheck]::ClassifyChatGptPids(`$chatGptPids, [ref]`$defaultVisiblePids, [ref]`$defaultAnyPids, [ref]`$hiddenDesktopPids)
    if (`$hiddenDesktopPids.Count -gt 0) {
        if (`$defaultVisiblePids.Count -eq 0) {
            Get-Process -Name 'ChatGPT*','codex-computer-use*' | Stop-Process -Force
            Start-Sleep -Milliseconds 500
        } else {
            foreach (`$hpid in `$hiddenDesktopPids) {
                & taskkill.exe /F /PID `$hpid /T 2>&1 | Out-Null
            }
            Start-Sleep -Milliseconds 300
        }
    }
}
if (-not (Get-Process -Name 'ChatGPT*')) {
    `$lockCandidates = @("`$env:APPDATA\Codex\web\Codex\lockfile")
    `$pkgLocks = Get-ChildItem "`$env:LOCALAPPDATA\Packages\OpenAI.Codex*\LocalCache\Roaming\Codex\web\Codex\lockfile" -File -ErrorAction SilentlyContinue
    if (`$pkgLocks) { `$lockCandidates += `$pkgLocks.FullName }
    `$lockCandidates | Where-Object { Test-Path `$_ } | Remove-Item -Force
}

# 7. Ensure SQLite triggers are installed across all state_*.sqlite databases in `$codexDir` and `$codexDir\sqlite`
`$dbPaths = New-Object System.Collections.Generic.List[string]
foreach (`$sDir in @(`$codexDir, (Join-Path `$codexDir 'sqlite'))) {
    if (Test-Path `$sDir) {
        Get-ChildItem -Path `$sDir -Filter 'state_*.sqlite' -File | ForEach-Object {
            if (-not `$dbPaths.Contains(`$_.FullName)) { `$dbPaths.Add(`$_.FullName) }
        }
        `$devDb = Join-Path `$sDir 'codex-dev.db'
        if ((Test-Path `$devDb) -and -not `$dbPaths.Contains(`$devDb)) { `$dbPaths.Add(`$devDb) }
    }
}
if ((`$dbPaths.Count -gt 0) -and (Get-Command python -ErrorAction SilentlyContinue)) {
    `$pyTrig = @'
import sqlite3, sys
provider = sys.argv[1]
models = [m.strip() for m in sys.argv[2:6] if m.strip()] + ['implement', 'explore', 'review']
db_paths = sys.argv[6:]
def q(s):
    return "'" + s.replace("'", "''") + "'"
unique_models = []
for m in models:
    if m not in unique_models:
        unique_models.append(m)
models_in_clause = ", ".join(q(m) for m in unique_models)
provider_lit = q(provider.strip() or '9router')
required_cols = {'id', 'model', 'model_provider', 'agent_role', 'thread_source'}
for db_path in db_paths:
    try:
        conn = sqlite3.connect(db_path, timeout=5.0)
        cur = conn.cursor()
        cur.execute("SELECT 1 FROM sqlite_master WHERE type='table' AND name='threads'")
        if cur.fetchone() is None:
            conn.close()
            continue
        cur.execute("PRAGMA table_info(threads);")
        existing_cols = {row[1] for row in cur.fetchall()}
        if not required_cols.issubset(existing_cols):
            conn.close()
            continue
        cur.execute("DROP TRIGGER IF EXISTS fix_subagent_provider_trigger;")
        cur.execute("DROP TRIGGER IF EXISTS fix_subagent_provider_update_trigger;")
        cur.execute(f"""
        CREATE TRIGGER fix_subagent_provider_trigger
        AFTER INSERT ON threads
        FOR EACH ROW
        WHEN COALESCE(NEW.agent_role, '') NOT IN ('guardian_classifier', 'guardian_review')
         AND (NEW.agent_role IS NOT NULL OR NEW.thread_source = 'subagent' OR NEW.thread_source LIKE '%subagent%' OR NEW.model LIKE '%9router%' OR NEW.model IN ({models_in_clause}))
        BEGIN
            UPDATE threads SET model_provider = {provider_lit} WHERE id = NEW.id;
        END;
        """)
        cur.execute(f"""
        CREATE TRIGGER fix_subagent_provider_update_trigger
        AFTER UPDATE OF model_provider, model ON threads
        FOR EACH ROW
        WHEN NEW.model_provider != {provider_lit}
         AND COALESCE(NEW.agent_role, '') NOT IN ('guardian_classifier', 'guardian_review')
         AND (NEW.agent_role IS NOT NULL OR NEW.thread_source = 'subagent' OR NEW.thread_source LIKE '%subagent%' OR NEW.model LIKE '%9router%' OR NEW.model IN ({models_in_clause}))
        BEGIN
            UPDATE threads SET model_provider = {provider_lit} WHERE id = NEW.id;
        END;
        """)
        conn.commit()
        conn.close()
    except Exception:
        pass
'@
    `$pyArgs = @('-', '$($Provider -replace "'","''")', '$($DefaultModel -replace "'","''")', '$($WorkerModel -replace "'","''")', '$($ExplorerModel -replace "'","''")', '$($ReviewerModel -replace "'","''")') + @(`$dbPaths)
    `$pyTrig | & python @pyArgs 2>&1 | Out-Null
}

function Test-ProxyDaemonRunning {
    param([string]`$CustomDir, [int]`$Port)
    `$lockFile = Join-Path `$CustomDir "proxy-daemon-`$Port.lock"
    if (Test-Path `$lockFile) {
        try {
            `$fs = [System.IO.File]::Open(`$lockFile, [System.IO.FileMode]::Open, [System.IO.FileAccess]::Read, [System.IO.FileShare]::None)
            `$fs.Close()
            `$fs.Dispose()
        } catch {
            return `$true
        }
    }
    `$daemonProc = Get-CimInstance Win32_Process -Filter "Name = 'codex-9router-subagents.exe' OR Name = 'codex-9router-proxy.exe' OR Name = 'codex.exe'" |
        Where-Object { `$_.CommandLine -match '--proxy-daemon' }
    return [bool]`$daemonProc
}

if (Test-Path `$proxyPath) {
    & `$proxyPath --doctor 2>&1 | Out-Null
    if ((-not (Test-ProxyDaemonRunning -CustomDir `$customDir -Port `$proxyPort)) -or (-not (Get-NetTCPConnection -LocalPort `$proxyPort -State Listen -ErrorAction SilentlyContinue))) {
        Start-Process -FilePath `$proxyPath -ArgumentList "--proxy-daemon" -WindowStyle Hidden
    }
}
"@
[System.IO.File]::WriteAllText($syncScriptPath, $syncScript, $utf8NoBom)

$startupFolder = [System.Environment]::GetFolderPath([System.Environment+SpecialFolder]::Startup)
$startupCmd = Join-Path $startupFolder "Codex9RouterHookSync.cmd"
$cmdContent = "@echo off`r`npowershell.exe -NoProfile -ExecutionPolicy Bypass -WindowStyle Hidden -File `"%LOCALAPPDATA%\OpenAI\Codex\custom\hook-sync.ps1`"`r`n"
[System.IO.File]::WriteAllText($startupCmd, $cmdContent, $utf8NoBom)
Write-Host "[OK] Registered self-healing startup hook in $startupCmd." -ForegroundColor Green

# 11. Terminate any stale .old.* processes that respawned during copy and clean up .old.* files
Get-NetTCPConnection -LocalPort $proxyPort -State Listen -ErrorAction SilentlyContinue | ForEach-Object {
    $procId = [int]$_.OwningProcess
    if ($procId -gt 0 -and -not $ancestorPids.Contains($procId)) {
        $p = Get-Process -Id $procId -ErrorAction SilentlyContinue
        $livePath = Get-LiveProcessImagePath $p
        if ($p -and ($p.Name -like "codex*.old*" -or ($livePath -and $livePath -like "*.old.*"))) {
            Stop-Process -Id $p.Id -Force -ErrorAction SilentlyContinue
        }
    }
}
Get-Process -ErrorAction SilentlyContinue | Where-Object {
    (-not $ancestorPids.Contains([int]$_.Id)) -and (
        $_.Name -like "codex*.old*" -or
        ($_.Name -like "codex*" -and ((Get-LiveProcessImagePath $_) -like "*.old.*"))
    )
} | Stop-Process -Force -ErrorAction SilentlyContinue
Start-Sleep -Milliseconds 600
Get-ChildItem "$desktopBinRoot\*\*.old.*", "$customDir\*.old.*" -ErrorAction SilentlyContinue | Remove-Item -Force -ErrorAction SilentlyContinue
foreach ($ehDir in $extraHookDirs) {
    if (Test-Path $ehDir) {
        Get-ChildItem (Join-Path $ehDir "*.old.*") -ErrorAction SilentlyContinue | Remove-Item -Force -ErrorAction SilentlyContinue
    }
}

# 12. Ensure background reverse-proxy listener (:$proxyPort) and app-server daemon are running
Write-Host "[*] Ensuring reverse proxy listener on 127.0.0.1:$proxyPort..." -ForegroundColor Gray
try {
    if ((-not (Test-ProxyDaemonRunning -CustomDir $customDir -Port $proxyPort)) -or (-not (Get-NetTCPConnection -LocalPort $proxyPort -State Listen -ErrorAction SilentlyContinue))) {
        Start-Process -FilePath $customShim -ArgumentList "--proxy-daemon" -WindowStyle Hidden
        Start-Sleep -Milliseconds 500
    }
    Start-Process -FilePath $customShim -ArgumentList "app-server", "daemon", "restart" -WindowStyle Hidden
    Write-Host "[OK] Codex reverse-proxy listener and daemon restart dispatched successfully." -ForegroundColor Green
} catch {
    Write-Host "[i] Daemon restart skipped (will initialize on next Codex launch)." -ForegroundColor Gray
}

Write-Host ""
Write-Host "===================================================================" -ForegroundColor Green
Write-Host "                     Installation Complete!                        " -ForegroundColor Green
Write-Host "===================================================================" -ForegroundColor Green
Write-Host "Subagents in Codex Desktop & CLI will now natively route to:"
Write-Host "  Provider       : $Provider" -ForegroundColor Cyan
Write-Host "  Endpoint       : $Endpoint" -ForegroundColor Cyan
Write-Host "  Default Model  : $DefaultModel" -ForegroundColor Cyan
Write-Host "  Worker Model   : $WorkerModel" -ForegroundColor Cyan
Write-Host "  Explorer Model : $ExplorerModel" -ForegroundColor Cyan
Write-Host "  Reviewer Model : $ReviewerModel" -ForegroundColor Cyan
Write-Host ""

# Run doctor check to verify everything
& $releaseBinary --doctor
Check-McpServerStatuses

# Broadcast WM_SETTINGCHANGE so running Explorer and terminal sessions refresh environment variables
try {
    if (-not ("Win32EnvNotify" -as [type])) {
        Add-Type -Namespace Win32Env -Name NativeMethods -MemberDefinition @"
            [DllImport("user32.dll", SetLastError = true, CharSet = CharSet.Auto)]
            public static extern IntPtr SendMessageTimeout(
                IntPtr hWnd, uint Msg, UIntPtr wParam, string lParam,
                uint fuFlags, uint uTimeout, out UIntPtr lpdwResult);
"@ -ErrorAction SilentlyContinue
    }
    $HWND_BROADCAST = [IntPtr]0xffff
    $WM_SETTINGCHANGE = 0x001A
    $SMTO_ABORTIFHUNG = 0x0002
    $result = [UIntPtr]::Zero
    [Win32Env.NativeMethods]::SendMessageTimeout($HWND_BROADCAST, $WM_SETTINGCHANGE, [UIntPtr]::Zero, "Environment", $SMTO_ABORTIFHUNG, 3000, [ref]$result) | Out-Null
} catch {}
