# Builds: `dotnet build` with its output shown in the app's column, cancellable with Stop/Quit, and skipped when
# nothing under $SourceDir changed since the last successful build. Also keeps build/start timings so the
# progress bars can show an estimate.

function Get-StampPath([string]$Key) { Join-Path $ProjectStateDir "build-$Key.stamp.json" }

function Get-BuildFingerprint([string]$RepoRoot) {
    # Tree hash of the committed sources + every uncommitted change (with its timestamp) under $SourceDir.
    $tree = git -C $RepoRoot rev-parse "HEAD:$($SourceDir -replace '\\', '/')" 2>$null
    if ($LASTEXITCODE -ne 0) { return $null }
    $parts = [Collections.Generic.List[string]]::new()
    $parts.Add($tree)
    $changes = git -C $RepoRoot status --porcelain=v1 -uall -- $SourceDir 2>$null
    # Build output never counts as a source change, even in a checkout that forgot to ignore it.
    foreach ($line in ($changes | Where-Object { $_ -notmatch '[\\/](bin|obj)[\\/]' })) {
        $path = ($line.Substring(3) -split ' -> ')[-1].Trim('"')
        $item = Get-Item -LiteralPath (Join-Path $RepoRoot $path) -Force -ErrorAction SilentlyContinue
        $parts.Add("$line|$(if ($item) { $item.LastWriteTimeUtc.Ticks } else { 0 })")
    }
    [Convert]::ToHexString([Security.Cryptography.SHA256]::HashData([Text.Encoding]::UTF8.GetBytes($parts -join "`n")))
}

function Get-BuildOutputTicks([string]$Key) {
    $project = Join-Path $Ctl.Root $Apps[$Key].Project
    $assembly = "$([IO.Path]::GetFileNameWithoutExtension($project)).dll"
    $output = Get-ChildItem (Join-Path (Split-Path $project) 'bin\Debug') -Filter $assembly -Recurse -Depth 2 -ErrorAction SilentlyContinue |
        Sort-Object LastWriteTimeUtc -Descending | Select-Object -First 1
    if ($output) { $output.LastWriteTimeUtc.Ticks } else { 0 }
}

function Test-BuildUpToDate([string]$Key, [string]$Fingerprint) {
    if (-not $Fingerprint) { return $false }
    $stamp = Read-Json (Get-StampPath $Key)
    $stamp -and $stamp.root -eq $Ctl.Root -and $stamp.fingerprint -eq $Fingerprint -and
        $stamp.outputTicks -ne 0 -and $stamp.outputTicks -eq (Get-BuildOutputTicks $Key)
}

function Save-BuildStamp([string]$Key, [string]$Fingerprint) {
    if (-not $Fingerprint) { return }
    Write-Json (Get-StampPath $Key) @{ root = $Ctl.Root; fingerprint = $Fingerprint; outputTicks = Get-BuildOutputTicks $Key }
}

# ---------------------------------------------------------------- timings (for progress estimates)

function Get-TimingsPath { Join-Path $ProjectStateDir 'timings.json' }

function Add-Timing([string]$Kind, [string]$Key, [double]$Seconds) {
    if (-not $Ctl.Timings.ContainsKey($Kind)) { $Ctl.Timings[$Kind] = @{} }
    $recent = @(@($Ctl.Timings[$Kind][$Key]) + [Math]::Round($Seconds, 1) | Where-Object { $null -ne $_ } | Select-Object -Last 5)
    $Ctl.Timings[$Kind][$Key] = $recent
    Write-Json (Get-TimingsPath) $Ctl.Timings
}

function Get-TimingEstimate([string]$Kind, [string]$Key) {
    $table = $Ctl.Timings[$Kind]
    if (-not $table) { return $null }
    $values = @($table[$Key] | Where-Object { $null -ne $_ } | Sort-Object)
    if (-not $values) { return $null }
    $values[[int][Math]::Floor($values.Count / 2)]   # median of the last runs
}

# ---------------------------------------------------------------- build

function Format-BuildError([string]$Line) {
    ($Line -replace '^\s*.*\\(?=[^\\]+\(\d+)', '' -replace '\s*\[[^\]]+\]\s*$', '').Trim()
}

# Projects a build will compile: the project plus everything it references, directly or not.
function Get-ProjectCount([string]$Project) {
    $seen = [Collections.Generic.HashSet[string]]::new([StringComparer]::OrdinalIgnoreCase)
    $queue = [Collections.Generic.Queue[string]]::new()
    $queue.Enqueue([IO.Path]::GetFullPath($Project))
    while ($queue.Count) {
        $path = $queue.Dequeue()
        if (-not (Test-Path $path) -or -not $seen.Add($path)) { continue }
        foreach ($match in [regex]::Matches([IO.File]::ReadAllText($path), '<ProjectReference\s+Include="([^"$]+)"')) {
            $queue.Enqueue([IO.Path]::GetFullPath((Join-Path (Split-Path $path) $match.Groups[1].Value)))
        }
    }
    $seen.Count
}

