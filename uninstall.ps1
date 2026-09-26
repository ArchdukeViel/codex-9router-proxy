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

# 1. Stop running codex processes
Write-Host "[*] Stopping running codex processes..." -ForegroundColor Gray
Get-Process -Name "*codex*" -ErrorAction SilentlyContinue | Where-Object { $_.Id -ne $PID } | ForEach-Object {
    Stop-Process -Id $_.Id -Force -ErrorAction SilentlyContinue
}
Start-Sleep -Milliseconds 500

# 2. Restore Desktop App Binaries
$desktopBinRoot = Join-Path $env:LOCALAPPDATA "OpenAI\Codex\bin"
if (Test-Path $desktopBinRoot) {
    Get-ChildItem -Path $desktopBinRoot -Directory | ForEach-Object {
        $c = Join-Path $_.FullName "codex.exe"
        $o = Join-Path $_.FullName "codex.orig.exe"
        if (Test-Path $o) {
            Move-Item -Path $o -Destination $c -Force
            Write-Host "[OK] Restored official binary at $c" -ForegroundColor Green
        }
    }
}

# 3. Restore Daemon Binaries
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

# 4. Remove Self-Healing Hook
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
