#Requires -Version 5.1
<#
.SYNOPSIS
    Uninstalls codex-9router-proxy and restores original official Codex binaries.
#>

[CmdletBinding()]
param()

$ErrorActionPreference = "Continue"

Write-Host "===================================================================" -ForegroundColor Yellow
Write-Host "            Codex 9Router Proxy - Uninstaller                      " -ForegroundColor Yellow
Write-Host "===================================================================" -ForegroundColor Yellow

# 1. Stop running codex processes and proxy port listeners
$parsedPort = 0
$userPortRaw = [System.Environment]::GetEnvironmentVariable("CODEX_PROXY_PORT", "User")
$proxyPort = if ($env:CODEX_PROXY_PORT -and [int]::TryParse($env:CODEX_PROXY_PORT.Trim(), [ref]$parsedPort) -and $parsedPort -gt 0) {
    $parsedPort
} elseif ($userPortRaw -and [int]::TryParse($userPortRaw.Trim(), [ref]$parsedPort) -and $parsedPort -gt 0) {
    $parsedPort
} else {
    20129
}
Write-Host "[*] Stopping running codex processes and port $proxyPort listeners..." -ForegroundColor Gray
Get-NetTCPConnection -LocalPort $proxyPort -ErrorAction SilentlyContinue | ForEach-Object {
    $procId = $_.OwningProcess
    if ($procId -gt 0 -and $procId -ne $PID) {
        Stop-Process -Id $procId -Force -ErrorAction SilentlyContinue
    }
}
Get-Process -Name "*codex*" -ErrorAction SilentlyContinue | Where-Object { $_.Id -ne $PID } | ForEach-Object {
    Stop-Process -Id $_.Id -Force -ErrorAction SilentlyContinue
}
Get-Process -ErrorAction SilentlyContinue | Where-Object { $_.Name -like "*.old*" -or $_.Path -like "*.old.*" } | ForEach-Object {
    Stop-Process -Id $_.Id -Force -ErrorAction SilentlyContinue
}
Start-Sleep -Milliseconds 500

# 2. Clean up CODEX_CLI_PATH, subagent routing environment variables, and User PATH
$userCliPath = [System.Environment]::GetEnvironmentVariable("CODEX_CLI_PATH", "User")
if ($userCliPath -and ($userCliPath -like "*codex-9router-subagents*" -or $userCliPath -like "*OpenAI\Codex\custom\*")) {
    [System.Environment]::SetEnvironmentVariable("CODEX_CLI_PATH", $null, "User")
    Write-Host "[OK] Removed CODEX_CLI_PATH ($userCliPath) from User Environment." -ForegroundColor Green
}
if ($env:CODEX_CLI_PATH -and ($env:CODEX_CLI_PATH -like "*codex-9router-subagents*" -or $env:CODEX_CLI_PATH -like "*OpenAI\Codex\custom\*")) {
    Remove-Item Env:CODEX_CLI_PATH -ErrorAction SilentlyContinue
    Write-Host "[OK] Cleared CODEX_CLI_PATH from current session." -ForegroundColor Green
}

$envVarsToClean = @(
    "CODEX_SUBAGENT_PROVIDER",
    "CODEX_SUBAGENT_ENDPOINT",
    "CODEX_DEFAULT_MODEL",
    "CODEX_WORKER_MODEL",
    "CODEX_EXPLORER_MODEL",
    "CODEX_REVIEWER_MODEL",
    "NINEROUTER_KEY"
)
foreach ($varName in $envVarsToClean) {
    if ($null -ne [System.Environment]::GetEnvironmentVariable($varName, "User")) {
        [System.Environment]::SetEnvironmentVariable($varName, $null, "User")
        Write-Host "[OK] Removed $varName from User Environment." -ForegroundColor Green
    }
    if (Test-Path "Env:$varName") {
        Remove-Item "Env:$varName" -ErrorAction SilentlyContinue
    }
}

