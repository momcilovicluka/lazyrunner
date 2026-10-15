<#
.SYNOPSIS
    Set up lazyrunner on this machine. Run it from your repo root, to make that project the default:
        cd C:\path\to\your\repo
        & 'C:\path\to\lazyrunner\install.ps1'

.DESCRIPTION
    - Checks for git, the .NET SDK, Node.js/npm and a usable execution policy.
    - Writes lazyrunner.config.json next to lazyrunner.ps1: repoRoot (the project you ran it in, if any) and the on/off
      settings below. Re-running keeps your saved values unless you pass new ones.
    - Unblocks the scripts (Windows blocks files that came from a download or an attachment).
    - Adds or updates the lazyrunner and lr aliases in your PowerShell profile.

.PARAMETER Frontend
    on/off: also run the project's npm apps. Default: keep the saved value, else on. Needs a project.

.PARAMETER AutoStart
    on/off: build and start everything as soon as lazyrunner opens (, then u in the panel toggles it). Default: keep, else on.

.PARAMETER AutoRestart
    on/off: restart automatically when the git branch changes (, then a in the panel toggles it). Default: keep, else off.

.PARAMETER StopFrontend
    on/off: whether Stop/Restart all also stop the npm apps (, then s in the panel toggles it). Default: keep, else on.
    Needs a project.

.PARAMETER SkipUnchangedBuilds
    on/off: skip dotnet build when nothing in the watched folder changed since the last build. Default: keep, else on.

.PARAMETER AutoUpdate
    on/off: install a newer lazyrunner from its release repo when it opens (, then v in the panel toggles it).
    Default: keep, else on.

.PARAMETER Notifications
    on/off: Windows notifications for failed builds, crashes and slow starts finishing. Default: keep, else on.
    Works from Windows PowerShell 5.1 too: it installs PowerShell 7 with winget if needed and reruns itself there.
#>
# No "#Requires -Version 7.0": 5.1 must get far enough to install PowerShell 7. Keep this file ASCII-only, since
# 5.1 reads BOM-less scripts as ANSI and a UTF-8 character can turn into a quote that breaks parsing.
[CmdletBinding()]
param(
    [ValidateSet('on', 'off')][string]$Frontend,
    [ValidateSet('on', 'off')][string]$AutoStart,
    [ValidateSet('on', 'off')][string]$AutoRestart,
    [ValidateSet('on', 'off')][string]$StopFrontend,
    [ValidateSet('on', 'off')][string]$SkipUnchangedBuilds,
    [ValidateSet('on', 'off')][string]$Notifications,
    [ValidateSet('on', 'off')][string]$AutoUpdate,
    [ValidateSet('on', 'off')][string]$StartupChecklist,
    [ValidateSet('on', 'off')][string]$RestartOnCodeChange,
    [string]$Theme   # a file name from themes\ without .json, e.g. dracula
)

$ErrorActionPreference = 'Stop'

# lazyrunner needs PowerShell 7 (pwsh). From an older shell, install it if missing, then rerun this script in it, so the
# aliases land in the PowerShell 7 profile rather than the Windows PowerShell one.
function Find-Pwsh {
    $onPath = Get-Command pwsh -ErrorAction SilentlyContinue
    if ($onPath) { return $onPath.Source }
    # A fresh winget install isn't on this session's PATH yet.
    $installed = Join-Path $env:ProgramFiles 'PowerShell\7\pwsh.exe'
    if (Test-Path $installed) { return $installed }
}

function Install-Pwsh {
    if (-not (Get-Command winget -ErrorAction SilentlyContinue)) {
        throw 'PowerShell 7 is required and winget is not available. Install it from https://aka.ms/powershell-release?tag=stable, then rerun install.ps1 from pwsh.'
    }
    Write-Host 'PowerShell 7 not found - installing it with winget...' -ForegroundColor Cyan
    winget install --id Microsoft.PowerShell --exact --source winget --accept-source-agreements --accept-package-agreements
    $pwsh = Find-Pwsh
    if (-not $pwsh) { throw "winget couldn't install PowerShell 7 (exit code $LASTEXITCODE). Install it from https://aka.ms/powershell-release?tag=stable, then rerun install.ps1 from pwsh." }
    $pwsh
}

