# Shared helpers: state files, the project and its config, repo/branch lookup, and the CLI side (trust, requests,
# running the UI).
# Dot-sourced by lazyrunner.ps1; uses the settings defined there.

function Read-Json([string]$Path) {
    for ($i = 0; $i -lt 5; $i++) {
        if (-not (Test-Path $Path)) { return $null }
        try { return [IO.File]::ReadAllText($Path) | ConvertFrom-Json -AsHashtable }
        catch { Start-Sleep -Milliseconds 40 }   # another process is rewriting it
    }
    $null
}

function Write-Json([string]$Path, $Data) {
    $json = $Data | ConvertTo-Json -Depth 10
    for ($i = 0; $i -lt 10; $i++) {
        try { [IO.File]::WriteAllText($Path, $json); return }
        catch { Start-Sleep -Milliseconds 40 }
    }
    throw "lazyrunner: could not write $Path"
}

# ---------------------------------------------------------------- the project

# Makes a resolved profile (Resolve-ProjectProfile) the one lazyrunner runs: $ProjectProfile, $RepoMarker, $SourceDir, $Apps
# (without the apps turned off on this machine), $AppIcons and $ProjectStateDir.
function Use-ProjectProfile($Project) {
    $script:ProjectProfile = $Project
    $script:RepoMarker = $Project.RepoMarker
    $script:SourceDir = $Project.SourceDir
    $script:Apps = $Project.Apps
    $script:AppIcons = $Project.Icons   # each app's icon in its title bar (■ when the profile gives none)
    $script:ProjectStateDir = Join-Path $StateDir $Project.Id
    $Project.AppKeys = @($Apps.Keys)   # all of them, to tell what a branch's profile changed
    $config = Get-Config
    foreach ($key in @($Apps.Keys)) { if (-not (Test-AppEnabled $config $key)) { $Apps.Remove($key) } }
    if (-not $Apps.Count) { throw "lazyrunner: every app of $($Project.Name) is turned off in $ConfigPath (apps.<key>.enabled, frontend)" }
}

# The project for the folder lazyrunner runs in (or a parent), else for repoRoot from lazyrunner.config.json; unresolved
# (Find-ProjectProfile).
function Find-Project {
    $found = Find-ProjectProfile (Get-Location).ProviderPath $LazyRunnerRoot
    if ($found) { return $found }
    $fallback = (Get-Config).repoRoot
    if ($fallback -and (Test-Path -LiteralPath $fallback)) { $found = Find-ProjectProfile (Resolve-Path -LiteralPath $fallback).ProviderPath $LazyRunnerRoot }
    if ($found) { return $found }
    $where = if ($fallback) { "and repoRoot '$fallback' (in $ConfigPath) isn't one either" } else { 'and no repoRoot is configured' }
    throw "lazyrunner: no project here - neither this folder nor a parent has a $RepoProfileFile or the marker file of a known profile, $where. Add a $RepoProfileFile (see projects\README.md), or run install.ps1 from your repo root."
}

# The project's state folder (build stamps and logs, timings, npm ci, the columns' logs), made on first use.
function Initialize-ProjectState {
    New-Item -ItemType Directory -Force $ProjectStateDir | Out-Null
}

# ---------------------------------------------------------------- config

# The settings for this machine: the defaults, then lazyrunner.config.json - its top level for everything, and for the
# settings that belong to a project ($ProjectConfigKeys) that project's section, "projects": { "<id>": { … } }.
function Get-Config {
    $config = $ConfigDefaults.Clone()
    $saved = Read-Json $ConfigPath
    if (-not $saved) { return $config }
    foreach ($key in $saved.Keys) { if ($key -ne 'projects' -and $key -notin $ProjectConfigKeys) { $config[$key] = $saved[$key] } }
    $section = Get-ProjectConfig $saved
    foreach ($key in $section.Keys) { $config[$key] = $section[$key] }
    $config
}

# This project's section of a saved config (empty when it has none yet, or before a project is chosen).
function Get-ProjectConfig($Saved) {
    if (-not $ProjectProfile) { return @{} }
    $section = $Saved.projects.($ProjectProfile.Id)
    if ($section -is [hashtable]) { return $section }
    @{}
}

# Saves one setting - a project's in its section, the rest at the top - and returns the config as Get-Config reads it.
function Set-ConfigValue([string]$Name, $Value) {
    $saved = Read-Json $ConfigPath
    if (-not $saved) { $saved = @{} }
    if ($ProjectProfile -and $Name -in $ProjectConfigKeys) {
        $section = Get-ProjectConfig $saved
        $section[$Name] = $Value
        if ($saved.projects -isnot [hashtable]) { $saved.projects = @{} }
        $saved.projects[$ProjectProfile.Id] = $section
    }
    else { $saved[$Name] = $Value }
    Write-Json $ConfigPath $saved
    Get-Config
}

# ---------------------------------------------------------------- trust (the CLI side)