$customDir = Join-Path $env:LOCALAPPDATA "OpenAI\Codex\custom"
$userPath = [System.Environment]::GetEnvironmentVariable("Path", "User")
if ($userPath) {
    $filteredUserPath = ($userPath -split ';' | Where-Object { $_.Trim() -and ($_.Trim().TrimEnd('\') -ne $customDir.TrimEnd('\')) }) -join ';'
    if ($filteredUserPath -ne $userPath) {
        [System.Environment]::SetEnvironmentVariable("Path", $filteredUserPath, "User")
        Write-Host "[OK] Removed $customDir from User PATH." -ForegroundColor Green
    }
}
if ($env:Path) {
    $env:Path = ($env:Path -split ';' | Where-Object { $_.Trim() -and ($_.Trim().TrimEnd('\') -ne $customDir.TrimEnd('\')) }) -join ';'
}

# 3. Drop SQLite subagent provider trigger if present
$codexDir = if ($env:CODEX_HOME -and $env:CODEX_HOME.Trim()) { $env:CODEX_HOME.Trim() } else { Join-Path $env:USERPROFILE ".codex" }
$dbCandidates = @(
    (Join-Path $codexDir "state_5.sqlite"),
    (Join-Path $codexDir "sqlite\state_5.sqlite"),
    (Join-Path $codexDir "sqlite\codex-dev.db")
) | Where-Object { Test-Path $_ }

if ($dbCandidates.Count -gt 0) {
    $hasPython = $false
    if (Get-Command python -ErrorAction SilentlyContinue) {
        try {
            & python -c "import sqlite3" 2>$null | Out-Null
            if ($LASTEXITCODE -eq 0) { $hasPython = $true }
        } catch {}
    }
    if ($hasPython) {
        $pyDropScript = @'
import sqlite3, sys
conn = sqlite3.connect(sys.argv[1])
cur = conn.cursor()
cur.execute("DROP TRIGGER IF EXISTS fix_subagent_provider_trigger;")
conn.commit()
conn.close()
'@
        foreach ($dbFile in $dbCandidates) {
            try {
                & python -c $pyDropScript $dbFile 2>&1 | Out-Null
                if ($LASTEXITCODE -eq 0) {
                    Write-Host "[OK] Removed SQLite subagent provider trigger from $dbFile" -ForegroundColor Green
                }
            } catch {}
        }
    }
}

# 4. Restore Desktop App Binaries
$storeResDir = $null
$storePkg = Get-AppxPackage -Name "*OpenAI.Codex*" -ErrorAction SilentlyContinue | Sort-Object Version -Descending | Select-Object -First 1
if ($storePkg -and $storePkg.InstallLocation) {
    $cand = Join-Path $storePkg.InstallLocation "app\resources"
    if (Test-Path $cand) { $storeResDir = $cand }
}
if (-not $storeResDir) {
    $winAppsCandidate = Get-ChildItem "C:\Program Files\WindowsApps\OpenAI.Codex*" -Directory -ErrorAction SilentlyContinue | Sort-Object LastWriteTime -Descending | Select-Object -First 1
    if ($winAppsCandidate) {
        $cand = Join-Path $winAppsCandidate.FullName "app\resources"
        if (Test-Path $cand) { $storeResDir = $cand }
    }
}
$desktopBinRoot = Join-Path $env:LOCALAPPDATA "OpenAI\Codex\bin"
if (Test-Path $desktopBinRoot) {
    Get-ChildItem -Path $desktopBinRoot -Directory | ForEach-Object {
        $c = Join-Path $_.FullName "codex.exe"
        $o = Join-Path $_.FullName "codex.orig.exe"
        if ((Test-Path $o) -and ((Get-Item $o).Length -gt 10000000)) {
            Move-Item -Path $o -Destination $c -Force
            Write-Host "[OK] Restored official binary at $c" -ForegroundColor Green
        } elseif ((Test-Path $c) -and ((Get-Item $c).Length -lt 10000000) -and $storeResDir -and (Test-Path (Join-Path $storeResDir "codex.exe"))) {
            Copy-Item -Path (Join-Path $storeResDir "codex.exe") -Destination $c -Force
            if (Test-Path $o) { Remove-Item -Path $o -Force -ErrorAction SilentlyContinue }
            Write-Host "[OK] Restored official binary at $c from Microsoft Store resources" -ForegroundColor Green
        }
    }
}
Get-ChildItem "$desktopBinRoot\*\*.old.*", "$customDir\*.old.*" -ErrorAction SilentlyContinue | Remove-Item -Force -ErrorAction SilentlyContinue

# 5. Restore Daemon Binaries
$daemonReleases = Join-Path $codexDir "packages\app-server-daemon\releases"
if (Test-Path $daemonReleases) {
    Get-ChildItem -Path $daemonReleases -Directory | ForEach-Object {
        $dBin = Join-Path $_.FullName "bin"
        $c = Join-Path $dBin "codex.exe"
        $o = Join-Path $dBin "codex.orig.exe"
        if (Test-Path $o) {
            Move-Item -Path $o -Destination $c -Force
            Write-Host "[OK] Restored official daemon binary at $c" -ForegroundColor Green
        }
    }
}

# 6. Remove Self-Healing Hook
$startupFolder = [System.Environment]::GetFolderPath([System.Environment+SpecialFolder]::Startup)
$startupCmd = Join-Path $startupFolder "Codex9RouterHookSync.cmd"
if (Test-Path $startupCmd) {
    Remove-Item -Path $startupCmd -Force -ErrorAction SilentlyContinue
    Write-Host "[OK] Removed startup hook $startupCmd." -ForegroundColor Green
}
Unregister-ScheduledTask -TaskName "Codex9RouterHookSync" -Confirm:$false -ErrorAction SilentlyContinue | Out-Null
Write-Host "[OK] Cleaned up scheduled tasks." -ForegroundColor Green

Write-Host ""
Write-Host "[OK] Uninstallation complete. Official stock binaries restored." -ForegroundColor Green
