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

# 1. Stop running codex processes and port 20129 listeners
Write-Host "[*] Stopping running codex processes..." -ForegroundColor Gray
Get-NetTCPConnection -LocalPort 20129 -ErrorAction SilentlyContinue | ForEach-Object {
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

# 2. Clean up CODEX_CLI_PATH if it points to our custom wrapper
$userCliPath = [System.Environment]::GetEnvironmentVariable("CODEX_CLI_PATH", "User")
if ($userCliPath -and ($userCliPath -like "*codex-9router-subagents*" -or $userCliPath -like "*OpenAI\Codex\custom\*")) {
    [System.Environment]::SetEnvironmentVariable("CODEX_CLI_PATH", $null, "User")
    Write-Host "[OK] Removed CODEX_CLI_PATH ($userCliPath) from User Environment." -ForegroundColor Green
}
if ($env:CODEX_CLI_PATH -and ($env:CODEX_CLI_PATH -like "*codex-9router-subagents*" -or $env:CODEX_CLI_PATH -like "*OpenAI\Codex\custom\*")) {
    Remove-Item Env:CODEX_CLI_PATH -ErrorAction SilentlyContinue
    Write-Host "[OK] Cleared CODEX_CLI_PATH from current session." -ForegroundColor Green
}

# 3. Restore Desktop App Binaries
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
$customDir = Join-Path $env:LOCALAPPDATA "OpenAI\Codex\custom"
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

# 4. Restore Daemon Binaries
$daemonReleases = Join-Path $env:USERPROFILE ".codex\packages\app-server-daemon\releases"
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

# 5. Remove Self-Healing Hook
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