# Build/install output into the app's column. MSBuild's "Project -> output.dll" lines tick the progress;
# warnings are counted, not shown (a big solution can have hundreds).
function Receive-ToolOutput($Process, $State, $Lines, $Progress) {
    $line = $null
    while ($Process.Lines.TryDequeue([ref]$line)) {
        $Lines.Add($line)
        $text = $AnsiPattern.Replace($line, '')
        if ($text -match ': warning |^npm warn') { $Progress.Warnings++; if ($Progress.Kind -eq 'build') { continue } }
        if ($Progress.Kind -eq 'build' -and $text -match '^\s*([^\s:]+) -> ') {
            $Progress.Done++
            Add-PaneLine $State "  ✓ $($Matches[1])" 'DarkGreen'
            continue
        }
        $color = if ($text -match ': error |^npm (ERR!|error)') { 'Red' } elseif ($text -match '^npm warn') { 'Yellow' } else { 'DarkGray' }
        Add-PaneLine $State (Format-BuildError $text) $color
    }
}

# Waits for a dotnet build with its output in the app's column, keeping the panel live: Stop/Quit cancel the
# whole start, s on the app being built ('skip') cancels just this build. Total > 0 shows real progress.
# Returns 'ok', 'failed', 'cancelled' or 'dropped' (s).
function Wait-Tool($Process, $Progress, $Lines) {
    $state = $Ctl.Apps[$Progress.Key]
    $Ctl.Building = $Progress
    $Ctl.CancelRequest = $null   # an s meant for a build that has since finished
    try {
        while (-not $Process.HasExited) {
            Receive-ToolOutput $Process $state $Lines $Progress
            $cancel = Invoke-BusyTick
            if ($cancel) {
                taskkill /PID $Process.Id /T /F 2>&1 | Out-Null
                $null = $Process.WaitForExit(10000)
                if ($cancel -eq 'quit') { $Ctl.Quit = $true }
                return $(if ($cancel -eq 'skip') { 'dropped' } else { 'cancelled' })
            }
            Start-Sleep -Milliseconds $LoopDelayMs
        }
        # MSBuild's reusable worker nodes can keep the output pipe open, so don't wait for end-of-stream.
        $null = $Process.WaitForExit(3000)
        Start-Sleep -Milliseconds 200
        Receive-ToolOutput $Process $state $Lines $Progress
    }
    finally { $Ctl.Building = $null }
    if ($Process.ExitCode -eq 0) { 'ok' } else { 'failed' }
}

function Write-BuildFailure([string]$Key, $Lines, [int]$Seconds, [string]$Log) {
    $errors = @($Lines | Where-Object { $_ -match ': error ' } | Select-Object -Unique)
    $Ctl.LastFailedLog = $Log
    $Ctl.LastBuildFailed = $true
    Add-Log "✗ $($Apps[$Key].Name) build failed after ${Seconds}s ($($errors.Count) errors) - o opens the full log" 'Red'
    $errors | Select-Object -First $MaxBuildErrorLines | ForEach-Object { Add-Log "    $(Format-BuildError $_)" 'Red' }
    Send-Notification "$($Apps[$Key].Name) build failed" "$($errors.Count) errors - see lazyrunner"
}

