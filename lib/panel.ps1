# Control logic and main loop: actions (start/stop/restart with builds), branch watching, response probes,
# readiness, requests from `lazyrunner stop/restart` in other terminals, and terminal setup/teardown.
# Everything runs in this one process; the servers are its child processes (see process.ps1).

$script:Ctl = $null

function Add-Log([string]$Text, [string]$Color = 'Gray') {
    $time = Get-Date -Format 'HH:mm:ss'
    $Ctl.Log.Add([pscustomobject]@{ Time = $time; Text = $Text; Color = $Color })
    if ($Ctl.Log.Count -gt $MaxLogEntries) { $Ctl.Log.RemoveAt(0) }
    Add-Content -Path $LogPath -Value "$(Get-Date -Format 'yyyy-MM-dd') $time $Text" -ErrorAction SilentlyContinue
}

function Test-AppRunning([string]$Key) { $Ctl.Apps[$Key].State -eq 'running' }

function Get-Names([string[]]$Keys) { ($Keys | ForEach-Object { $Apps[$_].Name }) -join ' + ' }

function Update-Listening {
    $Ctl.Listening = @([Net.NetworkInformation.IPGlobalProperties]::GetIPGlobalProperties().GetActiveTcpListeners().Port)
    $Ctl.NextListen = (Get-Date).AddSeconds(1)
}

# Which variant each npm app runs on this checkout (so also its port).
function Update-AppModes {
    foreach ($key in Get-AppKeys 'Variants') {
        $Ctl.Modes[$key] = Get-AppMode $Ctl.Root $key
        $Ctl.Ports[$key] = $Ctl.Modes[$key].Port
    }
}

# A running app whose branch now needs another of its variants (e.g. a dev server instead of a watch build).
function Test-ModeChanged([string]$Key) {
    (Test-AppRunning $Key) -and (Test-AppTrait $Key 'Variants') -and $Ctl.Apps[$Key].Script -ne $Ctl.Modes[$Key].Script
}

function Update-DbLabel {
    try { $Ctl.Db = Get-ActiveDbLabel } catch { $Ctl.Db = 'secrets.json unreadable' }
}

# ---------------------------------------------------------------- port conflicts

function Find-PortConflicts([string[]]$Keys) {
    $conflicts = foreach ($key in $Keys) {
        $port = $Ctl.Ports[$key]
        if (-not $port) { continue }   # a watch build has no port
        $owner = Get-NetTCPConnection -LocalPort $port -State Listen -ErrorAction SilentlyContinue | Select-Object -First 1
        if (-not $owner) { continue }
        $process = Get-LiveProcess $owner.OwningProcess
        [pscustomobject]@{ Key = $key; Port = $port; Pid = $owner.OwningProcess; Name = if ($process) { $process.ProcessName } else { 'unknown' } }
    }
    $Ctl.Conflicts = @($conflicts)
    foreach ($conflict in $Ctl.Conflicts) {
        Add-Log "Port $($conflict.Port) is already used by $($conflict.Name) (PID $($conflict.Pid)) - press ! to kill it and start" 'Red'
    }
    $Ctl.Conflicts.Count -gt 0
}

function Resolve-PortConflicts {
    if (-not $Ctl.Conflicts) { return }
    foreach ($conflict in $Ctl.Conflicts) {
        if ($conflict.Pid -le 4) { Add-Log "Port $($conflict.Port) belongs to Windows (PID $($conflict.Pid)); free it manually." 'Red'; return }
        taskkill /PID $conflict.Pid /T /F 2>&1 | Out-Null
        Add-Log "Killed $($conflict.Name) (PID $($conflict.Pid)) to free port $($conflict.Port)" 'Yellow'
    }
    $Ctl.Conflicts = @()
    Start-Sleep -Milliseconds 500
    Invoke-Start
}

# ---------------------------------------------------------------- actions

# ---------------------------------------------------------------- startup timeline + restart on code change

function Get-TimelineEntry([string]$Key) {
    if (-not $Ctl.Timeline.ContainsKey($Key)) { $Ctl.Timeline[$Key] = @{ BuildFrom = $null; BuildTo = $null; StartFrom = $null; UpAt = $null } }
    $Ctl.Timeline[$Key]
}

