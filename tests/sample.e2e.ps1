# Many apps, end to end: lazyrunner on the sample repo (fixtures\New-SampleRepo.ps1, profile fixtures\sample.json) - two
# .NET APIs and three npm apps: a dev server, a watch build an API serves, and one with two variants. U starts all
# five and every one must get ready; 5 focuses the fifth column; S then stops all but the app with includeInAll false.
# Run: pwsh -NoProfile -File tests\sample.e2e.ps1
. "$PSScriptRoot\TestHelpers.ps1"

$repo = & "$PSScriptRoot\fixtures\New-SampleRepo.ps1" -Path (Join-Path $TestRun 'repo')
New-Item -ItemType Directory -Force $env:LAZYRUNNER_STATE_DIR | Out-Null
$env:LAZYRUNNER_PROFILE = Join-Path $PSScriptRoot 'fixtures\sample.json'
$env:LAZYRUNNER_FRAME_DUMP = Join-Path $TestRun 'frame.txt'
$log = Join-Path $env:LAZYRUNNER_STATE_DIR 'lazyrunner.log'
$request = Join-Path $env:LAZYRUNNER_STATE_DIR 'request.json'
@{ autoStartOnOpen = $false; startupChecklist = $false; notifications = $false; theme = 'default'; projects = @{ sample = @{ apps = @{ docs = @{ includeInAll = $false } } } } } |
    ConvertTo-Json -Depth 5 | Set-Content $env:LAZYRUNNER_CONFIG_PATH

# Shared, since lazyrunner keeps its logs open while it writes them.
function Read-Text([string]$Path) {
    if (-not (Test-Path $Path)) { return '' }
    $reader = [IO.StreamReader]::new([IO.FileStream]::new($Path, 'Open', 'Read', 'ReadWrite, Delete'))
    try { $reader.ReadToEnd() } finally { $reader.Dispose() }
}
function Wait-Text([string]$Path, [string]$Pattern, [int]$TimeoutSec = 30) {
    $deadline = (Get-Date).AddSeconds($TimeoutSec)
    while ((Get-Date) -lt $deadline) {
        $text = Read-Text $Path
        if ($text -match $Pattern) { return $text }
        Start-Sleep -Milliseconds 400
    }
    $null
}
function Send-Keys([string[]]$Keys) { @{ action = 'keys'; keys = $Keys } | ConvertTo-Json | Set-Content $request }
function Get-LogLine([string]$Pattern) { ((Read-Text $log) -split "`r?`n" | Where-Object { $_ -match $Pattern } | Select-Object -Last 1) -replace '^\S+ \S+ ', '' }

$ui = Start-Process (Get-Process -Id $PID).Path -ArgumentList '-NoLogo', '-NoProfile', '-File', (Join-Path $LazyRunnerRoot 'lazyrunner.ps1'), '__ui', '-Root', $repo -WindowStyle Minimized -PassThru
try {
    "== five columns"
    $screen = Wait-Text $env:LAZYRUNNER_FRAME_DUMP 'Console'
    $titles = @($screen -split "`r?`n" | Where-Object { $_ -match 'Orders' -and $_ -match 'Billing' -and $_ -match 'Shop' -and $_ -match 'Docs' -and $_ -match 'Console' })
    Check "one title row with Orders, Billing, Shop, Docs, Console" ($titles.Count -ge 1)

    "== U starts all five"
    Send-Keys 'U'
    $ready = Wait-Text $log 'Ready in' 240
    Check "all ready: '$(Get-LogLine 'Ready in')'" ([bool]$ready)
    Check "the APIs built and listen: '$(Get-LogLine '● Orders is listening')' / '$(Get-LogLine '● Billing is listening')'" ($ready -match '● Orders is listening on :58201' -and $ready -match '● Billing is listening on :58202')
    Check "the dev server: '$(Get-LogLine '● Shop is')'" ($ready -match '● Shop is listening on :58301')
    Check "the watch build, served by Orders: '$(Get-LogLine '● Docs is')'" ($ready -match '● Docs is built \(served by Orders at http://localhost:58201\)')
    Check "the app with two variants runs the one its package.json has: '$(Get-LogLine '● Console is')'" ($ready -match '● Console is listening on :58303')
    $consoleLog = Read-Text (Join-Path $env:LAZYRUNNER_STATE_DIR 'sample\logs\console.log')
    Check "  with that variant's label in its column: 'npm run dev --verbose (live)'" ($consoleLog -match 'npm run dev --verbose \(live\)')

    "== 5 focuses the fifth column"
    Send-Keys '5'
    $focused = Wait-Text $env:LAZYRUNNER_FRAME_DUMP '▸ ■ Console' 10
    Check 'the Console column has the focus marker' ([bool]$focused)

    "== S stops all but the app with includeInAll false"
    Send-Keys 'S'
    $stopped = Wait-Text $log 'kept running' 30
    Check "'$(Get-LogLine 'Stopped ')'" ($stopped -match 'Stopped Orders \+ Billing \+ Shop \+ Console\.')
    Check "'$(Get-LogLine 'kept running')'" ($stopped -match 'Docs kept running')
}
finally {
    '{ "action": "quit" }' | Set-Content $request
    if (-not $ui.WaitForExit(30000)) { Stop-Process -Id $ui.Id -Force }
    Remove-Item Env:LAZYRUNNER_PROFILE, Env:LAZYRUNNER_FRAME_DUMP -ErrorAction SilentlyContinue
}

Complete-Tests
