# Self-update: lazyrunner checks its release repo ($UpdateRepo in lazyrunner.ps1, or updateRepoUrl / updateBranch in
# the config) for a higher VERSION and installs it - in the startup checklist (or quietly when the checklist is off),
# when the checklist runs again (, then r) and with "lazyrunner update". Only a higher VERSION counts, so pushing
# without bumping it updates nobody.
# - A git clone of the repo fast-forwards (never over local changes or local commits).
# - A copied folder (zip) is refreshed from a cache clone in the state folder; lazyrunner.config.json and any files of
#   your own stay as they are.
# After an update the UI reopens on the new files (Request-Reopen) and the checklist shows what's new.
# Dot-sourced by lazyrunner.ps1; uses common.ps1 (and view.ps1 / panel.ps1 in the UI).

$VersionPath = Join-Path $LazyRunnerRoot 'VERSION'
$ChangelogPath = Join-Path $LazyRunnerRoot 'CHANGELOG.md'
$SeenVersionPath = Join-Path $StateDir 'version.json'
$UpdateCacheDir = Join-Path $StateDir 'update'
$UpdateRef = 'refs/lazyrunner/update'
$GitTimeoutSec = 20
$WhatsNewMaxLines = 12

# ---------------------------------------------------------------- versions and the changelog

function Get-LazyRunnerVersion {
    $text = if (Test-Path $VersionPath) { (Get-Content $VersionPath -Raw).Trim() } else { '' }
    if ($text) { $text } else { '0.0.0' }
}

# "1.2.3" / "v1.2.3" / "1.2.3-beta" -> [version] 1.2.3; $null when it isn't a version.
function ConvertTo-Version([string]$Text) {
    $parsed = $null
    if ("$Text".Trim() -match '^v?(\d+(\.\d+){1,3})') { [void][version]::TryParse($Matches[1], [ref]$parsed) }
    $parsed
}

# Sections of a Keep-a-Changelog file: "## [1.2.0] - 2026-10-14" (or "## 1.2.0") and the lines below it.
# $After / $UpTo pick the versions in (After, UpTo]. Newest first, as the file lists them.
function Get-ChangelogEntries([string]$Text, [string]$After, [string]$UpTo) {
    $from = ConvertTo-Version $After
    $to = ConvertTo-Version $UpTo
    $entries = [Collections.Generic.List[object]]::new()
    $current = $null
    foreach ($line in ($Text -split '\r?\n')) {
        if ($line -match '^##\s+\[?v?(\d+(?:\.\d+){1,3}[^\]\s]*)\]?\s*(?:[-–]\s*(.+))?$') {
            $current = @{ Version = $Matches[1]; Date = "$($Matches[2])".Trim(); Lines = [Collections.Generic.List[string]]::new() }
            $entries.Add($current)
            continue
        }
        if ($line -match '^#\s' -or $line -match '^\[.+\]:\s') { $current = $null; continue }   # the title, link references
        if ($current -and $line.Trim()) { $current.Lines.Add($line.TrimEnd()) }
    }
    @($entries | Where-Object {
            $version = ConvertTo-Version $_.Version
            $version -and (-not $from -or $version -gt $from) -and (-not $to -or $version -le $to)
        })
}

function Get-Changelog { if (Test-Path $ChangelogPath) { Get-Content $ChangelogPath -Raw } else { '' } }

# Lines for showing changelog entries: "v1.2.0 · 2026-10-14" headings, then their bullets.
function Format-ChangelogLines($Entries) {
    foreach ($entry in $Entries) {
        @{ Text = "v$($entry.Version)$(if ($entry.Date) { " · $($entry.Date)" })"; Color = 'Cyan' }
        foreach ($line in $entry.Lines) {
            $text = $line -replace '^\s*[-*]\s+', '  • ' -replace '^###\s*', '' -replace '\*\*|`', ''
            @{ Text = $text; Color = $(if ($line -match '^###') { 'White' } else { 'Gray' }) }
        }
    }
}

# What changed since the version this machine last ran; remembers the current one. Nothing on the first run.
function Get-WhatsNew {
    $current = Get-LazyRunnerVersion
    $seen = (Read-Json $SeenVersionPath)?.seen
    if ($seen -ne $current) { Write-Json $SeenVersionPath @{ seen = $current } }
    if (-not $seen -or -not ((ConvertTo-Version $current) -gt (ConvertTo-Version $seen))) { return @() }
    @(Get-ChangelogEntries (Get-Changelog) $seen $current)
}

