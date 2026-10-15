# Shared by the lazyrunner test scripts; dot-source it. Keeps state, config and profiles apart from a real lazyrunner
# session, and offers Check / Complete-Tests.

[Console]::OutputEncoding = [Text.UTF8Encoding]::new($false)
$ErrorActionPreference = 'Stop'
$LazyRunnerRoot = Split-Path $PSScriptRoot
$TestRun = Join-Path ([IO.Path]::GetTempPath()) "lazyrunner-tests-$PID"
New-Item -ItemType Directory -Force $TestRun | Out-Null
$env:LAZYRUNNER_STATE_DIR = Join-Path $TestRun 'state'
$env:LAZYRUNNER_CONFIG_PATH = Join-Path $TestRun 'config.json'
$env:LAZYRUNNER_PROFILES_DIR = Join-Path $TestRun 'profiles'   # your own profiles stay out of it
$env:LAZYRUNNER_UPDATE_REPO = 'none'   # no update checks against the real release repo (tests set their own)

# lazyrunner.ps1's settings and libraries (everything before its entry point), as lazyrunner loads them.
function Get-LazyRunnerLoadScript([string]$Root = $LazyRunnerRoot) {
    $text = Get-Content (Join-Path $Root 'lazyrunner.ps1') -Raw
    $text = $text.Substring(0, $text.IndexOf('# ---------------------------------------------------------------- entry point'))
    [scriptblock]::Create((($text -replace '(?m)^#Requires.*$', '').Replace('$PSScriptRoot', "'$Root'")))
}

$script:TestFailures = 0
function Check([string]$Label, [bool]$Ok) {
    if (-not $Ok) { $script:TestFailures++ }
    "  $(if ($Ok) { 'OK  ' } else { 'FAIL' }) $Label"
}

function Complete-Tests {
    Remove-Item -Recurse -Force $TestRun -ErrorAction SilentlyContinue
    if ($script:TestFailures) { "$($script:TestFailures) failed"; exit 1 }
    'all passed'
}
