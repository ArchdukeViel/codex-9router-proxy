#Requires -Version 5.1
<#
.SYNOPSIS
    Interactive Installer for codex-9router-proxy.
    Routes OpenAI Codex Desktop & CLI subagents natively to 9Router or any custom OpenAI-compatible endpoint.

.DESCRIPTION
    Hooks the official Codex Desktop & CLI binaries to intercept JSON-RPC messages and transparently
    route child subagent threads to 9Router while preserving your primary ChatGPT session on gpt-6-luna.

.PARAMETER Provider
    Model provider identifier (default: "9router").

.PARAMETER Endpoint
    Base URL for the provider endpoint (default: "http://localhost:20128/v1").

.PARAMETER ApiKey
    API Key for the provider. If omitted, reads from existing environment or registry.

.PARAMETER NonInteractive
    Suppresses interactive prompts, using parameter values and defaults.
#>

[CmdletBinding()]
param(
    [string]$Provider,
    [string]$Endpoint,
    [string]$ApiKey,
    [switch]$NonInteractive
)

$ErrorActionPreference = "Stop"

Write-Host "===================================================================" -ForegroundColor Cyan
Write-Host "         Codex 9Router Proxy - Native GUI Subagents Installer      " -ForegroundColor Cyan
Write-Host "===================================================================" -ForegroundColor Cyan
Write-Host ""

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
            $keyInput = Read-Host "[?] Enter API Key" -AsSecureString
            $bstr = [System.Runtime.InteropServices.Marshal]::SecureStringToBSTR($keyInput)
            $plain = [System.Runtime.InteropServices.Marshal]::PtrToStringAuto($bstr)
            [System.Runtime.InteropServices.Marshal]::ZeroFreeBSTR($bstr)
            $ApiKey = $plain.Trim()
        }
    }
} else {
    if (-not $Provider) { $Provider = "9router" }
    if (-not $Endpoint) { $Endpoint = "http://localhost:20128/v1" }
    if (-not $ApiKey) {
        $ApiKey = [System.Environment]::GetEnvironmentVariable("NINEROUTER_KEY", "User")
    }
}

# Normalize endpoint (ensure trailing slash if required, or strip extra)
$Endpoint = $Endpoint.TrimEnd('/')

Write-Host ""
Write-Host "[+] Target Configuration:" -ForegroundColor Green
Write-Host "    Provider : $Provider"
Write-Host "    Endpoint : $Endpoint"
Write-Host "    API Key  : [PROTECTED / CONFIGURED]"
Write-Host ""

# 2. Persist Environment Variables & Registry
if (-not [string]::IsNullOrWhiteSpace($ApiKey)) {
    [System.Environment]::SetEnvironmentVariable("NINEROUTER_KEY", $ApiKey, "User")
    $env:NINEROUTER_KEY = $ApiKey
    Write-Host "[OK] Saved NINEROUTER_KEY to Windows User Environment (Registry HKCU\Environment)." -ForegroundColor Green
}
[System.Environment]::SetEnvironmentVariable("CODEX_SUBAGENT_PROVIDER", $Provider, "User")
$env:CODEX_SUBAGENT_PROVIDER = $Provider

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

# Replace existing provider block or append
$pattern = "(?ms)\[model_providers\.$Provider\].*?(?=\n\[|\z)"
if ($configContent -match $pattern) {
    $configContent = [System.Text.RegularExpressions.Regex]::Replace($configContent, $pattern, $providerSection.Trim())
} else {
    $configContent = $configContent.TrimEnd() + "`r`n`r`n" + $providerSection.Trim() + "`r`n"
}

# Ensure [agents] default_subagent_model is set
if ($configContent -notmatch "(?m)^default_subagent_model\s*=") {
    if ($configContent -match "\[agents\]") {
        $configContent = $configContent -replace "\[agents\]", "[agents]`r`ndefault_subagent_model = `"9router-subagent`""
    } else {
        $configContent = $configContent.TrimEnd() + "`r`n`r`n[agents]`r`ndefault_subagent_model = `"9router-subagent`"`r`n"
    }
}

# Ensure chatgpt_base_url is clean / not routing primary chat turns through invalid proxy
if ($configContent -match "(?m)^chatgpt_base_url\s*=") {
    $configContent = [System.Text.RegularExpressions.Regex]::Replace($configContent, "(?m)^chatgpt_base_url\s*=.*`r?`n", "")
}

$utf8NoBom = New-Object System.Text.UTF8Encoding($false)
[System.IO.File]::WriteAllText($configFile, $configContent, $utf8NoBom)
Write-Host "[OK] Configured $configFile (BOM-Free UTF-8)." -ForegroundColor Green

# 4. Build Release Binary
$scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$releaseBinary = Join-Path $scriptDir "target\release\codex-9router-proxy.exe"

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

# 5. Stop running codex processes before hooking
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
                # In Windows, a running binary cannot be overwritten, but CAN be moved aside
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

# 6. Hook Desktop App Binaries
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
                # If large official binary, back it up to codex.orig.exe
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

# 7. Hook Daemon Binaries
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

# 8. Deploy Custom Standalone Copy
$customDir = Join-Path $localAppData "OpenAI\Codex\custom"
if (-not (Test-Path $customDir)) {
    New-Item -ItemType Directory -Path $customDir -Force | Out-Null
}
$customShim = Join-Path $customDir "codex-9router-subagents.exe"
if (Safe-CopyExecutable -Source $releaseBinary -Destination $customShim) {
    Write-Host "[OK] Deployed standalone shim to $customShim" -ForegroundColor Green
}

# 9. Register Self-Healing Startup Hook
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

# 10. Restart Daemon if available
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
Write-Host "  Provider : $Provider" -ForegroundColor Cyan
Write-Host "  Endpoint : $Endpoint" -ForegroundColor Cyan
Write-Host "  Model    : worker -> implement, explorer -> explore, default -> 9router-subagent" -ForegroundColor Cyan
Write-Host ""
Write-Host "Launch the Codex Desktop App and test subagents with live sidebar streaming!" -ForegroundColor Yellow
Write-Host "===================================================================" -ForegroundColor Green