# ---------------------------------------------------------------- where updates come from

function Get-UpdateSource {
    $config = Get-Config
    $url = if ($config.updateRepoUrl) { $config.updateRepoUrl } else { $UpdateRepo.Url }
    if (-not $url -or $url -eq 'none') { return $null }
    @{ Url = $url; Branch = $(if ($config.updateBranch) { $config.updateBranch } else { $UpdateRepo.Branch }) }
}

# ---------------------------------------------------------------- progress
# What the update is doing, on the checklist's lazyrunner line (or printed by lazyrunner update):
#   checking github.com… → downloading ▕████░░░░░░▏ 40% → installing v0.2.0… → updated v0.1.0 → v0.2.0
# A step that takes longer than $StepShowSecondsAfter shows its seconds.

$StepShowSecondsAfter = 3
$ProgressBarCells = 10
$script:UpdateStep = @{ Text = ''; Percent = -1; Size = ''; Timer = [Diagnostics.Stopwatch]::new() }

# A new step is drawn at once (even a quick one); within a step the bar only moves forward, and a reading without a
# percentage (or size) keeps the last one.
function Set-UpdateStep([string]$Text, [double]$Percent = -1, [string]$Size = '') {
    $step = $script:UpdateStep
    $isNew = $step.Text -ne $Text
    if ($isNew) {
        $step.Text = $Text
        $step.Percent = -1
        $step.Size = ''
        $step.Timer.Restart()
        if (-not $script:Ctl) { Write-Host "  $Text$(if ($Percent -ge 0) { '…' })" -ForegroundColor DarkGray }
    }
    if ($Percent -ge 0) { $step.Percent = [Math]::Min(100, [Math]::Max($step.Percent, $Percent)) }
    if ($Size) { $step.Size = $Size }
    Update-StepDetail
    if ($isNew -and $script:Ctl) { Show-Frame }
}

function Format-UpdateStep {
    $step = $script:UpdateStep
    $text = $step.Text
    if ($step.Percent -ge 0) {
        $filled = [int][Math]::Floor($step.Percent / 100 * $ProgressBarCells)
        $text += " ▕$('█' * $filled)$('░' * ($ProgressBarCells - $filled))▏ $([int]$step.Percent)%"
    }
    if ($step.Size) { $text += " · $($step.Size)" }
    $seconds = $step.Timer.Elapsed.TotalSeconds
    if ($seconds -ge $StepShowSecondsAfter) { $text += " · $([int]$seconds)s" }
    $text
}

# Puts the current step on the checklist's running line; in a terminal, a percentage goes to Write-Progress.
function Update-StepDetail {
    if (-not $script:UpdateStep.Text) { return }
    if ($script:Ctl) {
        $item = if ($Ctl.Overlay -and $Ctl.Overlay.Kind -eq 'checklist') { @($Ctl.Overlay.Items | Where-Object { $_.Status -eq 'running' })[0] }
        if ($item) { $item.Detail = Format-UpdateStep }
        return
    }
    if ($script:UpdateStep.Percent -ge 0) { Write-Progress -Activity 'lazyrunner update' -Status (Format-UpdateStep) -PercentComplete $script:UpdateStep.Percent }
}

function Clear-UpdateStep {
    if (-not $script:Ctl) { Write-Progress -Activity 'lazyrunner update' -Completed }
    $script:UpdateStep.Text = ''
    $script:UpdateStep.Percent = -1
}

# Progress from git fetch --progress (stderr, lines split by CR): the server counting / compressing, then receiving
# objects ("Receiving objects:  40% (12/30), 1.20 MiB | 2.00 MiB/s") and resolving deltas. Mapped onto one 0-100
# bar, receiving being most of the work. @{ Percent (-1 = none yet); Size ('' = none yet) }.
function Get-FetchProgress([string]$Err) {
    $lines = @($Err -split '[\r\n]+' | Where-Object { $_ -match '\d+%' })
    $size = @($lines | Where-Object { $_ -match 'Receiving objects' } | ForEach-Object { if ($_ -match ',\s*([\d.]+ [KMG]?i?B)\b') { $Matches[1] } })[-1]
    $last = $lines[-1]
    if (-not $last -or $last -notmatch '(\d+)%') { return @{ Percent = -1; Size = "$size" } }
    $percent = [double]$Matches[1]
    $overall = if ($last -match 'Resolving deltas') { 85 + $percent * 0.15 } elseif ($last -match 'Receiving objects') { 10 + $percent * 0.75 } else { $percent * 0.1 }
    @{ Percent = $overall; Size = "$size" }
}

