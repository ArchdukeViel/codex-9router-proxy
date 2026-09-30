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

# 1. Stop running codex processes and verified proxy port listeners (excluding current process & ancestor chain)
function Get-AncestorProcessIds {
    $ancestors = New-Object 'System.Collections.Generic.HashSet[int]'
    $curPid = $PID
    for ($depth = 0; $depth -lt 16 -and $curPid -gt 0; $depth++) {
        [void]$ancestors.Add([int]$curPid)
        try {
            $procInfo = Get-CimInstance Win32_Process -Filter "ProcessId = $curPid" -ErrorAction Stop
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
$excludedPids = Get-AncestorProcessIds

$parsedPort = 0
$userPortRaw = [System.Environment]::GetEnvironmentVariable("CODEX_PROXY_PORT", "User")
$proxyPort = if ($env:CODEX_PROXY_PORT -and [int]::TryParse($env:CODEX_PROXY_PORT.Trim(), [ref]$parsedPort) -and $parsedPort -gt 0) {
    $parsedPort
} elseif ($userPortRaw -and [int]::TryParse($userPortRaw.Trim(), [ref]$parsedPort) -and $parsedPort -gt 0) {
    $parsedPort
} else {
    20129
}
$exactCodexProcessNames = @(
    "codex",
    "codex.orig",
    "codex-9router-proxy",
    "codex-9router-subagents",
    "codex-9router-subagents.orig",
    "codex-command-runner",
    "codex-windows-sandbox-setup",
    "codex-windows-sandbox-service",
    "codex-code-mode-host"
)
if (-not ([System.Management.Automation.PSTypeName]'CodexProcessNative').Type) {
    Add-Type -TypeDefinition @"
using System;
using System.Text;
using System.Runtime.InteropServices;
public static class CodexProcessNative {
    [DllImport("kernel32.dll", SetLastError=true)]
    public static extern IntPtr OpenProcess(uint access, bool inherit, int pid);
    [DllImport("kernel32.dll", SetLastError=true, CharSet=CharSet.Unicode)]
    public static extern bool QueryFullProcessImageNameW(IntPtr hProcess, int dwFlags, StringBuilder exeName, ref int lpdwSize);
    [DllImport("kernel32.dll")]
    public static extern bool CloseHandle(IntPtr hObject);
    public static string GetLiveImagePath(int pid) {
        IntPtr h = OpenProcess(0x1000, false, pid);
        if (h == IntPtr.Zero) return null;
        try {
            int cap = 2048;
            StringBuilder sb = new StringBuilder(cap);
            if (QueryFullProcessImageNameW(h, 0, sb, ref cap)) return sb.ToString();
            return null;
        } finally {
            CloseHandle(h);
        }
    }
}
"@ -ErrorAction SilentlyContinue
}
function Get-LiveProcessImagePath([System.Diagnostics.Process]$proc) {
    if (-not $proc) { return $null }
    try {
        $livePath = [CodexProcessNative]::GetLiveImagePath([int]$proc.Id)
        if ($livePath) { return $livePath }
    } catch {}
    try { return $proc.Path } catch { return $null }
}
Write-Host "[*] Stopping running codex processes and verified port $proxyPort listeners..." -ForegroundColor Gray
Get-NetTCPConnection -LocalPort $proxyPort -ErrorAction SilentlyContinue | ForEach-Object {
    $procId = $_.OwningProcess
    if ($procId -gt 0 -and -not $excludedPids.Contains([int]$procId)) {
        $ownerProc = Get-Process -Id $procId -ErrorAction SilentlyContinue
        if ($ownerProc -and (($exactCodexProcessNames -contains $ownerProc.Name) -or ($ownerProc.Name -like "codex*.old*"))) {
            Stop-Process -Id $procId -Force -ErrorAction SilentlyContinue
        }
    }
}
foreach ($procName in $exactCodexProcessNames) {
    Get-Process -Name $procName -ErrorAction SilentlyContinue | Where-Object { -not $excludedPids.Contains([int]$_.Id) } | ForEach-Object {
        Stop-Process -Id $_.Id -Force -ErrorAction SilentlyContinue
    }
}
Get-Process -ErrorAction SilentlyContinue | Where-Object {
    if ($excludedPids.Contains([int]$_.Id)) { return $false }
    if ($_.Name -like "codex*.old*") { return $true }
    if ($exactCodexProcessNames -contains $_.Name -or $_.Name -like "codex*") {
        $livePath = Get-LiveProcessImagePath $_
        if ($livePath -and $livePath -like "*.old.*") { return $true }
    }
    return $false
} | ForEach-Object {
    Stop-Process -Id $_.Id -Force -ErrorAction SilentlyContinue
}
Start-Sleep -Milliseconds 500

# 2. Clean up CODEX_CLI_PATH, subagent routing environment variables, and User PATH
$configuredSubagentProvider = [System.Environment]::GetEnvironmentVariable("CODEX_SUBAGENT_PROVIDER", "User")
if (-not $configuredSubagentProvider -and $env:CODEX_SUBAGENT_PROVIDER) {
    $configuredSubagentProvider = $env:CODEX_SUBAGENT_PROVIDER
}
if (-not $configuredSubagentProvider) {
    $configuredSubagentProvider = "9router"
}

$discoveredSubagentModels = New-Object System.Collections.Generic.List[string]
foreach ($envModelVar in @("CODEX_DEFAULT_MODEL", "CODEX_WORKER_MODEL", "CODEX_EXPLORER_MODEL", "CODEX_REVIEWER_MODEL")) {
    $uVal = [System.Environment]::GetEnvironmentVariable($envModelVar, "User")
    if ($uVal -and $uVal.Trim() -and -not $discoveredSubagentModels.Contains($uVal.Trim())) {
        $discoveredSubagentModels.Add($uVal.Trim())
    }
}

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
    "CODEX_PROXY_PORT",
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
    $filteredUserPath = ($userPath -split ';' | Where-Object { $_.Trim() -and ($_.Trim().TrimEnd('\') -ine $customDir.TrimEnd('\')) }) -join ';'
    if ($filteredUserPath -ne $userPath) {
        [System.Environment]::SetEnvironmentVariable("Path", $filteredUserPath, "User")
        Write-Host "[OK] Removed $customDir from User PATH." -ForegroundColor Green
    }
}
if ($env:Path) {
    $env:Path = ($env:Path -split ';' | Where-Object { $_.Trim() -and ($_.Trim().TrimEnd('\') -ine $customDir.TrimEnd('\')) }) -join ';'
}

# 3. Clean up codex-9router-proxy managed sections in ~/.codex/config.toml, ~/.codex/agents/*.toml, and ~/.codex/models_cache.json
$utf8NoBom = New-Object System.Text.UTF8Encoding($false)
$codexDir = if ($env:CODEX_HOME -and $env:CODEX_HOME.Trim()) { $env:CODEX_HOME.Trim() } else { Join-Path $env:USERPROFILE ".codex" }
$discoveredProviders = New-Object System.Collections.Generic.List[string]
$discoveredProviders.Add("9router")
if ($configuredSubagentProvider -and -not $discoveredProviders.Contains($configuredSubagentProvider)) {
    $discoveredProviders.Add($configuredSubagentProvider)
}

$agentsDir = Join-Path $codexDir "agents"
if (Test-Path $agentsDir) {
    foreach ($roleName in @("default", "worker", "explorer", "reviewer")) {
        $roleFile = Join-Path $agentsDir "$roleName.toml"
        if (Test-Path $roleFile) {
            try {
                $roleContent = [System.IO.File]::ReadAllText($roleFile, $utf8NoBom)
                $escapedSubProv = [regex]::Escape($configuredSubagentProvider)
                if ($roleContent -match "(?m)^\s*model_provider\s*=\s*`"(9router|$escapedSubProv)`"" -or
                    $roleContent -match "9router-subagent" -or
                    $roleContent -match "Subagent model routed via Codex 9Router Proxy") {
                    if ($roleContent -match '(?m)^\s*model_provider\s*=\s*"([^"]+)"') {
                        $rp = $Matches[1].Trim()
                        if ($rp -and -not $discoveredProviders.Contains($rp)) { $discoveredProviders.Add($rp) }
                    }
                    if ($roleContent -match '(?m)^\s*model\s*=\s*"([^"]+)"') {
                        $rm = $Matches[1].Trim()
                        if ($rm -and -not $discoveredSubagentModels.Contains($rm)) { $discoveredSubagentModels.Add($rm) }
                    }
                    Remove-Item -Path $roleFile -Force -ErrorAction SilentlyContinue
                    Write-Host "[OK] Removed managed role manifest $roleFile" -ForegroundColor Green
                }
            } catch {}
        }
    }
}

$configTomlPath = Join-Path $codexDir "config.toml"
if (Test-Path $configTomlPath) {
    try {
        $rawConfig = [System.IO.File]::ReadAllText($configTomlPath, $utf8NoBom)
        $updatedConfig = $rawConfig
        if ($updatedConfig -match '(?ms)^\s*\[subagent_models\]\r?\n(.*?)(?=^\s*\[|\z)') {
            $subModelsBlock = $Matches[1]
            [regex]::Matches($subModelsBlock, '(?m)^\s*(?:default|worker|explorer|reviewer)\s*=\s*"([^"]+)"') | ForEach-Object {
                $sm = $_.Groups[1].Value.Trim()
                if ($sm -and -not $discoveredSubagentModels.Contains($sm)) {
                    $discoveredSubagentModels.Add($sm)
                }
            }
        }
        foreach ($prov in ($discoveredProviders | Select-Object -Unique)) {
            if (-not $prov) { continue }
            $escapedProv = [regex]::Escape($prov)
            $updatedConfig = [regex]::Replace(
                $updatedConfig,
                "(?ms)^\s*\[model_providers\.$escapedProv\]\r?\n.*?(?=^\s*\[|\z)",
                ""
            )
        }
        $updatedConfig = [regex]::Replace(
            $updatedConfig,
            "(?ms)^\s*\[subagent_models\]\r?\n.*?(?=^\s*\[|\z)",
            ""
        )
        $updatedConfig = [regex]::Replace(
            $updatedConfig,
            "(?ms)^\s*\[mcp_servers\.subagent_9router\]\r?\n.*?(?=^\s*\[|\z)",
            ""
        )
        foreach ($roleName in @("default", "worker", "explorer", "reviewer")) {
            $escapedRole = [regex]::Escape($roleName)
            $updatedConfig = [regex]::Replace(
                $updatedConfig,
                "(?ms)^\s*\[agents\.$escapedRole\]\r?\n(?:[^\[]*?agents[\\/]+$escapedRole\.toml[^\[]*?)(?=^\s*\[|\z)",
                ""
            )
        }
        $updatedConfig = [regex]::Replace(
            $updatedConfig,
            '(?m)^\s*default_subagent_model\s*=\s*"[^"]*"\s*\r?\n?',
            ""
        )
        $updatedConfig = [regex]::Replace(
            $updatedConfig,
            '(?ms)^\s*\[agents\]\s*\r?\n(?=\s*\[|\z)',
            ""
        )
        $updatedConfig = [regex]::Replace(
            $updatedConfig,
            '(?m)^\s*chatgpt_base_url\s*=\s*"https?://127\.0\.0\.1:\d+/backend-api/?"\s*\r?\n?',
            ""
        )
        $updatedConfig = [regex]::Replace($updatedConfig, "(\r?\n){3,}", "`r`n`r`n").Trim() + "`r`n"
        if ($updatedConfig -ne $rawConfig) {
            [System.IO.File]::WriteAllText($configTomlPath, $updatedConfig, $utf8NoBom)
            Write-Host "[OK] Cleaned codex-9router-proxy provider and role sections from $configTomlPath" -ForegroundColor Green
        }
    } catch {}
}

$modelsCachePath = Join-Path $codexDir "models_cache.json"
if (Test-Path $modelsCachePath) {
    try {
        $cacheRaw = [System.IO.File]::ReadAllText($modelsCachePath, $utf8NoBom)
        $cacheObj = $cacheRaw | ConvertFrom-Json -ErrorAction Stop
        if ($cacheObj -and $null -ne $cacheObj.models) {
            $managedSlugs = @("9router-subagent", "implement", "explore", "review") + @($discoveredSubagentModels)
            $beforeCount = @($cacheObj.models).Count
            $filteredModels = @($cacheObj.models | Where-Object {
                $slug = [string]$_.slug
                $desc = [string]$_.description
                $disp = [string]$_.display_name
                $isOfficialSlug = ($slug -like "gpt-*") -or ($slug -like "o1*") -or ($slug -like "o3*") -or ($slug -like "o4*") -or ($slug -like "codex*")
                $isManagedSlug = ($slug -in $managedSlugs) -and (-not $isOfficialSlug)
                $isInjectedEntry = (-not $isOfficialSlug) -and (
                    ($_.comp_hash -eq "3000") -or
                    ($desc -eq "Subagent model routed via Codex 9Router Proxy") -or
                    ($disp -like "* (9Router)")
                )
                -not ($isManagedSlug -or $isInjectedEntry)
            })
            if ($filteredModels.Count -lt $beforeCount) {
                $cacheObj.models = $filteredModels
                $newCacheJson = $cacheObj | ConvertTo-Json -Depth 30
                [System.IO.File]::WriteAllText($modelsCachePath, $newCacheJson, $utf8NoBom)
                Write-Host "[OK] Removed injected subagent models from $modelsCachePath" -ForegroundColor Green
            }
        }
    } catch {}
}

# 4. Drop SQLite subagent provider trigger across all state_*.sqlite and codex-dev.db databases
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
conn = sqlite3.connect(sys.argv[1], timeout=5.0)
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

# 5. Discover Microsoft Store Codex resources (including HKCU AppModel Repository & OpenAI.CodexPrimaryRuntime*, sorted by [version])
function Get-PackageSortVersion {
    param([string]$Text)
    if ($Text -match '_(\d+\.\d+\.\d+\.\d+)_') {
        $v = $null
        if ([version]::TryParse($Matches[1], [ref]$v)) { return $v }
    }
    $v2 = $null
    if ([version]::TryParse($Text, [ref]$v2)) { return $v2 }
    return [version]"0.0.0.0"
}

function Get-CodexStoreResourceDir {
    $appModelRegPath = "HKCU:\Software\Classes\Local Settings\Software\Microsoft\Windows\CurrentVersion\AppModel\Repository\Packages"
    if (Test-Path $appModelRegPath) {
        $pkgKeys = Get-ChildItem -Path $appModelRegPath -ErrorAction SilentlyContinue |
            Where-Object { $_.PSChildName -like "OpenAI.Codex*" } |
            Sort-Object { Get-PackageSortVersion $_.PSChildName } -Descending
        foreach ($k in $pkgKeys) {
            try {
                $rootFolder = (Get-ItemProperty -Path $k.PSPath -Name "PackageRootFolder" -ErrorAction SilentlyContinue).PackageRootFolder
                if ($rootFolder -and (Test-Path $rootFolder)) {
                    $resCand = Join-Path $rootFolder "app\resources"
                    if (Test-Path (Join-Path $resCand "codex.exe")) { return $resCand }
                    if (Test-Path (Join-Path $rootFolder "codex.exe")) { return $rootFolder }
                }
            } catch {}
        }
    }
    $storePkgs = Get-AppxPackage -Name "*OpenAI.Codex*" -ErrorAction SilentlyContinue | Sort-Object { Get-PackageSortVersion $_.Version } -Descending
    foreach ($storePkg in $storePkgs) {
        if ($storePkg -and $storePkg.InstallLocation) {
            $cand = Join-Path $storePkg.InstallLocation "app\resources"
            if (Test-Path (Join-Path $cand "codex.exe")) { return $cand }
            if (Test-Path (Join-Path $storePkg.InstallLocation "codex.exe")) { return $storePkg.InstallLocation }
        }
    }
    $winAppsCandidates = Get-ChildItem "C:\Program Files\WindowsApps\OpenAI.Codex*" -Directory -ErrorAction SilentlyContinue | Sort-Object { Get-PackageSortVersion $_.Name } -Descending
    foreach ($winAppsCandidate in $winAppsCandidates) {
        $cand = Join-Path $winAppsCandidate.FullName "app\resources"
        if (Test-Path (Join-Path $cand "codex.exe")) { return $cand }
        if (Test-Path (Join-Path $winAppsCandidate.FullName "codex.exe")) { return $winAppsCandidate.FullName }
    }
    return $null
}

$storeResDir = Get-CodexStoreResourceDir
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
Get-ChildItem "$desktopBinRoot\*\*.old.*" -ErrorAction SilentlyContinue | Remove-Item -Force -ErrorAction SilentlyContinue

# 6. Restore Standalone CLI and VS Code / Cursor / Windsurf / Antigravity Extension Binaries
$externalBinDirs = New-Object System.Collections.Generic.List[string]
$standaloneCliBin = Join-Path $env:LOCALAPPDATA "Programs\OpenAI\Codex\bin"
if (Test-Path $standaloneCliBin) {
    $externalBinDirs.Add($standaloneCliBin)
}
foreach ($extRoot in @(
    (Join-Path $env:USERPROFILE ".vscode\extensions"),
    (Join-Path $env:USERPROFILE ".vscode-insiders\extensions"),
    (Join-Path $env:USERPROFILE ".cursor\extensions"),
    (Join-Path $env:USERPROFILE ".windsurf\extensions"),
    (Join-Path $env:USERPROFILE ".antigravity\extensions"),
    (Join-Path $env:USERPROFILE ".antigravity-ide\extensions")
)) {
    if (Test-Path $extRoot) {
        Get-ChildItem -Path $extRoot -Directory -Filter "openai.chatgpt-*" -ErrorAction SilentlyContinue | ForEach-Object {
            $extBinRoot = Join-Path $_.FullName "bin"
            if (Test-Path $extBinRoot) {
                if (Test-Path (Join-Path $extBinRoot "codex.exe")) {
                    $externalBinDirs.Add($extBinRoot)
                }
                Get-ChildItem -Path $extBinRoot -Directory -ErrorAction SilentlyContinue | ForEach-Object {
                    if ((Test-Path (Join-Path $_.FullName "codex.exe")) -or (Test-Path (Join-Path $_.FullName "codex.orig.exe"))) {
                        $externalBinDirs.Add($_.FullName)
                    }
                }
            }
        }
    }
}
foreach ($extDir in ($externalBinDirs | Select-Object -Unique)) {
    $c = Join-Path $extDir "codex.exe"
    $o = Join-Path $extDir "codex.orig.exe"
    if ((Test-Path $o) -and ((Get-Item $o).Length -gt 10000000)) {
        Move-Item -Path $o -Destination $c -Force -ErrorAction SilentlyContinue
        Write-Host "[OK] Restored official binary at $c" -ForegroundColor Green
    } elseif ((Test-Path $c) -and ((Get-Item $c).Length -lt 10000000) -and $storeResDir -and (Test-Path (Join-Path $storeResDir "codex.exe"))) {
        Copy-Item -Path (Join-Path $storeResDir "codex.exe") -Destination $c -Force -ErrorAction SilentlyContinue
        if (Test-Path $o) { Remove-Item -Path $o -Force -ErrorAction SilentlyContinue }
        Write-Host "[OK] Restored official binary at $c from Microsoft Store resources" -ForegroundColor Green
    }
    Get-ChildItem (Join-Path $extDir "*.old.*") -ErrorAction SilentlyContinue | Remove-Item -Force -ErrorAction SilentlyContinue
}

# 7. Restore Daemon Binaries
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

# 8. Remove Self-Healing Hook and Custom Shim Directory
$startupFolder = [System.Environment]::GetFolderPath([System.Environment+SpecialFolder]::Startup)
$startupCmd = Join-Path $startupFolder "Codex9RouterHookSync.cmd"
if (Test-Path $startupCmd) {
    Remove-Item -Path $startupCmd -Force -ErrorAction SilentlyContinue
    Write-Host "[OK] Removed startup hook $startupCmd." -ForegroundColor Green
}
Unregister-ScheduledTask -TaskName "Codex9RouterHookSync" -Confirm:$false -ErrorAction SilentlyContinue | Out-Null
Write-Host "[OK] Cleaned up scheduled tasks." -ForegroundColor Green

if (Test-Path $customDir) {
    Remove-Item -Path $customDir -Recurse -Force -ErrorAction SilentlyContinue
    if (-not (Test-Path $customDir)) {
        Write-Host "[OK] Removed custom shim directory $customDir." -ForegroundColor Green
    }
}

Write-Host ""
Write-Host "[OK] Uninstallation complete. Official stock binaries restored." -ForegroundColor Green