# After "Ready": one line per app, █ building, ▒ starting, · up, over the whole start.
function Write-Timeline([string[]]$Keys, [double]$Total) {
    $width = 24
    foreach ($key in $Keys) {
        $entry = Get-TimelineEntry $key
        if ($null -eq $entry.UpAt -or $null -eq $entry.StartFrom) { continue }
        $cells = foreach ($i in 0..($width - 1)) {
            $at = ($i + 0.5) / $width * $Total
            if ($null -ne $entry.BuildTo -and $at -ge $entry.BuildFrom -and $at -lt $entry.BuildTo) { '█' }
            elseif ($at -ge $entry.StartFrom -and $at -lt $entry.UpAt) { '▒' }
            elseif ($at -ge $entry.UpAt) { '·' }
            else { ' ' }
        }
        $build = if ($null -ne $entry.BuildTo) { "build $(Format-Duration ($entry.BuildTo - $entry.BuildFrom)) · " } else { '' }
        Add-Log ("  {0,-9}▕{1}▏ {2}start {3}" -f $Apps[$key].Name, (-join $cells), $build, (Format-Duration ($entry.UpAt - $entry.StartFrom))) 'DarkCyan'
    }
}

# restartOnCodeChange: once saving under $SourceDir has been quiet for a moment, rebuild + restart the running apps
# that build (the others watch their own files).
function Test-CodeChange {
    $last = [LazyRunnerWatcher]::LastChange
    if ($last -le $Ctl.CodeHandled) { return }
    if (-not $Ctl.Config.restartOnCodeChange) { $Ctl.CodeHandled = $last; return }
    if (([datetime]::UtcNow.Ticks - $last) / 1e7 -lt $CodeSettleSec) { return }
    $Ctl.CodeHandled = $last
    $apis = @(Get-AppKeys 'Builds' | Where-Object { Test-AppRunning $_ })
    if ($apis) { Invoke-Restart "code changed: $([IO.Path]::GetFileName([LazyRunnerWatcher]::LastFile))" -Only $apis }
}

function Invoke-Start([string[]]$Only, [switch]$ForceBuild) {
    if ($Ctl.Building) { Start-DuringBuild $Only; return }
    Update-AppModes   # the branch decides how each npm app runs (e.g. a dev server or a watch build)
    Test-LockChanged
    $keys = @($Apps.Keys | Where-Object { (-not $Only -or $_ -in $Only) -and -not (Test-AppRunning $_) })
    foreach ($key in @($keys | Where-Object { (Test-AppTrait $_ 'Variants') -and -not $Ctl.Modes[$_] })) {
        Add-Log "$($Apps[$key].Name) skipped: $(Get-NoModeReason $Ctl.Root $key)" 'Yellow'
        $keys = @($keys | Where-Object { $_ -ne $key })
    }
    if ($Ctl.NpmCi -and $keys -contains $Ctl.NpmCi.Key) {   # its node_modules is being rewritten
        $Ctl.NpmCi.StartAfter = $true
        Add-Log "$($Apps[$Ctl.NpmCi.Key].Name) starts after npm ci" 'DarkGray'
        $keys = @($keys | Where-Object { $_ -ne $Ctl.NpmCi.Key })
        if (-not $keys) { return }
    }
    if (-not $keys) { Add-Log 'Already running.'; return }
    if (Find-PortConflicts $keys) { return }
    $Ctl.ActionStarted = [Diagnostics.Stopwatch]::StartNew()
    $Ctl.Timeline = @{}
    $Ctl.ReadyPending = @()
    $Ctl.LastBuildFailed = $false

    # Apps that need no build start right away and compile while the others build.
    $directKeys = @($keys | Where-Object { -not (Test-AppTrait $_ 'Builds') })
    foreach ($key in $directKeys) { Start-App $key }
    if ($directKeys) { Add-Log "Starting $(Get-Names $directKeys)" 'Cyan' }

    $apiKeys = @($keys | Where-Object { Test-AppTrait $_ 'Builds' })
    if ($apiKeys) {
        $Ctl.Busy = 'Checking for source changes…'
        Show-Frame
        $fingerprint = Get-BuildFingerprint $Ctl.Root
        $Ctl.Busy = ''
        # Builds run one at a time; s on an API takes it out of this queue (or cancels its build).
        $Ctl.BuildQueue = $apiKeys
        try {
            foreach ($key in $apiKeys) {
                if ($key -notin $Ctl.BuildQueue) { continue }
                $result = Build-App $key $fingerprint -Force:$ForceBuild
                if ($result -eq 'dropped') { $Ctl.BuildQueue = @($Ctl.BuildQueue | Where-Object { $_ -ne $key }) }
                if ($result -eq 'cancelled') { Invoke-Stop -Only $keys -Quiet; return }   # Stop/Quit stops the others too
                if ($result -eq 'failed') { Add-Log 'APIs not started - fix the build and press U or R.' 'Yellow'; return }
            }
            $apiKeys = @($Ctl.BuildQueue)
        }
        finally { $Ctl.BuildQueue = @() }
        foreach ($key in $apiKeys) { Start-App $key }
        if ($apiKeys) { Add-Log "Starting $(Get-Names $apiKeys)" 'Cyan' }
    }
    # Keeps what started during the build (u on an npm app, or after npm ci).
    $Ctl.ReadyPending = @(@($Ctl.ReadyPending) + @($keys | Where-Object { Test-AppRunning $_ }) | Select-Object -Unique)
    $Ctl.CodeHandled = [LazyRunnerWatcher]::LastChange   # what this build picked up is not a reason to restart again
}