# Returns 'ok', 'skipped', 'failed' or 'cancelled'.
function Build-App([string]$Key, [string]$Fingerprint, [switch]$Force) {
    $app = $Apps[$Key]
    $state = $Ctl.Apps[$Key]
    if (-not $Force -and $Ctl.Config.skipUnchangedBuilds -and (Test-BuildUpToDate $Key $Fingerprint)) {
        Add-Log "✓ $($app.Name) unchanged since its last build - skipped (b / B force a rebuild)" 'Green'
        return 'skipped'
    }
    $command = Get-BuildCommand $Key $Ctl.Root
    $project = $command.Project
    $log = Join-Path $ProjectStateDir "build-$Key.log"
    Add-PaneLine $state "──── ⚙ $(Get-Date -Format 'HH:mm:ss') · $($command.File) build $([IO.Path]::GetFileName($project)) ────" 'Cyan'
    $lines = [Collections.Generic.List[string]]::new()
    $timer = [Diagnostics.Stopwatch]::StartNew()
    $progress = @{ Key = $Key; Kind = 'build'; Timer = $timer; Done = 0; Total = (Get-ProjectCount $project); Warnings = 0 }
    $build = [LazyRunnerProcess]::Start($command.File, [string[]]$command.Args, $command.Dir)
    $timeline = if ($Ctl.ActionStarted) { Get-TimelineEntry $Key } else { $null }
    if ($timeline) { $timeline.BuildFrom = $Ctl.ActionStarted.Elapsed.TotalSeconds - $timer.Elapsed.TotalSeconds }
    $result = Wait-Tool $build $progress $lines
    if ($timeline) { $timeline.BuildTo = $Ctl.ActionStarted.Elapsed.TotalSeconds }
    [IO.File]::WriteAllLines($log, $lines)
    $seconds = [int]$timer.Elapsed.TotalSeconds
    if ($result -in 'cancelled', 'dropped') {
        Add-PaneLine $state '■ build cancelled' 'Yellow'
        Add-Log "■ $($app.Name) build cancelled$(if ($result -eq 'dropped') { ' (s) - it stays stopped' })" 'Yellow'
        return $result
    }
    if ($result -eq 'failed') {
        Add-PaneLine $state "✗ build failed after ${seconds}s" 'Red'
        Write-BuildFailure $Key $lines $seconds $log
        return 'failed'
    }
    Save-BuildStamp $Key $Fingerprint
    Add-Timing 'build' $Key $timer.Elapsed.TotalSeconds
    $warnings = if ($progress.Warnings) { ", $($progress.Warnings) warnings" } else { '' }
    Add-PaneLine $state "✓ built in ${seconds}s ($($progress.Done) projects$warnings)" 'Green'
    Add-Log "✓ $($app.Name) built in ${seconds}s ($($progress.Done) projects$warnings)" 'Green'
    'ok'
}

# ---------------------------------------------------------------- npm ci (the packages of the apps that Install)

# One stamp per app: the package-lock.json its last npm ci from lazyrunner installed.
function Get-NpmCiStampPath([string]$Key) { Join-Path $ProjectStateDir "npm-ci.$Key.stamp.json" }

function Get-NpmCiStamp([string]$Key) { Read-Json (Get-NpmCiStampPath $Key) }

function Get-LockHash([string]$RepoRoot, [string]$Key) {
    $lock = Join-Path $RepoRoot (Join-Path $Apps[$Key].Dir 'package-lock.json')
    if (-not (Test-Path $lock)) { return $null }
    $text = [IO.File]::ReadAllText($lock).Replace("`r`n", "`n")   # line-ending conversions (autocrlf) are not changes
    [Convert]::ToHexString([Security.Cryptography.SHA256]::HashData([Text.Encoding]::UTF8.GetBytes($text)))
}

# $Ctl.LockChanged: the apps whose package-lock.json differs from the one their last `npm ci` (run from lazyrunner)
# installed. Warns once per change.
function Test-LockChanged {
    $Ctl.LockChanged = @(foreach ($key in Get-AppKeys 'Installs') {
            $stamp = Get-NpmCiStamp $key
            $hash = Get-LockHash $Ctl.Root $key
            if (-not ($stamp -and $hash -and $stamp.root -eq $Ctl.Root -and $stamp.lockHash -ne $hash)) { continue }
            $key
            if ($Ctl.LockWarned[$key] -eq $hash) { continue }
            $Ctl.LockWarned[$key] = $hash
            Add-Log "$($Apps[$key].Dir)\package-lock.json changed since the last npm ci - press i to reinstall its packages" 'Yellow'
        })
}

# Workspace tools can leave a daemon running from node_modules (Nx does), which keeps files locked while npm ci
# replaces them; stop it first when there is one.
function Stop-NxDaemon([string]$Dir) {
    $nx = Join-Path $Dir 'node_modules\nx\bin\nx.js'
    $node = Get-Command node.exe -ErrorAction SilentlyContinue
    if (-not (Test-Path $nx) -or -not $node) { return }
    $process = [LazyRunnerProcess]::Start($node.Source, [string[]]@($nx, 'daemon', '--stop'), $Dir)
    $null = $process.WaitForExit(20000)
}

# Which app i installs: one whose lock file changed (the focused one if it is among them), else the focused one,
# else one whose node_modules is missing, else the first.
function Select-NpmCiKey {
    $keys = @(Get-AppKeys 'Installs')
    if ($Ctl.LockChanged) { if ($Ctl.Focus -in $Ctl.LockChanged) { return $Ctl.Focus } else { return @($Ctl.LockChanged)[0] } }
    if ($Ctl.Focus -in $keys) { return $Ctl.Focus }
    $missing = @($keys | Where-Object { -not (Test-Path (Join-Path $Ctl.Root (Join-Path $Apps[$_].Dir 'node_modules'))) })
    if ($missing) { return $missing[0] }
    if ($keys) { $keys[0] }
}

