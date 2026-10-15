<#
.SYNOPSIS
    lazyrunner - run a project's .NET APIs and npm frontends side by side in one full-screen terminal app: builds,
    ports, readiness, logs and restarts handled, without opening an IDE.

.DESCRIPTION
    lazyrunner [start]   Open lazyrunner in this terminal: every app's output side by side, the control panel below.
                         Starts everything unless auto-start is off (lazyrunner start always starts). If lazyrunner
                         is already open in another terminal, tells it to start whatever is stopped.
    lazyrunner stop      Stop everything in the open lazyrunner (also cancels a running build).
    lazyrunner restart   Stop, rebuild if anything changed, run again.
    lazyrunner profile check [file] [-Json]   Check a project profile: its fields, and against the checkout it is for.
    lazyrunner profile init [folder] [-Save]  Propose a .lazyrunner.json for a checkout (-Save writes it).
                         projects\README.md describes profiles and how to write one.
    lazyrunner update    Update lazyrunner from its release repo now (asks for a login if git needs one).
    lazyrunner version   The installed version, where updates come from, and what's new in it.
    lr is short for lazyrunner.

    Keys (vim / lazygit style; lowercase = the focused app, uppercase = all apps; ? lists them all):
          space start/stop focused   u/U start   s/S stop (S cancels a build)   r/R restart   b/B full rebuild
          d database (switch the connection string in secrets.json)   i npm ci   w/W open in the browser
          o log file   x menu for the focused app   / search, n N next match   , settings   t theme
          : or Ctrl+P command palette   q / Ctrl+C quit   ! kill whatever holds a port
          h l Tab 1-9 focus   j k Ctrl+u Ctrl+d Ctrl+b Ctrl+f gg G scroll   z zoom   > < = column width

    The project is the one containing the current directory: a repo with its own .lazyrunner.json, or one a known
    profile's marker file identifies (projects\README.md); outside one, repoRoot from lazyrunner.config.json is used.

    Setup: run install.ps1 (from your repo root, to make it the default project). It writes lazyrunner.config.json
    next to this script and adds the lazyrunner and lr aliases to $PROFILE. Requires PowerShell 7+, git, and the .NET
    SDK and Node.js for the apps that need them.
#>
#Requires -Version 7.0
[CmdletBinding()]
param(
    [Parameter(Position = 0)]
    [ValidateSet('start', 'stop', 'restart', 'profile', 'update', 'version', 'help', '__ui')]
    [string]$Command = 'start',
    [Parameter(Position = 1)]
    [string]$Action,   # lazyrunner profile check | init
    [Parameter(Position = 2)]
    [string]$Path,     # its file or folder
    [switch]$Json,
    [switch]$Save,
    [string]$Root,
    [switch]$AutoStart
)

# ================================================================ PROJECT SETTINGS (same for everyone)
# Per-machine values live in lazyrunner.config.json - see install.ps1.

# The project - the file that marks its repo, the folder whose changes need a rebuild and its apps (the columns, left
# to right) - comes from a project profile (lib\profile.ps1, projects\README.md): the repo's own .lazyrunner.json, yours
# in %APPDATA%\lazyrunner\projects, or one in projects\. lazyrunner finds it at the entry point; Use-ProjectProfile sets:
#   $RepoMarker  file (relative to the repo root) that marks a checkout; searched upward from the current directory
#   $SourceDir   folder whose changes require a rebuild; used to skip unchanged builds
#   $Apps        key -> Name, Kind (the app type; lib\apptypes.ps1 says what each type does) and the type's fields.
#                dotnet: Profile (launch profile from Properties\launchSettings.json - ports come from there), Swagger
#                (the panel probes <Swagger>/index.html), Project. npm: Dir and Variants (Label, When, Script, then Port
#                for a dev server or ServedBy + ReadyFile for a watch build), picked from Dir\package.json on every
#                start. Turn an app off per machine with "apps": { "<key>": { "enabled": false } } in lazyrunner.config.json
#                ("frontend": false turns off every npm app).
#   $AppIcons, $ProjectStateDir (this project's folder in the state folder) and $ProjectProfile itself.
# LAZYRUNNER_PROFILE (a profile file) makes that the only profile, and LAZYRUNNER_PROFILES_DIR replaces your profiles' folder (tests).
. (Join-Path $PSScriptRoot 'lib\profile.ps1')
$LazyRunnerRoot = $PSScriptRoot
$ProjectProfile = $null

# Self-update (lib\update.ps1): the repo and branch releases are pushed to. lazyrunner installs a higher VERSION from
# there when it opens (and when the checklist runs again) unless "autoUpdate" is off. Empty Url = no updates.
# Per machine, "updateRepoUrl" / "updateBranch" in lazyrunner.config.json override these; LAZYRUNNER_UPDATE_REPO
# replaces the Url ("none": no updates, which the tests use).
$UpdateRepo = @{
    Url    = if ($env:LAZYRUNNER_UPDATE_REPO) { $env:LAZYRUNNER_UPDATE_REPO } else { 'https://github.com/momcilovicluka/lazyrunner.git' }
    Branch = 'main'
}