# During a build (u, or npm ci finishing in the background) only apps that need no build can start: builds run one
# at a time.
function Start-DuringBuild([string[]]$Only) {
    foreach ($key in @($Apps.Keys | Where-Object { -not $Only -or $_ -in $Only })) {
        $name = $Apps[$key].Name
        if (Test-AppRunning $key) { Add-Log "$name is already running."; continue }
        if (-not (Test-AppTrait $key 'Builds')) { Start-WithoutBuild $key; continue }
        if ($key -eq $Ctl.Building.Key) { Add-Log "$name is building right now."; continue }
        if ($key -in $Ctl.BuildQueue) { Add-Log "$name builds and starts after the current build."; continue }
        Add-Log "$name can't build while another build runs (one at a time) - press u again afterwards" 'Yellow'
    }
}

function Start-WithoutBuild([string]$Key) {
    Update-AppModes
    $name = $Apps[$Key].Name
    if ((Test-AppTrait $Key 'Variants') -and -not $Ctl.Modes[$Key]) { Add-Log "$name skipped: $(Get-NoModeReason $Ctl.Root $Key)" 'Yellow'; return }
    if ($Ctl.NpmCi -and $Ctl.NpmCi.Key -eq $Key) { $Ctl.NpmCi.StartAfter = $true; Add-Log "$name starts after npm ci" 'DarkGray'; return }
    if (Find-PortConflicts @($Key)) { return }
    Start-App $Key
    Add-Log "Starting $name" 'Cyan'
    $Ctl.ReadyPending = @(@($Ctl.ReadyPending) + $Key | Select-Object -Unique)
}

# After a database switch: the APIs read secrets.json only when they start and run with --no-build, so stopping
# and starting the processes is enough. No git check, no build, and it also works while another app builds.
function Restart-ApiProcesses([string[]]$Keys, [string]$Reason) {
    Add-Log "Restarting $(Get-Names $Keys) ($Reason - no build needed)" 'Cyan'
    Invoke-Stop -Only $Keys -Quiet
    if (-not $Ctl.Building) { $Ctl.ActionStarted = [Diagnostics.Stopwatch]::StartNew(); $Ctl.Timeline = @{}; $Ctl.ReadyPending = @() }
    foreach ($key in $Keys) { Start-App $key }
    $Ctl.ReadyPending = @(@($Ctl.ReadyPending) + $Keys | Select-Object -Unique)
}

# s: stop the focused app - or cancel its build, take it out of the build queue, or cancel its npm ci.
function Invoke-StopFocused {
    $key = $Ctl.Focus
    $name = $Apps[$key].Name
    if ($Ctl.Building -and $Ctl.Building.Key -eq $key) { $Ctl.CancelRequest = 'skip'; return }   # the build loop picks it up
    if ($key -in $Ctl.BuildQueue) {
        $Ctl.BuildQueue = @($Ctl.BuildQueue | Where-Object { $_ -ne $key })
        Add-Log "$name won't be built or started this time" 'Yellow'
        return
    }
    if ($Ctl.NpmCi -and $Ctl.NpmCi.Key -eq $key) { Stop-NpmCi; return }
    Invoke-Stop -Only $key
}

function Invoke-Stop([string[]]$Only, [switch]$Quiet) {
    $keys = @($Apps.Keys | Where-Object { -not $Only -or $_ -in $Only })
    if (-not $Only) { $Ctl.Conflicts = @() }
    $Ctl.ReadyPending = @($Ctl.ReadyPending | Where-Object { $_ -notin $keys })
    foreach ($key in $keys) { $Ctl.Probes[$key] = New-Probe }   # results in flight belong to the old servers
    $stopped = @($keys | Where-Object { Stop-App $_ 'stopped' })
    Update-Listening
    if ($Quiet) { return }
    if ($stopped) { Add-Log "Stopped $(Get-Names $stopped)." } else { Add-Log 'Nothing was running.' }
}