if ($PSVersionTable.PSVersion.Major -lt 7) {
    $pwsh = Find-Pwsh
    if ($pwsh) { Write-Host "PowerShell 7 found ($pwsh)" -ForegroundColor Green } else { $pwsh = Install-Pwsh }
    Write-Host 'Continuing the install in PowerShell 7...' -ForegroundColor Cyan
    $forward = foreach ($name in $PSBoundParameters.Keys) { "-$name"; $PSBoundParameters[$name] }
    & $pwsh -NoLogo -NoProfile -ExecutionPolicy Bypass -WorkingDirectory (Get-Location).ProviderPath -File $PSCommandPath @forward
    exit $LASTEXITCODE
}

# The project this folder belongs to, found the way lazyrunner finds it (lib\profile.ps1): its root becomes repoRoot,
# the folder lazyrunner uses when it runs outside any project. None is fine: lazyrunner works inside any project's
# checkout.
. (Join-Path $PSScriptRoot 'lib\profile.ps1')
$Project = $null
try {
    $found = Find-ProjectProfile (Get-Location).ProviderPath $PSScriptRoot
    if ($found) { $Project = Resolve-ProjectProfile $found $PSScriptRoot }
}
catch { Write-Host $_.Exception.Message -ForegroundColor Red; exit 1 }
$LazyRunnerScript = Join-Path $PSScriptRoot 'lazyrunner.ps1'
$ConfigPath       = Join-Path $PSScriptRoot 'lazyrunner.config.json'
$ProfilePath      = $PROFILE.CurrentUserCurrentHost
$AliasMarker      = '# lazyrunner (managed by lazyrunner\install.ps1)'
$Aliases          = @('lazyrunner', 'lr')
$Tools            = [ordered]@{ git = 'Git for Windows'; dotnet = 'the .NET SDK (for .NET apps)'; npm = 'Node.js (for npm apps)' }

# Keep in sync with $ConfigDefaults in lazyrunner.ps1 (without the settings each project keeps in its own section,
# $ProjectKeys). Switches maps each on/off parameter to its config key.
$Defaults = [ordered]@{
    repoRoot                  = ''
    autoStartOnOpen           = $true
    autoRestartOnBranchChange = $false
    skipUnchangedBuilds       = $true
    notifications             = $true
    panelLogLines             = 5
    startupChecklist          = $true
    theme                     = 'default'
    autoUpdate                = $true
}
$ProjectKeys = @('columnWidths', 'apps', 'frontend', 'stopFrontendOnStop')
$Switches = [ordered]@{
    Frontend            = 'frontend'
    AutoStart           = 'autoStartOnOpen'
    AutoRestart         = 'autoRestartOnBranchChange'
    StopFrontend        = 'stopFrontendOnStop'
    SkipUnchangedBuilds = 'skipUnchangedBuilds'
    Notifications       = 'notifications'
    StartupChecklist    = 'startupChecklist'
    RestartOnCodeChange = 'restartOnCodeChange'
    AutoUpdate          = 'autoUpdate'
}
$SwitchValues = @{ Frontend = $Frontend; AutoStart = $AutoStart; AutoRestart = $AutoRestart; StopFrontend = $StopFrontend; SkipUnchangedBuilds = $SkipUnchangedBuilds; Notifications = $Notifications; StartupChecklist = $StartupChecklist; RestartOnCodeChange = $RestartOnCodeChange; AutoUpdate = $AutoUpdate }

function Write-Ok([string]$Text) { Write-Host "  $([char]0x2713) $Text" -ForegroundColor Green }
function Write-Warn([string]$Text) { Write-Host "  ! $Text" -ForegroundColor Yellow }

function Test-Prerequisites {
    foreach ($tool in $Tools.Keys) {
        if (Get-Command $tool -ErrorAction SilentlyContinue) { Write-Ok "$tool found" }
        else { Write-Warn "$tool not found on PATH - install $($Tools[$tool])" }
    }
    $policy = Get-ExecutionPolicy
    if ($policy -in 'Restricted', 'AllSigned') {
        Write-Warn "Execution policy is $policy, so lazyrunner can't run. Allow local scripts with: Set-ExecutionPolicy -Scope CurrentUser RemoteSigned"
    }
}

