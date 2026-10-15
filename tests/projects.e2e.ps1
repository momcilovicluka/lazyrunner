# Two projects, end to end, through the real lazyrunner command: lazyrunner opens in repo A (its own, trusted .lazyrunner.json), > widens
# a column there, then `lazyrunner start` for repo B (a profile in your profiles folder) makes it reopen for B. Each project
# keeps its own settings and state folder. Run: pwsh -NoProfile -File tests\projects.e2e.ps1
. "$PSScriptRoot\TestHelpers.ps1"
. (Join-Path $LazyRunnerRoot 'lib\profile.ps1')

$repoA = & "$PSScriptRoot\fixtures\New-SampleRepo.ps1" -Path (Join-Path $TestRun 'a')
$repoB = & "$PSScriptRoot\fixtures\New-SampleRepo.ps1" -Path (Join-Path $TestRun 'b') -OrdersPort 58211 -BillingPort 58212 -ShopPort 58311 -ConsolePort 58313
Copy-Item (Join-Path $PSScriptRoot 'fixtures\sample.json') (Join-Path $repoA '.lazyrunner.json')
# B: the same layout under other names and ports, from your profiles folder.
$other = Get-Content (Join-Path $PSScriptRoot 'fixtures\sample.json') -Raw | ConvertFrom-Json -AsHashtable
$other.id = 'other'; $other.name = 'Other'
$names = @{ orders = 'Ledger'; billing = 'Payments'; shop = 'Store'; docs = 'Guide'; console = 'Panel' }
foreach ($app in $other.apps) { $app.name = $names[$app.key] }
$other.apps[2].variants[0].port = 58311
$other.apps[4].variants[0].port = 58312; $other.apps[4].variants[1].port = 58313
New-Item -ItemType Directory -Force $env:LAZYRUNNER_PROFILES_DIR, $env:LAZYRUNNER_STATE_DIR | Out-Null
$other | ConvertTo-Json -Depth 10 | Set-Content (Join-Path $env:LAZYRUNNER_PROFILES_DIR 'other.json')
# A's repo profile is trusted already (projects.Tests.ps1 covers the question itself).
Approve-ProjectProfile (Resolve-ProjectProfile (Find-ProjectProfile $repoA $LazyRunnerRoot) $LazyRunnerRoot) (Join-Path $env:LAZYRUNNER_STATE_DIR 'trusted-profiles.json')
@{ autoStartOnOpen = $false; startupChecklist = $false; notifications = $false; theme = 'default' } | ConvertTo-Json | Set-Content $env:LAZYRUNNER_CONFIG_PATH
$env:LAZYRUNNER_FRAME_DUMP = Join-Path $TestRun 'frame.txt'
$request = Join-Path $env:LAZYRUNNER_STATE_DIR 'request.json'

function Read-Text([string]$Path) {
    if (-not (Test-Path $Path)) { return '' }
    $reader = [IO.StreamReader]::new([IO.FileStream]::new($Path, 'Open', 'Read', 'ReadWrite, Delete'))
    try { $reader.ReadToEnd() } finally { $reader.Dispose() }
}
function Wait-For([scriptblock]$Condition, [int]$TimeoutSec = 30) {
    $deadline = (Get-Date).AddSeconds($TimeoutSec)
    while ((Get-Date) -lt $deadline) {
        if (& $Condition) { return $true }
        Start-Sleep -Milliseconds 400
    }
    $false
}
function Read-Config { Get-Content $env:LAZYRUNNER_CONFIG_PATH -Raw | ConvertFrom-Json -AsHashtable }

# lazyrunner itself, in a terminal of its own (minimized), started in repo A like you would.
$lazyrunner = Start-Process (Get-Process -Id $PID).Path -ArgumentList '-NoLogo', '-NoProfile', '-File', (Join-Path $LazyRunnerRoot 'lazyrunner.ps1') -WorkingDirectory $repoA -WindowStyle Minimized -PassThru
try {
    "== lazyrunner in repo A"
    Check 'it opens with A''s apps (Orders …)' (Wait-For { (Read-Text $env:LAZYRUNNER_FRAME_DUMP) -match 'Orders' })
    @{ action = 'keys'; keys = @('>', '>') } | ConvertTo-Json | Set-Content $request
    $widened = Wait-For { (Read-Config).projects.sample.columnWidths.orders -eq 43 } 10
    Check "> > widens Orders, saved for A only: projects.sample.columnWidths.orders = $((Read-Config).projects.sample.columnWidths.orders)" $widened

    "== lazyrunner start in repo B, while it is open for A"
    @{ action = 'start'; root = $repoB; id = 'test' } | ConvertTo-Json | Set-Content $request
    Check 'it reopens with B''s apps (Ledger …)' (Wait-For { (Read-Text $env:LAZYRUNNER_FRAME_DUMP) -match 'Ledger' } 60)
    $line = ((Read-Text (Join-Path $env:LAZYRUNNER_STATE_DIR 'lazyrunner.log')) -split "`r?`n" | Where-Object { $_ -match 'Reopening lazyrunner' } | Select-Object -Last 1) -replace '^\S+ \S+ ', ''
    Check "the log says why: '$line'" ($line -match [regex]::Escape("Reopening lazyrunner for $repoB (another project)"))
    $config = Read-Config
    Check "A's width is still A's, and B has none of it ($(@($config.projects.Keys) -join ', '))" ($config.projects.sample.columnWidths.orders -eq 43 -and -not $config.projects.other.columnWidths)
    Check 'each has its own state folder: sample\, other\' ((Test-Path (Join-Path $env:LAZYRUNNER_STATE_DIR 'sample\logs')) -and (Test-Path (Join-Path $env:LAZYRUNNER_STATE_DIR 'other\logs')))
}
finally {
    '{ "action": "quit" }' | Set-Content $request
    if (-not $lazyrunner.WaitForExit(60000)) { Stop-Process -Id $lazyrunner.Id -Force }
    Remove-Item Env:LAZYRUNNER_FRAME_DUMP -ErrorAction SilentlyContinue
}
Check "q ends lazyrunner itself too (exit code $($lazyrunner.ExitCode))" ($lazyrunner.ExitCode -eq 0)

Complete-Tests