# The apps Stop all / Restart all leave running (Test-InStopAll: the npm apps when stopFrontendOnStop is off). A
# restart still restarts one whose branch needs another variant (Test-ModeChanged).
function Get-KeptKeys([switch]$ForRestart) {
    $kept = @($Apps.Keys | Where-Object { -not (Test-InStopAll $_) })
    if ($ForRestart -and $kept) { Update-AppModes; $kept = @($kept | Where-Object { -not (Test-ModeChanged $_) }) }
    $kept
}

# S in the panel and `lazyrunner stop`.
function Invoke-StopAll {
    $kept = @(Get-KeptKeys)
    if ($Ctl.NpmCi -and $Ctl.NpmCi.Key -notin $kept) { Stop-NpmCi }   # stopping an app cancels its npm ci too
    if (-not $kept) { Invoke-Stop; return }
    $keys = @($Apps.Keys | Where-Object { $_ -notin $kept })
    if ($keys) { Invoke-Stop -Only $keys }
    $running = @($kept | Where-Object { Test-AppRunning $_ })
    if ($running) { Add-Log "$(Get-Names $running) kept running (, then s: make Stop all include it)." 'DarkGray' }
    elseif (-not $keys) { Add-Log 'Nothing was running.' }
}

function Invoke-Restart([string]$Reason, [string[]]$Only, [switch]$ForceBuild) {
    $kept = if ($Only) { @() } else { @(Get-KeptKeys -ForRestart) }
    $stopKeys = if ($Only) { $Only } elseif ($kept) { @($Apps.Keys | Where-Object { $_ -notin $kept }) } else { $null }
    Add-Log "Restarting$(if ($stopKeys) { " $(Get-Names $stopKeys)" }) ($Reason)" 'Cyan'
    if ($stopKeys -or -not $kept) { Invoke-Stop -Only $stopKeys -Quiet }   # no keys and nothing kept: all of them
    Invoke-Start -Only $Only -ForceBuild:$ForceBuild   # also starts anything that was not running
}

function Set-Root([string]$RepoRoot) {
    $Ctl.Root = $RepoRoot
    $Ctl.GitDir = Get-GitDir $RepoRoot
    $Ctl.Branch = Get-Branch $Ctl.GitDir
    $Ctl.Pending = $null
    foreach ($key in $Apps.Keys) { $Ctl.Ports[$key] = Get-AppPort $RepoRoot $key }
    Update-AppModes
    Update-DbLabel
    [LazyRunnerWatcher]::Watch((Join-Path $RepoRoot $SourceDir))
    $Ctl.CodeHandled = [LazyRunnerWatcher]::LastChange
}

function Update-Branch {
    if (-not $Ctl.GitDir) { return }
    $branch = Get-Branch $Ctl.GitDir
    if (-not $branch -or $branch -eq $Ctl.Branch) { $Ctl.Pending = $null; return }
    if ($branch -ne $Ctl.Pending) { $Ctl.Pending = $branch; $Ctl.PendingSince = Get-Date; return }
    # Wait until the checkout has settled before rebuilding.
    $settling = ((Get-Date) - $Ctl.PendingSince).TotalSeconds -lt $BranchSettleSec
    if ($settling -or (Test-Path (Join-Path $Ctl.GitDir 'index.lock'))) { return }

    $previous = $Ctl.Branch
    $Ctl.Branch = $branch
    $Ctl.Pending = $null
    Test-ProfileChange
    Update-AppModes
    Test-LockChanged
    $running = @($Apps.Keys | Where-Object { Test-AppRunning $_ })
    if (-not $Ctl.Config.autoRestartOnBranchChange) { Add-Log "Branch changed $previous → $branch (auto-restart is off, press R)" 'Yellow'; return }
    if (-not $running) { Add-Log "Branch changed $previous → $branch (nothing running, not restarting)" 'Yellow'; return }
    # Only the running apps that build restart: npm run's own watcher picks up the others' changes. Those restart
    # only when the branch needs another variant (Test-ModeChanged).
    $keys = @($running | Where-Object { -not (Test-AppTrait $_ 'WatchesOwnFiles') -or (Test-ModeChanged $_) })
    foreach ($key in @($running | Where-Object { $_ -notin $keys })) {
        Add-Log "$($Apps[$key].Name) keeps running ($($Ctl.Modes[$key].Script) watches the files itself)" 'DarkGray'
    }
    if ($keys) { Invoke-Restart "branch changed $previous → $branch" -Only $keys }
}