# Asks before using a repo profile that isn't trusted yet, or changed since (Test-ProfileTrusted). $Ask asks the
# question (Read-Host; tests pass their own).
function Confirm-ProfileTrust($Project, [scriptblock]$Ask = { param($Prompt) Read-Host $Prompt }) {
    if (Test-ProfileTrusted $Project $TrustStorePath) { return $true }
    $state = if ((Read-TrustStore $TrustStorePath).ContainsKey($Project.Path)) { 'changed since you trusted it' } else { "is this repo's own profile for lazyrunner" }
    Write-Host "$($Project.Path) $state. $($Project.Name) runs:" -ForegroundColor Yellow
    foreach ($line in Get-ProfileSummary $Project) { Write-Host $line }
    $answer = & $Ask 'Trust it and run these? [y/N]'
    if ("$answer".Trim() -notmatch '^(y|yes)$') { Write-Host 'Not trusted - lazyrunner did not start.' -ForegroundColor DarkGray; return $false }
    Approve-ProjectProfile $Project $TrustStorePath
    $true
}

function Assert-Prerequisites {
    $missing = @('git', 'dotnet') | Where-Object { -not (Get-Command $_ -ErrorAction SilentlyContinue) }
    if ($missing) { throw "lazyrunner: not found on PATH: $($missing -join ', '). Install it and try again." }
    if ([Console]::IsInputRedirected -or [Console]::IsOutputRedirected) { throw 'lazyrunner: needs an interactive terminal (input or output is redirected).' }
}

function Get-LiveProcess([int]$Id) {
    if ($Id -gt 0) { Get-Process -Id $Id -ErrorAction SilentlyContinue }
}

function Get-Controller {
    $info = Read-Json $ControllerPath
    if (-not $info) { return $null }
    $process = Get-LiveProcess $info.pid
    if ($process -and $process.StartTime.Ticks -eq $info.startTicks) { return $info }
    $null
}

# node + npm-cli.js avoids npm.cmd's extra cmd.exe layer.
function Get-NpmLauncher {
    $npm = Get-Command npm.cmd -ErrorAction SilentlyContinue
    if (-not $npm) { throw 'npm was not found on PATH - install Node.js' }
    $cli = Join-Path (Split-Path $npm.Source) 'node_modules\npm\bin\npm-cli.js'
    $node = Get-Command node.exe -ErrorAction SilentlyContinue
    if ($node -and (Test-Path $cli)) { return @{ File = $node.Source; Prefix = @($cli) } }
    @{ File = $npm.Source; Prefix = @() }
}

function Get-GitDir([string]$RepoRoot) {
    $dir = git -C $RepoRoot rev-parse --absolute-git-dir 2>$null
    if ($LASTEXITCODE -eq 0) { $dir } else { $null }
}

function Get-Branch([string]$GitDir) {
    if (-not $GitDir) { return '' }
    try { $head = [IO.File]::ReadAllText((Join-Path $GitDir 'HEAD')).Trim() } catch { return $null }
    if ($head -match '^ref: refs/heads/(.+)$') { return $Matches[1] }
    "$($head.Substring(0, [Math]::Min(8, $head.Length))) (detached)"
}

function Send-Request([string]$Action, [string]$RepoRoot) {
    Write-Json $RequestPath @{ action = $Action; root = $RepoRoot; id = [guid]::NewGuid().ToString() }
    Write-Host "lazyrunner is open in another terminal - sent it '$Action'." -ForegroundColor Cyan
}

# Runs the UI in its own pwsh: its console settings, types and child-process job never leak into this shell, and when
# it exits (q), everything it started is gone. When it exits to reopen - for another project, or to reload this one's
# profile (exit code $ReopenExitCode, where to in reopen.json) - it starts again there, after the trust question if
# that profile needs one.
function Start-Ui([string]$RepoRoot, [bool]$StartNow) {
    while ($true) {
        $uiArgs = @('-NoLogo', '-NoProfile', '-File', $LazyRunnerScriptPath, '__ui', '-Root', $RepoRoot) + $(if ($StartNow) { '-AutoStart' } else { @() })
        & (Get-Process -Id $PID).Path @uiArgs
        if ($LASTEXITCODE -ne $ReopenExitCode) { return }
        $reopen = Read-Json $ReopenPath
        Remove-Item $ReopenPath -ErrorAction SilentlyContinue
        if (-not $reopen) { return }
        $found = Find-ProjectProfile $reopen.root $LazyRunnerRoot
        if (-not $found) { Write-Host "lazyrunner: $($reopen.root) is not a project (no profile matches it)." -ForegroundColor Red; return }
        $project = try { Resolve-ProjectProfile $found $LazyRunnerRoot } catch { Write-Host $_.Exception.Message -ForegroundColor Red; return }
        if (-not (Confirm-ProfileTrust $project)) { return }
        $RepoRoot = $project.Root
        $StartNow = [bool]$reopen.start
    }
}