# ---------------------------------------------------------------- git

# git without prompts (a missing login fails instead of waiting for a dialog), with a timeout. In the UI the frame
# keeps redrawing while it waits. -Interactive (lazyrunner update in a terminal) lets git ask for a login.
# -Progress: the command is a fetch --progress, whose percentage goes to the current step.
function Invoke-Git([string[]]$Arguments, [string]$Dir, [int]$TimeoutSec = $GitTimeoutSec, [switch]$Interactive, [switch]$Progress) {
    $info = [Diagnostics.ProcessStartInfo]::new('git')
    foreach ($argument in @('-C', $Dir) + $Arguments) { $info.ArgumentList.Add($argument) }
    $info.UseShellExecute = $false
    $info.RedirectStandardOutput = $true
    $info.RedirectStandardError = $true
    $info.CreateNoWindow = $true
    if (-not $Interactive) {
        $info.Environment['GIT_TERMINAL_PROMPT'] = '0'
        $info.Environment['GCM_INTERACTIVE'] = 'never'
        if (-not $env:GIT_SSH_COMMAND) { $info.Environment['GIT_SSH_COMMAND'] = 'ssh -o BatchMode=yes' }
    }
    try { $process = [Diagnostics.Process]::Start($info) }
    catch { return @{ Ok = $false; Out = ''; Err = "git couldn't start: $($_.Exception.Message)" } }
    $out = $process.StandardOutput.ReadToEndAsync()
    $err = [Text.StringBuilder]::new()
    $buffer = [char[]]::new(4096)
    $read = $process.StandardError.ReadAsync($buffer, 0, $buffer.Length)
    $timer = [Diagnostics.Stopwatch]::StartNew()
    while ($true) {
        while ($read.IsCompleted -and $read.Result -gt 0) {   # stderr as it comes, for the progress
            [void]$err.Append($buffer, 0, $read.Result)
            $read = $process.StandardError.ReadAsync($buffer, 0, $buffer.Length)
        }
        if ($Progress) { $fetched = Get-FetchProgress $err.ToString(); Set-UpdateStep $script:UpdateStep.Text $fetched.Percent $fetched.Size }
        else { Update-StepDetail }
        if ($process.WaitForExit(100)) { break }
        if ($script:Ctl) { Show-Frame }
        if ($timer.Elapsed.TotalSeconds -gt $TimeoutSec) {
            try { $process.Kill($true) } catch { $null = $_ }   # it may have just exited
            return @{ Ok = $false; Out = ''; Err = "no answer within ${TimeoutSec}s" }
        }
    }
    $process.WaitForExit()
    if (-not $read.IsCompleted) { [void]$read.Wait(2000) }
    if ($read.IsCompleted -and $read.Result -gt 0) { [void]$err.Append($buffer, 0, $read.Result); [void]$err.Append($process.StandardError.ReadToEnd()) }
    @{ Ok = $process.ExitCode -eq 0; Out = $out.Result.Trim(); Err = $err.ToString().Trim() }
}

# The line of git's error output worth showing: its fatal: / error: line, else the first one.
function Get-GitMessage([string]$Err) {
    $lines = @($Err -split '\r?\n' | Where-Object { $_.Trim() })
    $main = @($lines | Where-Object { $_ -match '^(fatal|error): ' })[0] ?? $lines[0]
    "$main" -replace '^(fatal|error): ', ''
}