# ---------------------------------------------------------------- the project's profile

# Whether a checkout runs with the same profile as this one (another worktree, say): then lazyrunner just moves there.
function Test-SameProject([string]$RepoRoot) {
    $found = Find-ProjectProfile $RepoRoot $LazyRunnerRoot
    if (-not $found) { return $false }
    try { $other = Resolve-ProjectProfile $found $LazyRunnerRoot } catch { return $false }
    $other.Id -eq $ProjectProfile.Id -and $other.Hash -eq $ProjectProfile.Hash -and $other.Source -eq $ProjectProfile.Source
}

# Another project, or this one's profile reloaded: the UI exits with $ReopenExitCode and the terminal's lazyrunner opens it
# again (Start-Ui), so the apps, settings and state of that profile load from scratch.
function Request-Reopen([string]$RepoRoot, [switch]$Start, [string]$Reason) {
    Add-Log "Reopening lazyrunner for $RepoRoot ($Reason)…" 'Cyan'
    Write-Json $ReopenPath @{ root = $RepoRoot; start = [bool]$Start }
    $Ctl.ExitCode = $ReopenExitCode
    $Ctl.Quit = $true
}

# , then p: reopen with the profile this checkout has now, starting what was running.
function Invoke-ReloadProfile {
    Request-Reopen $Ctl.Root -Start:([bool]@($Apps.Keys | Where-Object { Test-AppRunning $_ })) 'reloading the project profile'
}