# Used for any value missing from lazyrunner.config.json.
$ConfigDefaults = @{
    repoRoot                  = ''
    frontend                  = $true
    autoStartOnOpen           = $true
    autoRestartOnBranchChange = $false
    stopFrontendOnStop        = $true    # off: Stop/Restart all (S/R, lazyrunner stop/restart) leave the npm apps running
    skipUnchangedBuilds       = $true
    notifications             = $true
    columnWidths              = @{}   # relative width per app key (33 when missing, so equal by default; > < adjust the focused one)
    panelLogLines             = 5
    restartOnCodeChange       = $false   # rebuild + restart the running apps that build when the watch folder changes (, then e)
    startupChecklist          = $true   # splash that checks tools, ports, database and packages when lazyrunner opens
    theme                     = 'default'   # a file name from themes\ without .json (t cycles); NO_COLOR turns colour off
    autoUpdate                = $true   # install a newer lazyrunner from $UpdateRepo when it opens (, then v)
    updateRepoUrl             = ''      # overrides $UpdateRepo.Url on this machine
    updateBranch              = ''      # overrides $UpdateRepo.Branch on this machine
}
# The settings that belong to a project: lazyrunner.config.json keeps them per project, in "projects": { "<id>": { … } }.
$ProjectConfigKeys = 'columnWidths', 'apps', 'frontend', 'stopFrontendOnStop'

# Per-machine config, written by install.ps1 and by the panel (the , settings, > < widths).
# LAZYRUNNER_CONFIG_PATH / LAZYRUNNER_STATE_DIR / LAZYRUNNER_SECRETS_PATH let tests run beside a real session without touching its files.
$ConfigPath = if ($env:LAZYRUNNER_CONFIG_PATH) { $env:LAZYRUNNER_CONFIG_PATH } else { Join-Path $PSScriptRoot 'lazyrunner.config.json' }

# Colour themes, one <id>.json per theme (add your own; t cycles through them).
$ThemesDir = if ($env:LAZYRUNNER_THEMES_DIR) { $env:LAZYRUNNER_THEMES_DIR } else { Join-Path $PSScriptRoot 'themes' }

# Runtime state: request/controller files, trusted repo profiles, lazyrunner.log, and a folder per project (<id>\) with its
# build logs and stamps, per-app logs and timings.
$StateDir = if ($env:LAZYRUNNER_STATE_DIR) { $env:LAZYRUNNER_STATE_DIR } else { Join-Path $env:LOCALAPPDATA 'lazyrunner' }
# ================================================================ end of settings

$ErrorActionPreference = 'Stop'

$LazyRunnerScriptPath  = $PSCommandPath
$RequestPath    = Join-Path $StateDir 'request.json'
$ControllerPath = Join-Path $StateDir 'controller.json'
$LogPath        = Join-Path $StateDir 'lazyrunner.log'
$TrustStorePath = Join-Path $StateDir 'trusted-profiles.json'
$ReopenPath     = Join-Path $StateDir 'reopen.json'
$ReopenExitCode = 75   # the UI asks the terminal's lazyrunner to open it again (another project, a reloaded profile)

$LoopDelayMs         = 100
$BranchSettleSec     = 3
$CodeSettleSec       = 2
$ProbeIntervalSec    = 5
$ProbeTimeoutSec     = 5
$SlowResponseMs      = 1000
$NotifyReadyAfterSec = 20
$MaxBuildErrorLines  = 6
$MaxLogEntries       = 200
$MaxPaneLines        = 5000
$MaxLinesPerTick     = 2000

New-Item -ItemType Directory -Force -Path $StateDir | Out-Null

foreach ($lib in 'apptypes', 'profiletools', 'common', 'update', 'process', 'build', 'secrets', 'metrics', 'themes', 'view', 'actions', 'input', 'checklist', 'panel') { . (Join-Path $PSScriptRoot "lib\$lib.ps1") }

# ---------------------------------------------------------------- entry point

switch ($Command) {
    '__ui' {
        try {
            $found = Find-ProjectProfile $Root $LazyRunnerRoot
            if (-not $found) { throw "lazyrunner: $Root is not a project (no profile matches it)." }
            $project = Resolve-ProjectProfile $found $LazyRunnerRoot
            if (-not (Test-ProfileTrusted $project $TrustStorePath)) { throw "lazyrunner: $($project.Path) isn't trusted yet - run lazyrunner in that repo to review it." }
            Use-ProjectProfile $project
        }
        catch { Write-Host $_.Exception.Message -ForegroundColor Red; exit 1 }
        try { Invoke-Ui -RepoRoot $project.Root -StartNow:$AutoStart; exit $Ctl.ExitCode }
        catch {
            Add-Content -Path $LogPath -Value "$(Get-Date -Format 'yyyy-MM-dd HH:mm:ss') ✗ lazyrunner crashed: $($_.Exception.Message)`n$($_.ScriptStackTrace)" -ErrorAction SilentlyContinue
            Write-Host "lazyrunner crashed: $($_.Exception.Message) (details in $LogPath)" -ForegroundColor Red
            exit 1
        }
    }
    'help' { Get-Help $PSCommandPath -Detailed; return }
    'update' { Invoke-UpdateCommand; return }
    'version' { Show-VersionCommand; return }
    'profile' {
        $output = @(Invoke-ProfileCommand $Action $Path -Json:$Json -Save:$Save)
        $output | Select-Object -SkipLast 1   # the JSON (check -Json, init); the last item is the exit code
        exit $output[-1]
    }
}

$controller = Get-Controller
if ($Command -eq 'stop') {
    if ($controller) { Send-Request 'stop' '' } else { Write-Host 'lazyrunner is not running.' -ForegroundColor DarkGray }
    return
}
$project = try { Resolve-ProjectProfile (Find-Project) $LazyRunnerRoot } catch { Write-Host $_.Exception.Message -ForegroundColor Red; exit 1 }
foreach ($warning in $ProfileWarnings) { Write-Host "Skipped a profile: $warning" -ForegroundColor Yellow }
if (-not (Confirm-ProfileTrust $project)) { exit 1 }
if ($controller) { Send-Request $Command $project.Root; return }

Assert-Prerequisites
Start-Ui $project.Root ($PSBoundParameters.ContainsKey('Command') -or (Get-Config).autoStartOnOpen)
