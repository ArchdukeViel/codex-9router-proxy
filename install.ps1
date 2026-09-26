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
    [ValidateSet("9router", "ollama", "lmstudio", "openrouter", "vllm", "litellm")]
    [string]$Preset,
    [switch]$Doctor,
    [switch]$NonInteractive
)

$ErrorActionPreference = "Stop"
$utf8NoBom = New-Object System.Text.UTF8Encoding($false)
$scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$releaseBinary = Join-Path $scriptDir "target\release\codex-9router-proxy.exe"

# If -Doctor requested, run diagnostic check immediately
if ($Doctor) {
    if (Test-Path $releaseBinary) {
        & $releaseBinary --doctor
    } else {
        $customShim = Join-Path $env:LOCALAPPDATA "OpenAI\Codex\custom\codex-9router-subagents.exe"
        if (Test-Path $customShim) {
            & $customShim --doctor
        } else {
            Write-Error "No compiled codex-9router-proxy binary found. Run .\install.ps1 first to build and install."
        }
    }
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
        }
        "ollama" {
            if (-not $Provider) { $Provider = "ollama" }
            if (-not $Endpoint) { $Endpoint = "http://localhost:11434/v1" }
            if (-not $DefaultModel) { $DefaultModel = "qwen2.5-coder:32b" }
        }
        "lmstudio" {
            if (-not $Provider) { $Provider = "lmstudio" }
            if (-not $Endpoint) { $Endpoint = "http://localhost:1234/v1" }
            if (-not $DefaultModel) { $DefaultModel = "local-model" }
        }
        "openrouter" {
            if (-not $Provider) { $Provider = "openrouter" }
            if (-not $Endpoint) { $Endpoint = "https://openrouter.ai/api/v1" }
            if (-not $DefaultModel) { $DefaultModel = "anthropic/claude-3.5-sonnet" }
        }
        "vllm" {
            if (-not $Provider) { $Provider = "vllm" }
            if (-not $Endpoint) { $Endpoint = "http://localhost:8000/v1" }
            if (-not $DefaultModel) { $DefaultModel = "default-model" }
        }
        "litellm" {
            if (-not $Provider) { $Provider = "litellm" }
            if (-not $Endpoint) { $Endpoint = "http://localhost:4000/v1" }
            if (-not $DefaultModel) { $DefaultModel = "default-model" }
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

    # Model Configuration Prompts per Role
    if (-not $DefaultModel) {
        $dmInput = Read-Host "[?] Enter Default Subagent Model [default: 9router-subagent]"
        $DefaultModel = if ([string]::IsNullOrWhiteSpace($dmInput)) { "9router-subagent" } else { $dmInput.Trim() }
    }

    if (-not $WorkerModel) {
        $wmInput = Read-Host "[?] Enter Worker Subagent Model [default: $DefaultModel]"
        $WorkerModel = if ([string]::IsNullOrWhiteSpace($wmInput)) { $DefaultModel } else { $wmInput.Trim() }
    }

    if (-not $ExplorerModel) {
        $emInput = Read-Host "[?] Enter Explorer Subagent Model [default: $DefaultModel]"
        $ExplorerModel = if ([string]::IsNullOrWhiteSpace($emInput)) { $DefaultModel } else { $emInput.Trim() }
    }

    if (-not $ReviewerModel) {
        $rmInput = Read-Host "[?] Enter Reviewer Subagent Model [default: $DefaultModel]"
        $ReviewerModel = if ([string]::IsNullOrWhiteSpace($rmInput)) { $DefaultModel } else { $rmInput.Trim() }
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
Write-Host ""

# 2. Persist Environment Variables & Registry
if (-not [string]::IsNullOrWhiteSpace($ApiKey)) {
    [System.Environment]::SetEnvironmentVariable("NINEROUTER_KEY", $ApiKey, "User")
    $env:NINEROUTER_KEY = $ApiKey
    Write-Host "[OK] Saved NINEROUTER_KEY to Windows User Environment (Registry HKCU\Environment)." -ForegroundColor Green
}
[System.Environment]::SetEnvironmentVariable("CODEX_SUBAGENT_PROVIDER", $Provider, "User")
$env:CODEX_SUBAGENT_PROVIDER = $Provider

[System.Environment]::SetEnvironmentVariable("CODEX_SUBAGENT_ENDPOINT", $Endpoint, "User")
$env:CODEX_SUBAGENT_ENDPOINT = $Endpoint

[System.Environment]::SetEnvironmentVariable("CODEX_DEFAULT_MODEL", $DefaultModel, "User")
$env:CODEX_DEFAULT_MODEL = $DefaultModel

[System.Environment]::SetEnvironmentVariable("CODEX_WORKER_MODEL", $WorkerModel, "User")
$env:CODEX_WORKER_MODEL = $WorkerModel

[System.Environment]::SetEnvironmentVariable("CODEX_EXPLORER_MODEL", $ExplorerModel, "User")
$env:CODEX_EXPLORER_MODEL = $ExplorerModel

[System.Environment]::SetEnvironmentVariable("CODEX_REVIEWER_MODEL", $ReviewerModel, "User")
$env:CODEX_REVIEWER_MODEL = $ReviewerModel

# 3. Configure ~/.codex/config.toml (BOM-Free UTF-8)
$codexDir = Join-Path $env:USERPROFILE ".codex"
if (-not (Test-Path $codexDir)) {
    New-Item -ItemType Directory -Path $codexDir -Force | Out-Null
}
$configFile = Join-Path $codexDir "config.toml"
$configContent = if (Test-Path $configFile) {
    [System.IO.File]::ReadAllText($configFile, [System.Text.Encoding]::UTF8)
} else {
    ""
}

# Update or insert [model_providers.<Provider>]
$providerSection = @"
[model_providers.$Provider]
name = "$Provider"
base_url = "$Endpoint"
env_key = "NINEROUTER_KEY"
"@
$pattern = "(?ms)\[model_providers\.$Provider\].*?(?=\n\[|\z)"
if ($configContent -match $pattern) {
    $configContent = [System.Text.RegularExpressions.Regex]::Replace($configContent, $pattern, $providerSection.Trim())
} else {
    $configContent = $configContent.TrimEnd() + "`r`n`r`n" + $providerSection.Trim() + "`r`n"
}

# Ensure [agents] default_subagent_model is set
$agentsSection = @"
[agents]
default_subagent_model = "$DefaultModel"
"@
$patternAgents = "(?ms)\[agents\].*?(?=\n\[|\z)"
if ($configContent -match "(?m)^\[agents\]") {
    if ($configContent -match "(?m)^default_subagent_model\s*=") {
        $configContent = [System.Text.RegularExpressions.Regex]::Replace($configContent, "(?m)^default_subagent_model\s*=.*", "default_subagent_model = `"$DefaultModel`"")
    } else {
        $configContent = $configContent -replace "\[agents\]", "[agents]`r`ndefault_subagent_model = `"$DefaultModel`""
    }
} else {
    $configContent = $configContent.TrimEnd() + "`r`n`r`n" + $agentsSection.Trim() + "`r`n"
}

# Ensure [subagent_models] section is configured
$subagentModelsSection = @"
[subagent_models]
default = "$DefaultModel"
worker = "$WorkerModel"
explorer = "$ExplorerModel"
reviewer = "$ReviewerModel"
"@
$patternSubModels = "(?ms)\[subagent_models\].*?(?=\n\[|\z)"
if ($configContent -match $patternSubModels) {
    $configContent = [System.Text.RegularExpressions.Regex]::Replace($configContent, $patternSubModels, $subagentModelsSection.Trim())
} else {
    $configContent = $configContent.TrimEnd() + "`r`n`r`n" + $subagentModelsSection.Trim() + "`r`n"
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
    
    # Write ~/.codex/agents/<role>.toml
    $roleTomlContent = @"
name = "$roleName"
description = "$($roleInfo.Desc)"
model = "$($roleInfo.Model)"
model_provider = "$Provider"
model_reasoning_effort = "high"
"@
    $targetToml = Join-Path $agentsDir "$roleName.toml"
    [System.IO.File]::WriteAllText($targetToml, $roleTomlContent, $utf8NoBom)

    # Link in config.toml
    $roleBlock = @"
[agents.$roleName]
description = "$($roleInfo.Desc)"
config_file = "$roleTomlPath"
"@
    $patternRole = "(?ms)\[agents\.$roleName\].*?(?=\n\[|\z)"
    if ($configContent -match $patternRole) {
        $configContent = [System.Text.RegularExpressions.Regex]::Replace($configContent, $patternRole, $roleBlock.Trim())
    } else {
        $configContent = $configContent.TrimEnd() + "`r`n`r`n" + $roleBlock.Trim() + "`r`n"
    }
}

# Ensure chatgpt_base_url is clean / direct
if ($configContent -match "(?m)^chatgpt_base_url\s*=") {
    $configContent = [System.Text.RegularExpressions.Regex]::Replace($configContent, "(?m)^chatgpt_base_url\s*=.*`r?`n", "")
}

[System.IO.File]::WriteAllText($configFile, $configContent, $utf8NoBom)
Write-Host "[OK] Configured $configFile and role manifests in $agentsDir (BOM-Free UTF-8)." -ForegroundColor Green

# 4. Inject SQLite Automatic Provider Trigger into Databases
$dbCandidates = @(
    (Join-Path $codexDir "state_5.sqlite"),
    (Join-Path $codexDir "sqlite\state_5.sqlite")
)

$hasPython = [bool](Get-Command python -ErrorAction SilentlyContinue)
if ($hasPython) {
    foreach ($db in $dbCandidates) {
        if (Test-Path $db) {
            $pyScript = @"
import sqlite3, sys
db_path = r'$db'
provider = r'$Provider'
try:
    conn = sqlite3.connect(db_path)
    cur = conn.cursor()
    cur.execute('''
    CREATE TRIGGER IF NOT EXISTS fix_subagent_provider_trigger
    AFTER INSERT ON threads
    FOR EACH ROW
    WHEN (NEW.agent_role IS NOT NULL OR NEW.thread_source = 'subagent' OR NEW.model LIKE '%9router%' OR NEW.model IN ('implement', 'explore', 'review'))
    BEGIN
        UPDATE threads SET model_provider = provider WHERE id = NEW.id;
    END;
    ''')
    conn.commit()
    conn.close()
    sys.exit(0)
except Exception as e:
    sys.stderr.write(str(e))
    sys.exit(1)
"@
            python -c $pyScript
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

# 5. Build Release Binary
if (-not (Test-Path $releaseBinary)) {
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
}

if (-not (Test-Path $releaseBinary)) {
    throw "Release binary not found at $releaseBinary"
}
Write-Host "[OK] Release binary ready ($((Get-Item $releaseBinary).Length) bytes)." -ForegroundColor Green

# 6. Stop running codex processes before hooking
Write-Host "[*] Checking for running codex processes..." -ForegroundColor Gray
Get-Process -Name "*codex*" -ErrorAction SilentlyContinue | Where-Object { $_.Id -ne $PID } | ForEach-Object {
    Write-Host "    Stopping process $($_.Name) (PID $($_.Id))..." -ForegroundColor Gray
    Stop-Process -Id $_.Id -Force -ErrorAction SilentlyContinue
}
Start-Sleep -Milliseconds 500

function Safe-CopyExecutable {
    param(
        [string]$Source,
        [string]$Destination
    )
    $destDir = Split-Path -Parent $Destination
    $fileName = Split-Path -Leaf $Destination
    if (-not (Test-Path $Destination)) {
        Copy-Item -Path $Source -Destination $Destination -Force
        return $true
    }
    $maxRetries = 5
    for ($attempt = 1; $attempt -le $maxRetries; $attempt++) {
        try {
            Copy-Item -Path $Source -Destination $Destination -Force -ErrorAction Stop
            return $true
        } catch {
            if ($attempt -lt $maxRetries) {
                Start-Sleep -Milliseconds 600
            } else {
                $tempOld = Join-Path $destDir "$fileName.old.$([Guid]::NewGuid().ToString('N').Substring(0,6))"
                try {
                    Move-Item -Path $Destination -Destination $tempOld -Force -ErrorAction Stop
                    Copy-Item -Path $Source -Destination $Destination -Force -ErrorAction Stop
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

# 7. Hook Desktop App Binaries
$localAppData = $env:LOCALAPPDATA
$desktopBinRoot = Join-Path $localAppData "OpenAI\Codex\bin"
$hookedCount = 0

if (Test-Path $desktopBinRoot) {
    Get-ChildItem -Path $desktopBinRoot -Directory | ForEach-Object {
        $targetDir = $_.FullName
        $codexExe = Join-Path $targetDir "codex.exe"
        $codexOrig = Join-Path $targetDir "codex.orig.exe"

        if ((Test-Path $codexExe) -or (Test-Path $codexOrig)) {
            if (Test-Path $codexExe) {
                $len = (Get-Item $codexExe).Length
                if ($len -gt 10000000 -and -not (Test-Path $codexOrig)) {
                    Copy-Item -Path $codexExe -Destination $codexOrig -Force
                    Write-Host "[OK] Backed up official binary to $codexOrig" -ForegroundColor Green
                }
            }
            if (Safe-CopyExecutable -Source $releaseBinary -Destination $codexExe) {
                Write-Host "[OK] Installed proxy hook to $codexExe" -ForegroundColor Green
                $hookedCount++
            }
        }
    }
}

# 8. Hook Daemon Binaries
$daemonReleases = Join-Path $codexDir "packages\app-server-daemon\releases"
if (Test-Path $daemonReleases) {
    Get-ChildItem -Path $daemonReleases -Directory | ForEach-Object {
        $dBin = Join-Path $_.FullName "bin"
        $codexExe = Join-Path $dBin "codex.exe"
        $codexOrig = Join-Path $dBin "codex.orig.exe"

        if ((Test-Path $codexExe) -or (Test-Path $codexOrig)) {
            if (Test-Path $codexExe) {
                $len = (Get-Item $codexExe).Length
                if ($len -gt 10000000 -and -not (Test-Path $codexOrig)) {
                    Copy-Item -Path $codexExe -Destination $codexOrig -Force
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

# 9. Deploy Custom Standalone Copy
$customDir = Join-Path $localAppData "OpenAI\Codex\custom"
if (-not (Test-Path $customDir)) {
    New-Item -ItemType Directory -Path $customDir -Force | Out-Null
}
$customShim = Join-Path $customDir "codex-9router-subagents.exe"
if (Safe-CopyExecutable -Source $releaseBinary -Destination $customShim) {
    Write-Host "[OK] Deployed standalone shim to $customShim" -ForegroundColor Green
}

# 10. Register Self-Healing Startup Hook
$syncScriptPath = Join-Path $customDir "hook-sync.ps1"
$syncScript = @"
`$ErrorActionPreference = 'SilentlyContinue'
`$proxyPath = '$customShim'
`$binRoot = Join-Path `$env:LOCALAPPDATA 'OpenAI\Codex\bin'
if (Test-Path `$binRoot -and Test-Path `$proxyPath) {
    Get-ChildItem -Path `$binRoot -Directory | ForEach-Object {
        `$c = Join-Path `$_.FullName 'codex.exe'
        `$o = Join-Path `$_.FullName 'codex.orig.exe'
        if (Test-Path `$c) {
            if ((Get-Item `$c).Length -gt 10000000 -and -not (Test-Path `$o)) {
                Copy-Item `$c `$o -Force
                Copy-Item `$proxyPath `$c -Force
            }
        }
    }
}
"@
[System.IO.File]::WriteAllText($syncScriptPath, $syncScript, $utf8NoBom)

$startupFolder = [System.Environment]::GetFolderPath([System.Environment+SpecialFolder]::Startup)
$startupCmd = Join-Path $startupFolder "Codex9RouterHookSync.cmd"
$cmdContent = "@echo off`r`npowershell.exe -NoProfile -ExecutionPolicy Bypass -WindowStyle Hidden -File `"$syncScriptPath`"`r`n"
[System.IO.File]::WriteAllText($startupCmd, $cmdContent, [System.Text.Encoding]::ASCII)
Write-Host "[OK] Registered self-healing startup hook in $startupCmd." -ForegroundColor Green

# 11. Restart Daemon if available
Write-Host "[*] Restarting app-server daemon..." -ForegroundColor Gray
try {
    Start-Process -FilePath $customShim -ArgumentList "app-server", "daemon", "restart" -WindowStyle Hidden
    Write-Host "[OK] Codex daemon restart dispatched successfully." -ForegroundColor Green
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