# A branch can change the repo's .lazyrunner.json, or add or remove one: say what changed (lazyrunner keeps running with the
# profile it has; , then p reloads).
function Test-ProfileChange {
    $found = Find-ProjectProfile $Ctl.Root $LazyRunnerRoot
    try { $now = if ($found) { Resolve-ProjectProfile $found $LazyRunnerRoot } else { $null } }
    catch { Add-Log "✗ $($_.Exception.Message -replace "`n\s*", ' ')" 'Red'; $Ctl.ProfileChanged = $true; return }
    if ($now -and $now.Path -eq $ProjectProfile.Path -and $now.Hash -eq $ProjectProfile.Hash) { return }
    $Ctl.ProfileChanged = $true
    if (-not $now) { Add-Log "No profile matches this checkout any more - lazyrunner keeps $($ProjectProfile.Name)'s until you reopen it" 'Yellow'; return }
    $was = @($ProjectProfile.AppKeys)
    $changes = @(
        @($now.Apps.Keys | Where-Object { $_ -notin $was }) | ForEach-Object { "+ $($now.Apps[$_].Name)" }
        @($was | Where-Object { -not $now.Apps.Contains($_) }) | ForEach-Object { "- $($_)" }
        @($was | Where-Object { $now.Apps.Contains($_) -and $Apps.Contains($_) -and (ConvertTo-SortedJson $now.Apps[$_]) -ne (ConvertTo-SortedJson $Apps[$_]) }) | ForEach-Object { "~ $($now.Apps[$_].Name)" }
    )
    $what = if ($changes) { $changes -join ', ' } else { 'its settings' }
    $trust = if (Test-ProfileTrusted $now $TrustStorePath) { '' } else { ' - it needs your trust first' }
    Add-Log "This branch changes the project profile ($(Split-Path -Leaf $now.Path): $what): , then p reopens lazyrunner with it$trust" 'Yellow'
}

function Read-Request {
    if (-not (Test-Path $RequestPath)) { return }
    $request = Read-Json $RequestPath
    Remove-Item $RequestPath -ErrorAction SilentlyContinue
    if (-not $request) { return }
    if ($request.action -in 'start', 'restart' -and $request.root -and $request.root -ne $Ctl.Root) {
        if (-not (Test-SameProject $request.root)) { Request-Reopen $request.root -Start 'another project'; return }
        Add-Log "Switching to $($request.root)" 'Cyan'
        Invoke-Stop -Quiet
        Set-Root $request.root
        Invoke-Start
        return
    }
    switch ($request.action) {
        'start'   { Invoke-Start }
        'stop'    { Invoke-StopAll }
        'restart' { Invoke-Restart 'requested from the command line' }
        'quit'    { $Ctl.Quit = $true }
        'keys'    { foreach ($name in @($request.keys)) { $null = Invoke-UiKey (ConvertTo-KeyInfo $name) } }
    }
}

# ---------------------------------------------------------------- monitoring

# Times each request inside the task itself, so the panel being busy (e.g. building) can't inflate the result.
$ProbeHelperSource = @'
using System.Diagnostics;
using System.Net.Http;
using System.Threading.Tasks;

public sealed class LazyRunnerProbeResult
{
    public bool Ok { get; init; }
    public int Code { get; init; }
    public long Ms { get; init; }
}

public static class LazyRunnerProbe
{
    public static async Task<LazyRunnerProbeResult> GetAsync(HttpClient http, string url)
    {
        var timer = Stopwatch.StartNew();
        try
        {
            using var response = await http.GetAsync(url, HttpCompletionOption.ResponseHeadersRead).ConfigureAwait(false);
            return new LazyRunnerProbeResult { Ok = true, Code = (int)response.StatusCode, Ms = timer.ElapsedMilliseconds };
        }
        catch (System.Exception)
        {
            return new LazyRunnerProbeResult { Ok = false, Code = 0, Ms = timer.ElapsedMilliseconds };
        }
    }
}
'@

function Install-ProbeHelper {
    if (-not ('LazyRunnerProbe' -as [type])) {
        Add-Type -TypeDefinition $ProbeHelperSource -ReferencedAssemblies System.Net.Http, System.Net.Primitives, System.Runtime
    }
}

function New-Probe { @{ Task = $null; Last = $null; NextAt = [datetime]::MinValue } }

function Write-ProbeChange([string]$Key, $Previous, $Current) {
    # Log only transitions, so lazyrunner.log records when an app hung and when it came back.
    $name = $Apps[$Key].Name
    if ($Current.Ok -and -not $Previous) { Add-Log "● $name responding ($($Current.Ms) ms)" 'Green'; return }
    if ($Previous.Ok -eq $Current.Ok) { return }
    if ($Current.Ok) { Add-Log "● $name responding again ($($Current.Ms) ms)" 'Green' }
    else { Add-Log "✗ $name is not responding (no answer within ${ProbeTimeoutSec}s)" 'Red' }
}

function Update-Probes {
    foreach ($key in $Apps.Keys) {
        $probe = $Ctl.Probes[$key]
        if ($probe.Task) {
            if (-not $probe.Task.IsCompleted) { continue }
            $result = $probe.Task.Result
            $Ctl.Probes[$key] = @{ Task = $null; Last = $result; NextAt = (Get-Date).AddSeconds($ProbeIntervalSec) }
            Add-History $Ctl.Apps[$key].RespHistory $(if ($result.Ok) { $result.Ms } else { $ProbeTimeoutSec * 1000 })
            Write-ProbeChange $key $probe.Last $result
            continue
        }
        $port = $Ctl.Ports[$key]
        if (-not ((Test-AppRunning $key) -and ($Ctl.Listening -contains $port))) { $Ctl.Probes[$key] = New-Probe; continue }
        if ((Get-Date) -lt $probe.NextAt) { continue }
        $path = if ($Apps[$key].Swagger) { "$($Apps[$key].Swagger)/index.html" } else { '/' }
        $task = [LazyRunnerProbe]::GetAsync($Ctl.Http, "http://localhost:$port$path")
        $Ctl.Probes[$key] = @{ Task = $task; Last = $probe.Last; NextAt = $probe.NextAt }
    }
}

# Up = listening on its port; a watch build has no port, so it is up once it has written its ready file.
function Test-AppReady([string]$Key) {
    if ($Ctl.Ports[$Key]) { return $Ctl.Listening -contains $Ctl.Ports[$Key] }
    $mode = $Ctl.Modes[$Key]
    if (-not $mode.ReadyFile) { return $false }
    $output = Get-Item -LiteralPath (Join-Path $Ctl.Root (Join-Path $Apps[$Key].Dir $mode.ReadyFile)) -ErrorAction SilentlyContinue
    $output -and $output.LastWriteTimeUtc -gt $Ctl.Apps[$Key].StartedAt
}

function Watch-AppStates {
    foreach ($key in $Apps.Keys) {
        $state = $Ctl.Apps[$key]
        $name = $Apps[$key].Name
        if ($state.Proc -and $state.State -eq 'running' -and $state.Proc.HasExited) {
            Receive-AppOutput $state
            $state.ExitCode = $state.Proc.ExitCode
            $state.Proc = $null
            $state.State = 'exited'
            Add-PaneLine $state "■ exited with code $($state.ExitCode)" 'Red'
            $state.FlashUntil = [datetime]::UtcNow.AddSeconds(3)
            Add-Log "✗ $name exited unexpectedly (code $($state.ExitCode)) - see its column" 'Red'
            Send-Notification "$name stopped unexpectedly" "Exit code $($state.ExitCode)"
            $Ctl.ReadyPending = @()
        }
        if ($state.State -eq 'running' -and $Ctl.UpGen[$key] -ne $state.Gen -and (Test-AppReady $key)) {
            $Ctl.UpGen[$key] = $state.Gen
            $Ctl.UpAfter[$key] = Get-SecondsSince $state.StartedAt
            if ($Ctl.ActionStarted) { (Get-TimelineEntry $key).UpAt = $Ctl.ActionStarted.Elapsed.TotalSeconds }
            Add-Timing 'start' $key $Ctl.UpAfter[$key]
            $what = if ($Ctl.Ports[$key]) { "is listening on :$($Ctl.Ports[$key])" } else { "is built (served by $($Apps[$Ctl.Modes[$key].ServedBy].Name) at $(Get-AppUrl $key))" }
            Add-Log "● $name $what - up in $(Format-Duration $Ctl.UpAfter[$key])" 'Green'
        }
    }
    # Ready once every app this start launched is up in its current generation (not the previous run) - and not
    # while apps are still building (an npm app can be up long before them).
    if ($Ctl.Building -or $Ctl.BuildQueue.Count) { return }
    $pending = @($Ctl.ReadyPending)
    if (-not $pending -or @($pending | Where-Object { $Ctl.UpGen[$_] -ne $Ctl.Apps[$_].Gen }).Count) { return }
    $seconds = $Ctl.ActionStarted.Elapsed.TotalSeconds
    $Ctl.ReadyPending = @()
    Add-Log "Ready in $(Format-Duration $seconds)" 'Green'
    Write-Timeline $pending $seconds
    $Ctl.CelebrateText = "ready in $(Format-Duration $seconds)"
    $Ctl.CelebrateUntil = [datetime]::UtcNow.AddSeconds(4)
    if ($seconds -ge $NotifyReadyAfterSec) { Send-Notification "$($ProjectProfile.Name) is ready" "$(Get-Names $pending) up in $(Format-Duration $seconds)" }
}

# One tick while a build blocks the main loop: output keeps flowing, npm ci keeps going, and the keys that can't
# conflict with the build still work. Returns 'stop' or 'quit' (cancel the start) or 'skip' (X: cancel this build).
function Invoke-BusyTick {
    if ((Get-Date) -ge $Ctl.NextListen) { Update-Listening }
    Receive-Output
    Update-NpmCi
    Update-Metrics
    Update-Probes
    Watch-AppStates
    $cancel = $null
    while ([Console]::KeyAvailable) {
        $result = Invoke-UiKey ([Console]::ReadKey($true)) -Busy
        if ($result) { $cancel = $result }
    }
    $request = Read-Json $RequestPath
    if ($request -and $request.action -in 'stop', 'quit', 'keys') {
        Remove-Item $RequestPath -ErrorAction SilentlyContinue
        if ($request.action -eq 'keys') {   # scripted tests
            foreach ($name in @($request.keys)) { $result = Invoke-UiKey (ConvertTo-KeyInfo $name) -Busy; if ($result) { $cancel = $result } }
        }
        else { $cancel = $request.action }
    }
    if (-not $cancel -and $Ctl.CancelRequest) { $cancel = $Ctl.CancelRequest }
    $Ctl.CancelRequest = $null
    if ($cancel -eq 'quit' -or ($cancel -eq 'stop' -and $Ctl.NpmCi -and (Test-InStopAll $Ctl.NpmCi.Key))) { Stop-NpmCi }
    Show-Frame
    $cancel
}

# ---------------------------------------------------------------- terminal + main loop

function Initialize-Terminal {
    $Ctl.Term = @{ Encoding = [Console]::OutputEncoding; CtrlC = [Console]::TreatControlCAsInput; Title = $Host.UI.RawUI.WindowTitle }
    [LazyRunnerConsole]::EnableVirtualTerminal()
    [Console]::OutputEncoding = [Text.UTF8Encoding]::new($false)
    [Console]::TreatControlCAsInput = $true   # Ctrl+C is a key here (it quits), not a signal
    # Alternate screen (your scrollback is untouched), hidden cursor, no auto-wrap at the last column.
    [Console]::Out.Write("`e[?1049h`e[?25l`e[?7l`e[2J")
    Set-TerminalColors $Ctl.Theme.Terminal
}

function Restore-Terminal {
    Set-TerminalColors $null
    [Console]::Out.Write("`e]9;4;0;0`e\`e[?7h`e[?25h`e[?1049l")
    [Console]::Out.Flush()
    [Console]::TreatControlCAsInput = $Ctl.Term.CtrlC
    [Console]::OutputEncoding = $Ctl.Term.Encoding
    $Host.UI.RawUI.WindowTitle = $Ctl.Term.Title
}

function Invoke-Ui([string]$RepoRoot, [switch]$StartNow) {
    Install-ProcessHelper
    Install-ProbeHelper
    Initialize-ProjectState
    $script:Ctl = @{
        Config = Get-Config; Quit = $false; Frame = 0
        Root = $null; GitDir = $null; Branch = ''; Pending = $null; PendingSince = [datetime]::MinValue
        Ports = @{}; Listening = @(); NextListen = [datetime]::MinValue; Apps = [ordered]@{}
        UpGen = @{}; UpAfter = @{}; Probes = @{}; Conflicts = @(); ReadyPending = @(); ActionStarted = $null
        Busy = ''; Building = $null; NpmCi = $null; BuildQueue = @(); CancelRequest = $null; LastFailedLog = $null; LastBuildFailed = $false; Modes = @{}; Db = $null
        LockChanged = @(); LockWarned = @{}; Overlay = $null; CelebrateUntil = $null; CelebrateText = ''
        NextMetrics = [datetime]::MinValue; NextTree = [datetime]::MinValue; Timeline = @{}; CodeHandled = 0L; Bars = @{}
        ThemeId = 'default'; ThemeName = 'default'; Theme = $null; StyleCache = @{}
        Search = $null; PendingG = $null; BodyRows = 20
        Focus = @($Apps.Keys)[0]; Zoom = $false; Picker = $null; Summary = @{ Text = ''; Color = 'Gray' }; LastOsc = ''
        Timings = (Read-Json (Get-TimingsPath)) ?? @{}; ExitCode = 0; ProfileChanged = $false
        Version = (Get-LazyRunnerVersion); StartNow = [bool]$StartNow; WhatsNew = @(); UpdatedTo = $null
        Http = [Net.Http.HttpClient]@{ Timeout = [timespan]::FromSeconds($ProbeTimeoutSec) }
        Log = [Collections.Generic.List[object]]::new(); PrevRows = @(); LastSize = ''; LastDump = [datetime]::MinValue
    }
    foreach ($key in $Apps.Keys) { $Ctl.Apps[$key] = New-AppState $key; $Ctl.Probes[$key] = New-Probe }
    foreach ($warning in Import-Themes) { Add-Log $warning 'Yellow' }
    Set-Theme $Ctl.Config.theme
    Write-Json $ControllerPath @{ pid = $PID; startTicks = (Get-Process -Id $PID).StartTime.Ticks }
    Remove-Item $RequestPath -ErrorAction SilentlyContinue
    Initialize-Terminal
    try {
        Open-PaneLogs
        Set-Root $RepoRoot
        Show-Frame
        $Ctl.WhatsNew = @(Get-WhatsNew)
        if ($Ctl.Config.startupChecklist) { Invoke-Checklist }   # it checks for an update first
        else { Write-WhatsNewLog $Ctl.WhatsNew; Invoke-QuietUpdate }
        if ($Ctl.Quit) { return }   # quit, or reopening on an update
        if ($StartNow) { Invoke-Start }
        else { Add-Log 'Ready - press U to start everything (auto-start when lazyrunner opens is off: , then u turns it on).' 'Cyan' }
        $nextSlowTick = [datetime]::MinValue
        while (-not $Ctl.Quit) {
            Read-UiKeys
            if ($Ctl.Quit) { break }
            Read-Request
            if ((Get-Date) -ge $nextSlowTick) {
                Update-Listening
                Update-Branch
                Update-DbLabel
                Test-CodeChange
                $nextSlowTick = (Get-Date).AddSeconds(1)
            }
            Receive-Output
            Update-NpmCi
            Update-Metrics
            Update-Probes
            Watch-AppStates
            Show-Frame
            Start-Sleep -Milliseconds $LoopDelayMs
        }
    }
    finally {
        Stop-NpmCi
        Invoke-Stop -Quiet   # the job object also kills everything if this process dies abruptly
        [LazyRunnerWatcher]::Stop()
        Close-PaneLogs
        Remove-Item $ControllerPath -ErrorAction SilentlyContinue
        $Ctl.Http.Dispose()
        Restore-Terminal
    }
}