function Save-Config([string]$RepoRoot) {
    $config = [ordered]@{} + $Defaults
    if (Test-Path $ConfigPath) {
        try {
            $saved = Get-Content $ConfigPath -Raw | ConvertFrom-Json -AsHashtable
            foreach ($key in $saved.Keys) { $config[$key] = $saved[$key] }
        }
        catch { Write-Warn "Ignoring unreadable $ConfigPath ($($_.Exception.Message))" }
    }
    if ($Theme) {
        $available = @(Get-ChildItem (Join-Path $PSScriptRoot 'themes') -Filter *.json -File | ForEach-Object BaseName)
        if ($available -notcontains $Theme) { throw "Unknown theme '$Theme'. Available: $($available -join ', ')" }
        $config.theme = $Theme
    }
    if ($RepoRoot) { $config.repoRoot = $RepoRoot }
    foreach ($param in $Switches.Keys) {
        if (-not $SwitchValues[$param]) { continue }
        $key = $Switches[$param]
        $value = $SwitchValues[$param] -eq 'on'
        if ($ProjectKeys -notcontains $key) { $config[$key] = $value; continue }
        if (-not $Project) { Write-Warn "-$param needs a project: run install.ps1 inside its repo"; continue }
        # A project's settings live in its own section, "projects": { "<id>": { ... } }.
        if ($config.projects -isnot [System.Collections.IDictionary]) { $config.projects = [ordered]@{} }
        if (-not $config.projects[$Project.Id]) { $config.projects[$Project.Id] = [ordered]@{} }
        $config.projects[$Project.Id][$key] = $value
    }
    $config | ConvertTo-Json -Depth 6 | Set-Content $ConfigPath -Encoding utf8
    Write-Ok "config saved to $ConfigPath"
    foreach ($key in $config.Keys) {
        $value = if ($config[$key] -is [System.Collections.IDictionary]) { ($config[$key].GetEnumerator() | ForEach-Object { "$($_.Key)=$($_.Value)" }) -join ', ' } else { $config[$key] }
        Write-Host "      $key = $value"
    }
}

function Install-Alias {
    New-Item -ItemType Directory -Force (Split-Path $ProfilePath) | Out-Null
    $lines = if (Test-Path $ProfilePath) { @(Get-Content $ProfilePath) } else { @() }
    $ours = '^\s*Set-Alias\s+(-Name\s+)?(' + ($Aliases -join '|') + ')\s'
    $kept = @($lines | Where-Object { $_ -ne $AliasMarker -and $_ -notmatch $ours })
    $aliasLines = foreach ($alias in $Aliases) { "Set-Alias -Name $alias -Value '$($LazyRunnerScript.Replace("'", "''"))'" }
    Set-Content $ProfilePath ($kept + $AliasMarker + $aliasLines) -Encoding utf8
    $verb = if ($kept.Count -lt $lines.Count) { 'updated in' } else { 'added to' }
    Write-Ok "$($Aliases -join ' and ') aliases $verb $ProfilePath"
}

if (-not (Test-Path $LazyRunnerScript)) { throw "lazyrunner.ps1 not found next to install.ps1 ($PSScriptRoot)." }
$repoRoot = ''
if ($Project) {
    $repoRoot = $Project.Root
    Write-Host "Installing lazyrunner for $($Project.Name) in $repoRoot" -ForegroundColor Cyan
}
else {
    Write-Host 'Installing lazyrunner' -ForegroundColor Cyan
    Write-Warn "no project here (no .lazyrunner.json and no known profile's marker file in this folder or a parent) - repoRoot stays as it was"
}
Test-Prerequisites
Save-Config $repoRoot
Get-ChildItem $PSScriptRoot -Filter *.ps1 -Recurse | Unblock-File
Write-Ok 'scripts unblocked'
Install-Alias

Write-Host ''
Write-Host 'Done. Open a new terminal (or run  . $PROFILE ) and type  lazyrunner  (or  lr )' -ForegroundColor Cyan
if ($repoRoot) { Write-Host "Inside a project's checkout lazyrunner uses that checkout; anywhere else it uses $repoRoot." }
else { Write-Host "lazyrunner works inside a project's checkout: a repo with a .lazyrunner.json, or one your profiles know (projects\README.md)." }