# The install is a clone of its own (not a folder inside some other repo).
function Test-GitInstall {
    if (-not (Test-Path (Join-Path $LazyRunnerRoot '.git'))) { return $false }
    $top = Invoke-Git @('rev-parse', '--show-toplevel') $LazyRunnerRoot 5
    $top.Ok -and [IO.Path]::GetFullPath($top.Out).TrimEnd('\') -eq [IO.Path]::GetFullPath($LazyRunnerRoot).TrimEnd('\')
}

# Asks the release repo for its branch's commit (ls-remote, no download), then fetches it into $UpdateRef only when
# it isn't there yet - in the install itself when it is a clone, else in the cache - and reads its VERSION.
# @{ Version; Mode = git / copy; Dir } or @{ Error }.
function Get-RemoteRelease($Source, [switch]$Interactive) {
    $mode = if (Test-GitInstall) { 'git' } else { 'copy' }
    $dir = if ($mode -eq 'git') { $LazyRunnerRoot } else { $UpdateCacheDir }
    if ($mode -eq 'copy' -and -not (Test-Path (Join-Path $dir '.git'))) {
        New-Item -ItemType Directory -Force $dir | Out-Null
        $init = Invoke-Git @('init', '--quiet') $dir
        if (-not $init.Ok) { return @{ Error = "couldn't prepare $dir - $(Get-GitMessage $init.Err)" } }
    }
    $repoHost = try { ([uri]$Source.Url).Host } catch { '' }
    Set-UpdateStep "checking $(if ($repoHost) { $repoHost } else { 'the release repo' })…"
    $remote = Invoke-Git @('ls-remote', $Source.Url, "refs/heads/$($Source.Branch)") $dir -Interactive:$Interactive
    if (-not $remote.Ok) { return @{ Error = (Get-FetchError $remote.Err $Source) } }
    $commit = ("$($remote.Out)" -split '\s+')[0]
    if (-not $commit) { return @{ Error = "no branch $($Source.Branch) at $($Source.Url)" } }
    $have = Invoke-Git @('rev-parse', '--verify', '--quiet', "$UpdateRef^{commit}") $dir 5
    if ($have.Out -ne $commit) {
        Set-UpdateStep 'downloading' 0
        # Only the cache is shallow: --depth in a full clone would make that clone shallow too.
        $depth = if ($mode -eq 'copy') { @('--depth', '1') } else { @() }
        $fetch = Invoke-Git (@('fetch', '--progress', '--no-tags') + $depth + @($Source.Url, "+refs/heads/$($Source.Branch):$UpdateRef")) $dir -Interactive:$Interactive -Progress
        if (-not $fetch.Ok) { return @{ Error = (Get-FetchError $fetch.Err $Source) } }
        Set-UpdateStep 'downloading' 100
    }
    $version = Invoke-Git @('show', "${UpdateRef}:VERSION") $dir 5
    if (-not $version.Ok -or -not (ConvertTo-Version $version.Out)) { return @{ Error = "the $($Source.Branch) branch has no VERSION file" } }
    @{ Version = $version.Out.Trim(); Mode = $mode; Dir = $dir }
}

function Get-FetchError([string]$Message, $Source) {
    $repoHost = try { ([uri]$Source.Url).Host } catch { $Source.Url }
    if (-not $repoHost) { $repoHost = $Source.Url }
    if ($Message -match 'terminal prompts disabled|Authentication failed|could not read Username|Permission denied \(publickey|returned error: 40[13]') { return "not signed in to $repoHost - run lazyrunner update once in a terminal" }
    if ($Message -match "couldn't find remote ref") { return "no branch $($Source.Branch) at $($Source.Url)" }
    if ($Message -match 'does not appear to be a git repository|[Rr]epository .*not found|returned error: 404') { return "no repo at $($Source.Url)" }
    if ($Message -match 'Could not resolve host|unable to access|timed out|no answer') { return "can't reach $repoHost (offline?)" }
    Get-GitMessage $Message
}

# Puts the fetched release in place. $null when done, else why not.
function Install-Release($Release, [string]$Branch) {
    Set-UpdateStep "installing v$($Release.Version)…"
    if ($Release.Mode -eq 'git') {
        $current = Invoke-Git @('rev-parse', '--abbrev-ref', 'HEAD') $LazyRunnerRoot 5
        if ($current.Out -ne $Branch) { return "this clone is on $($current.Out), not $Branch" }
        $dirty = Invoke-Git @('status', '--porcelain', '--untracked-files=no') $LazyRunnerRoot 10
        if ($dirty.Out) { return "it has local changes ($LazyRunnerRoot)" }
        $merge = Invoke-Git @('merge', '--ff-only', '--quiet', $UpdateRef) $LazyRunnerRoot
        if (-not $merge.Ok) { return "it has commits of its own that aren't in the release repo" }
    }
    else {
        $checkout = Invoke-Git @('--work-tree', $LazyRunnerRoot, 'checkout', '--force', $UpdateRef, '--', '.') $Release.Dir
        if (-not $checkout.Ok) { return "copying the files failed - $(Get-GitMessage $checkout.Err)" }
    }
    if ((Get-LazyRunnerVersion) -ne $Release.Version) { return "VERSION still says $(Get-LazyRunnerVersion) afterwards" }
    $null
}

# Checks and, when a higher version is out, installs it. @{ Status = ok / warn / skip; Detail; UpdatedTo }.
function Update-LazyRunner([switch]$Force, [switch]$Interactive) {
    try { Invoke-SelfUpdate -Force:$Force -Interactive:$Interactive }
    finally { Clear-UpdateStep }
}

function Invoke-SelfUpdate([switch]$Force, [switch]$Interactive) {
    $version = Get-LazyRunnerVersion
    if (-not $Force -and -not (Get-Config).autoUpdate) { return @{ Status = 'skip'; Detail = "v$version · auto-update off (, then v turns it on)" } }
    $source = Get-UpdateSource
    if (-not $source) { return @{ Status = 'skip'; Detail = "v$version · no release repo configured" } }
    $release = Get-RemoteRelease $source -Interactive:$Interactive
    if ($release.Error) { return @{ Status = 'warn'; Detail = "v$version · couldn't check for updates: $($release.Error)" } }
    $local = ConvertTo-Version $version
    $remote = ConvertTo-Version $release.Version
    if ($remote -lt $local) { return @{ Status = 'ok'; Detail = "v$version · ahead of the released v$($release.Version)" } }
    if ($remote -eq $local) { return @{ Status = 'ok'; Detail = "v$version · up to date" } }
    $problem = Install-Release $release $source.Branch
    if ($problem) { return @{ Status = 'warn'; Detail = "v$version · v$($release.Version) is out, but not installed: $problem" } }
    Add-Content -Path $LogPath -Value "$(Get-Date -Format 'yyyy-MM-dd HH:mm:ss') updated lazyrunner v$version -> v$($release.Version)" -ErrorAction SilentlyContinue
    @{ Status = 'ok'; Detail = "updated v$version → v$($release.Version)"; UpdatedTo = $release.Version }
}

# ---------------------------------------------------------------- in the UI

# After an update the UI reopens on the new files. At startup nothing runs yet, so it reopens right away (and starts
# the apps if it was going to). Later (, then r) running apps would be stopped, so it leaves that to you: , then p.
function Restart-AfterUpdate([string]$Version, [switch]$Manual) {
    $running = @($Ctl.Apps.Keys | Where-Object { $Ctl.Apps[$_].State -eq 'running' }).Count -or $Ctl.Building -or $Ctl.NpmCi
    if ($Manual -and $running) {
        Add-Log "lazyrunner v$Version is installed - , then p reopens lazyrunner on it (the running apps restart)" 'Cyan'
        return
    }
    Request-Reopen $Ctl.Root -Start:($Ctl.StartNow -and -not $Manual) "lazyrunner v$Version installed"
}

# Startup with the checklist off: the same check, its outcome in the log.
function Invoke-QuietUpdate {
    $result = Update-LazyRunner
    if ($result.Status -eq 'warn') { Add-Log "! lazyrunner: $($result.Detail)" 'Yellow' }
    if ($result.UpdatedTo) { Restart-AfterUpdate $result.UpdatedTo }
}

function Write-WhatsNewLog($Entries) {
    if (-not $Entries) { return }
    Add-Log "lazyrunner updated to v$(Get-LazyRunnerVersion) - what's new (, then c shows the changelog):" 'Cyan'
    foreach ($line in @(Format-ChangelogLines $Entries | Where-Object { $_.Text -match '^\s+•' } | Select-Object -First 5)) { Add-Log $line.Text 'Gray' }
}

function Open-ChangelogView {
    $lines = @(Format-ChangelogLines (Get-ChangelogEntries (Get-Changelog)))
    if (-not $lines) { Add-Log "No changelog next to lazyrunner ($ChangelogPath)." 'DarkGray'; return }
    $Ctl.Overlay = @{ Kind = 'changelog'; Lines = $lines; Top = 0 }
}

# Changelog lines wrapped at word boundaries to $Width; a bullet's continuation lines line up under its text.
function Get-WrappedLines($Lines, [int]$Width) {
    foreach ($line in $Lines) {
        $text = [string]$line.Text
        $indent = if ($text -match '^\s*•\s|^\s+') { ' ' * $Matches[0].Length } else { '' }
        while ($text.Length -gt $Width) {
            $cut = $text.LastIndexOf(' ', $Width)
            if ($cut -le $indent.Length) { $cut = $Width }
            @{ Text = $text.Substring(0, $cut).TrimEnd(); Color = $line.Color }
            $text = $indent + $text.Substring($cut).TrimStart()
        }
        @{ Text = $text; Color = $line.Color }
    }
}

function Get-ChangelogRows([int]$Width, [int]$Rows) {
    $overlay = $Ctl.Overlay
    $lines = @(Get-WrappedLines $overlay.Lines ([Math]::Max(20, [Math]::Min($Width - 8, 90))))
    $visible = [Math]::Max(1, $Rows - 4)
    $overlay.Top = [Math]::Max(0, [Math]::Min($overlay.Top, $lines.Count - $visible))
    $shown = foreach ($line in @($lines | Select-Object -Skip $overlay.Top -First $visible)) { , @(New-Seg $line.Text $line.Color) }
    $more = if ($lines.Count -gt $visible) { "$($overlay.Top + 1)-$($overlay.Top + @($shown).Count) of $($lines.Count) · j k scroll · " } else { '' }
    Get-BoxRows "lazyrunner v$($Ctl.Version) · changelog" @($shown) "${more}Esc closes" $Width $Rows 90
}

# Keys in the changelog: j k and pages scroll; Esc, q or c close it.
function Invoke-ChangelogKey($Overlay, [string]$Name) {
    switch -CaseSensitive ($Name) {
        { $_ -cin 'Escape', 'q', 'c' } { $Ctl.Overlay = $null }
        { $_ -cin 'j', 'DownArrow' } { $Overlay.Top++ }
        { $_ -cin 'k', 'UpArrow' } { $Overlay.Top-- }
        { $_ -in 'Ctrl+D', 'PageDown' } { $Overlay.Top += 10 }
        { $_ -in 'Ctrl+U', 'PageUp' } { $Overlay.Top -= 10 }
        { $_ -cin 'g', 'Home' } { $Overlay.Top = 0 }
        { $_ -cin 'G', 'End' } { $Overlay.Top = [int]::MaxValue }
    }
}

# ---------------------------------------------------------------- lazyrunner update / lazyrunner version (no UI)

function Invoke-UpdateCommand {
    if (-not (Get-UpdateSource)) { Write-Host "lazyrunner v$(Get-LazyRunnerVersion) - no release repo configured (`$UpdateRepo in lazyrunner.ps1, or updateRepoUrl in $(Split-Path -Leaf $ConfigPath))." -ForegroundColor DarkGray; return }
    Write-Host "lazyrunner v$(Get-LazyRunnerVersion)" -ForegroundColor Cyan
    $before = Get-LazyRunnerVersion
    $result = Update-LazyRunner -Force -Interactive
    $color = @{ ok = 'Green'; warn = 'Yellow'; skip = 'DarkGray' }[$result.Status]
    Write-Host $result.Detail -ForegroundColor $color
    if (-not $result.UpdatedTo) { return }
    foreach ($line in @(Format-ChangelogLines (Get-ChangelogEntries (Get-Changelog) $before $result.UpdatedTo))) { Write-Host $line.Text -ForegroundColor $line.Color }
    if (Get-Controller) { Write-Host 'lazyrunner is open - , then p there reopens it on the new version.' -ForegroundColor Yellow }
}

function Show-VersionCommand {
    Write-Host "lazyrunner v$(Get-LazyRunnerVersion)" -ForegroundColor Cyan
    $source = Get-UpdateSource
    $auto = if ((Get-Config).autoUpdate) { 'on' } else { 'off' }
    Write-Host $(if ($source) { "updates from $($source.Url) ($($source.Branch)), auto-update $auto" } else { 'no release repo configured' }) -ForegroundColor DarkGray
    foreach ($line in @(Format-ChangelogLines @(Get-ChangelogEntries (Get-Changelog))[0])) { Write-Host $line.Text -ForegroundColor $line.Color }
}
