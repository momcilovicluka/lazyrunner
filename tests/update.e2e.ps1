# Self-update at startup, end to end: lazyrunner v0.1.0 (a clone of a local release repo) opens while v0.2.0 is
# published; its startup checklist installs v0.2.0, lazyrunner reopens itself on it, and the new version's checklist
# shows what's new. Run: pwsh -NoProfile -File tests\update.e2e.ps1
. "$PSScriptRoot\TestHelpers.ps1"
. "$PSScriptRoot\fixtures\New-ReleaseRepo.ps1"

$release = New-ReleaseRepo (Join-Path $TestRun 'release') '0.1.0'
$install = Join-Path $TestRun 'install'
Invoke-TestGit $TestRun @('clone', '--quiet', $release.Remote, $install) | Out-Null
Publish-Version $release '0.2.0' 'Something new'
$repo = & "$PSScriptRoot\fixtures\New-SampleRepo.ps1" -Path (Join-Path $TestRun 'repo')
New-Item -ItemType Directory -Force $env:LAZYRUNNER_STATE_DIR | Out-Null
$env:LAZYRUNNER_PROFILE = Join-Path $PSScriptRoot 'fixtures\sample.json'
$env:LAZYRUNNER_FRAME_DUMP = Join-Path $TestRun 'frame.txt'
@{ autoStartOnOpen = $false; startupChecklist = $true; notifications = $false; theme = 'default'; autoUpdate = $true; updateRepoUrl = $release.Remote } |
    ConvertTo-Json | Set-Content $env:LAZYRUNNER_CONFIG_PATH
$log = Join-Path $env:LAZYRUNNER_STATE_DIR 'lazyrunner.log'

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
function Get-LogLine([string]$Pattern) { ((Read-Text $log) -split "`r?`n" | Where-Object { $_ -match $Pattern } | Select-Object -Last 1) -replace '^\S+ \S+ ', '' }

# lazyrunner from the v0.1.0 install, in a terminal of its own (minimized), opened in the sample repo.
$ui = Start-Process (Get-Process -Id $PID).Path -ArgumentList '-NoLogo', '-NoProfile', '-File', (Join-Path $install 'lazyrunner.ps1') -WorkingDirectory $repo -WindowStyle Minimized -PassThru
try {
    "== lazyrunner v0.1.0 opens while v0.2.0 is out"
    $reopened = Wait-For { (Read-Text $env:LAZYRUNNER_FRAME_DUMP) -match "lazyrunner v0\.2\.0 · startup checklist" } 90
    Check "the checklist installed it: '$(Get-LogLine 'updated lazyrunner')'" ((Read-Text $log) -match 'updated lazyrunner v0\.1\.0 -> v0\.2\.0' -and (Get-Content (Join-Path $install 'VERSION') -Raw).Trim() -eq '0.2.0')
    Check "and lazyrunner reopened on it: '$(Get-LogLine 'Reopening')'" ($reopened -and (Read-Text $log) -match 'Reopening lazyrunner for .*\(lazyrunner v0\.2\.0 installed\)')
    $news = Wait-For { $frame = Read-Text $env:LAZYRUNNER_FRAME_DUMP; $frame -match "What's new" -and $frame -match 'Something new' } 20
    Check "the new version's checklist shows what's new ('Something new')" $news
    if (-not $news) { (Read-Text $env:LAZYRUNNER_FRAME_DUMP) -split "`r?`n" | ForEach-Object { "      | $_" } }
}
finally {
    '{ "action": "quit" }' | Set-Content (Join-Path $env:LAZYRUNNER_STATE_DIR 'request.json')
    if (-not $ui.WaitForExit(60000)) { Stop-Process -Id $ui.Id -Force }
    Remove-Item Env:LAZYRUNNER_PROFILE, Env:LAZYRUNNER_FRAME_DUMP -ErrorAction SilentlyContinue
}
Check "q ends lazyrunner itself too (exit code $($ui.ExitCode))" ($ui.ExitCode -eq 0)

Complete-Tests