# npm ci runs in the background ($Ctl.NpmCi, one at a time), next to a build if there is one: every tick (main loop
# and build loop) calls Update-NpmCi. Its app can't run meanwhile (node_modules is half-written), so starting it
# only sets StartAfter, and it starts once npm ci succeeds.
function Invoke-NpmCi([string]$Key) {
    if (-not $Key) { $Key = Select-NpmCiKey }
    if (-not $Key) { Add-Log 'There is no npm app (the project has none, or lazyrunner.config.json turns them off).' 'Yellow'; return }
    if ($Ctl.NpmCi) { Add-Log "npm ci is already running ($($Apps[$Ctl.NpmCi.Key].Name))."; return }
    $name = $Apps[$Key].Name
    $dir = Join-Path $Ctl.Root $Apps[$Key].Dir
    if (-not (Test-Path (Join-Path $dir 'package-lock.json'))) { Add-Log "No package-lock.json in $dir - npm ci needs one." 'Yellow'; return }
    $wasRunning = Test-AppRunning $Key
    if ($wasRunning) { Invoke-Stop -Only $Key -Quiet }   # a running dev server locks files in node_modules
    Add-PaneLine $Ctl.Apps[$Key] "──── ⚙ $(Get-Date -Format 'HH:mm:ss') · npm ci ────" 'Cyan'
    Add-Log "Running npm ci for $name…$(if ($Ctl.Building) { ' (next to the build)' })" 'Cyan'
    Stop-NxDaemon $dir
    $npm = Get-NpmLauncher
    $timer = [Diagnostics.Stopwatch]::StartNew()
    $Ctl.NpmCi = @{
        Key = $Key
        Process = [LazyRunnerProcess]::Start($npm.File, [string[]]($npm.Prefix + @('ci')), $dir)
        Lines = [Collections.Generic.List[string]]::new()
        Progress = @{ Key = $Key; Kind = 'npmci'; Timer = $timer; Done = 0; Total = 0; Warnings = 0 }
        StartAfter = $wasRunning
    }
}

function Update-NpmCi {
    $job = $Ctl.NpmCi
    if (-not $job) { return }
    Receive-ToolOutput $job.Process $Ctl.Apps[$job.Key] $job.Lines $job.Progress
    if (-not $job.Process.HasExited) { return }
    $null = $job.Process.WaitForExit(3000)
    Receive-ToolOutput $job.Process $Ctl.Apps[$job.Key] $job.Lines $job.Progress
    Complete-NpmCi $(if ($job.Process.ExitCode -eq 0) { 'ok' } else { 'failed' })
}

# S (when Stop all includes its app), s on its app and q cancel it.
function Stop-NpmCi {
    if (-not $Ctl.NpmCi) { return }
    taskkill /PID $Ctl.NpmCi.Process.Id /T /F 2>&1 | Out-Null
    $null = $Ctl.NpmCi.Process.WaitForExit(10000)
    Complete-NpmCi 'cancelled'
}

function Complete-NpmCi([string]$Result) {
    $job = $Ctl.NpmCi
    $Ctl.NpmCi = $null
    $key = $job.Key
    $state = $Ctl.Apps[$key]
    $log = Join-Path $ProjectStateDir "npm-ci-$key.log"
    [IO.File]::WriteAllLines($log, $job.Lines)
    $seconds = [int]$job.Progress.Timer.Elapsed.TotalSeconds
    $waiting = if ($job.StartAfter) { " - $($Apps[$key].Name) was waiting for it and stays stopped" } else { '' }
    switch ($Result) {
        'cancelled' {
            Add-PaneLine $state '■ npm ci cancelled - node_modules may be incomplete, press i again' 'Yellow'
            Add-Log "■ npm ci cancelled - node_modules may be incomplete, press i again$waiting" 'Yellow'
        }
        'failed' {
            $Ctl.LastFailedLog = $log
            $Ctl.LastBuildFailed = $true
            Add-PaneLine $state "✗ npm ci failed after ${seconds}s" 'Red'
            Add-Log "✗ npm ci failed after ${seconds}s - o opens the full log$waiting" 'Red'
            Send-Notification 'npm ci failed' "See the $($Apps[$key].Name) column in lazyrunner"
        }
        'ok' {
            Write-Json (Get-NpmCiStampPath $key) @{ root = $Ctl.Root; lockHash = Get-LockHash $Ctl.Root $key }
            $Ctl.LockChanged = @($Ctl.LockChanged | Where-Object { $_ -ne $key })
            Add-Timing 'npmci' $key $job.Progress.Timer.Elapsed.TotalSeconds
            Add-PaneLine $state "✓ npm ci done in $(Format-Duration $seconds)" 'Green'
            Add-Log "✓ npm ci done in $(Format-Duration $seconds)" 'Green'
            if ($job.StartAfter -and -not $Ctl.Quit) { Invoke-Start -Only $key }
        }
    }
}
