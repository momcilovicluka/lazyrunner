# A release repo for the self-update tests: a bare repo (what lazyrunner updates from) and a work clone that publishes
# versions to it. Dot-source it, then:
#   $release = New-ReleaseRepo -Path <folder> -Version 0.1.0   # this lazyrunner's files, as v0.1.0
#   Publish-Version $release '0.2.0' 'Something new'           # bump VERSION, add a changelog entry, push
# Uses the lazyrunner files of this checkout (committed or not; not the ones git ignores, like lazyrunner.config.json).

function Invoke-TestGit([string]$Dir, [string[]]$Arguments) {
    $output = & git -C $Dir -c user.name=tests -c user.email=tests@localhost -c commit.gpgsign=false @Arguments 2>&1
    if ($LASTEXITCODE) { throw "git $($Arguments -join ' ') failed in ${Dir}: $output" }
    $output
}

function New-ReleaseRepo([string]$Path, [string]$Version) {
    $source = Split-Path (Split-Path $PSScriptRoot)
    $remote = Join-Path $Path 'remote.git'
    $work = Join-Path $Path 'work'
    New-Item -ItemType Directory -Force $remote, $work | Out-Null
    Invoke-TestGit $remote @('init', '--quiet', '--bare', '-b', 'main') | Out-Null
    foreach ($file in @(git -C $source ls-files --cached --others --exclude-standard)) {
        $from = Join-Path $source $file
        if (-not (Test-Path -LiteralPath $from -PathType Leaf)) { continue }   # deleted but not yet committed
        $to = Join-Path $work $file
        New-Item -ItemType Directory -Force (Split-Path $to) | Out-Null
        Copy-Item -LiteralPath $from $to
    }
    Set-Content (Join-Path $work 'VERSION') $Version
    Invoke-TestGit $work @('init', '--quiet', '-b', 'main') | Out-Null
    Invoke-TestGit $work @('add', '-A') | Out-Null
    Invoke-TestGit $work @('commit', '--quiet', '-m', "v$Version") | Out-Null
    Invoke-TestGit $work @('remote', 'add', 'origin', $remote) | Out-Null
    Invoke-TestGit $work @('push', '--quiet', 'origin', 'main') | Out-Null
    @{ Remote = $remote; Work = $work }
}

# A new version on the release branch: VERSION bumped and a changelog entry on top, pushed.
function Publish-Version($Release, [string]$Version, [string]$Note) {
    Set-Content (Join-Path $Release.Work 'VERSION') $Version
    $changelog = Join-Path $Release.Work 'CHANGELOG.md'
    $text = Get-Content $changelog -Raw
    $entry = "## [$Version] - 2026-10-16`n`n### Added`n- $Note`n`n"
    $at = $text.IndexOf('## [')
    Set-Content $changelog ($text.Substring(0, $at) + $entry + $text.Substring($at)) -NoNewline
    Invoke-TestGit $Release.Work @('commit', '--quiet', '-am', "v$Version") | Out-Null
    Invoke-TestGit $Release.Work @('push', '--quiet', 'origin', 'main') | Out-Null
}
