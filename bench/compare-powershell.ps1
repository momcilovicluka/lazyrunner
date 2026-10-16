# Compares the Rust lazyrunner with the PowerShell one (0.1) on the same load: -Apps apps, each printing -Rate lines a
# second (the Rust binary's synthetic emitter, run through npm for the PowerShell version). Each UI opens in a console
# window of its own (your default terminal); its CPU time and memory are read from outside the same way for both, and
# the first frame is when its frame dump appears.
# Run (after cargo build --release): pwsh -NoProfile -File bench\compare-powershell.ps1 [-Apps 5] [-Rate 2000] [-Seconds 20]
param([int]$Apps = 5, [int]$Rate = 2000, [int]$Seconds = 20, [int]$WarmupSeconds = 6)
$ErrorActionPreference = 'Stop'

$root = Split-Path $PSScriptRoot
$exe = Join-Path $root 'target\release\lazyrunner.exe'
if (-not (Test-Path $exe)) { throw "build it first: cargo build --release" }
$work = Join-Path $root 'target\bench-compare'
if (Test-Path $work) { Remove-Item $work -Recurse -Force }
New-Item -ItemType Directory $work | Out-Null
$names = 'Orders', 'Billing', 'Users', 'Cart', 'Search', 'Reports', 'Gateway', 'Mailer', 'Docs'

function Wait-Until([scriptblock]$Condition, [int]$TimeoutSec) {
    $deadline = (Get-Date).AddSeconds($TimeoutSec)
    while (-not (& $Condition)) {
        if ((Get-Date) -gt $deadline) { return $false }
        Start-Sleep -Milliseconds 5
    }
    $true
}

# The process that draws: the PowerShell launcher runs its UI in a child pwsh (`__ui`), which it reopens on demand.
function Find-UiProcess([Diagnostics.Process]$Started) {
    $child = Get-CimInstance Win32_Process -Filter "ParentProcessId = $($Started.Id)" |
        Where-Object { $_.CommandLine -match '__ui' } | Select-Object -First 1
    if ($child) { Get-Process -Id $child.ProcessId } else { $Started }
}

# Starts the UI, then reads its CPU time and memory from outside over the measured window (after the warm-up).
function Measure-Ui([string]$Name, [string]$File, [string]$Arguments, [string]$Dir, [hashtable]$Environment) {
    $dump = Join-Path $work "$Name-frame.txt"
    $saved = @{}
    foreach ($key in $Environment.Keys + 'LAZYRUNNER_FRAME_DUMP') { $saved[$key] = [Environment]::GetEnvironmentVariable($key) }
    foreach ($key in $Environment.Keys) { [Environment]::SetEnvironmentVariable($key, $Environment[$key]) }
    [Environment]::SetEnvironmentVariable('LAZYRUNNER_FRAME_DUMP', $dump)
    $clock = [Diagnostics.Stopwatch]::StartNew()
    try { $process = Start-Process $File -ArgumentList $Arguments -WorkingDirectory $Dir -PassThru }
    finally { foreach ($key in $saved.Keys) { [Environment]::SetEnvironmentVariable($key, $saved[$key]) } }
    $firstFrame = if (Wait-Until { Test-Path $dump } 60) { $clock.Elapsed.TotalMilliseconds } else { $null }
    Start-Sleep -Seconds $WarmupSeconds
    $launcher = $process
    $process = Find-UiProcess $launcher
    $process.Refresh()
    $cpuStart = $process.TotalProcessorTime
    $wallStart = $clock.Elapsed
    $peak = 0
    while (($clock.Elapsed - $wallStart).TotalSeconds -lt $Seconds -and -not $process.HasExited) {
        $process.Refresh()
        $peak = [Math]::Max($peak, $process.WorkingSet64)
        Start-Sleep -Milliseconds 500
    }
    $process.Refresh()
    $cpu = ($process.TotalProcessorTime - $cpuStart).TotalMilliseconds / ($clock.Elapsed - $wallStart).TotalMilliseconds * 100
    [pscustomobject]@{
        Process            = $launcher
        Frame              = Get-Content $dump -Raw -ErrorAction SilentlyContinue
        'launch -> first frame (ms)' = if ($null -ne $firstFrame) { [int]$firstFrame } else { 'none' }
        'cpu (% of a core)' = [Math]::Round($cpu, 1)
        'memory peak (MB)' = [int]($peak / 1MB)
    }
}

# ---------------------------------------------------------------- Rust: the spike on the same load
"Rust: $Apps apps x $Rate lines/s for ${Seconds}s (after ${WarmupSeconds}s warm-up)..."
$report = Join-Path $work 'rust-report.json'
$rust = Measure-Ui 'rust' $exe "spike --no-splash --apps $Apps --rate $Rate --bench $($WarmupSeconds + $Seconds + 3) --report `"$report`"" $work @{}
[void]$rust.Process.WaitForExit(30000)
$numbers = Get-Content $report -Raw | ConvertFrom-Json
$rust | Add-Member 'fps' $numbers.steady.fps
$rust | Add-Member 'frame p99 (ms)' $numbers.frame_interval_ms.p99

# ---------------------------------------------------------------- PowerShell: npm apps whose dev script is the same emitter
"PowerShell: the same apps through npm..."
$repo = Join-Path $work 'repo'
New-Item -ItemType Directory -Force (Join-Path $repo 'services') | Out-Null
Set-Content (Join-Path $repo 'Bench.sln') ''
$profileApps = foreach ($i in 0..($Apps - 1)) {
    $dir = Join-Path $repo "web\app$i"
    New-Item -ItemType Directory -Force (Join-Path $dir 'node_modules') | Out-Null
    $readyAfter = 1500 + 350 * $i
    $script = "`"$exe`" emit --rate $Rate --ready-after-ms $readyAfter --seed $($i + 1)"
    @{ name = "app$i"; private = $true; scripts = @{ dev = $script } } | ConvertTo-Json | Set-Content (Join-Path $dir 'package.json')
    @{ name = "app$i"; lockfileVersion = 3; requires = $true; packages = @{} } | ConvertTo-Json | Set-Content (Join-Path $dir 'package-lock.json')
    @{ key = "app$i"; name = $names[$i]; type = 'npm'; dir = "web/app$i"; variants = @(@{ script = 'dev'; port = 59900 + $i }) }
}
$profilePath = Join-Path $work 'bench.json'
@{ schemaVersion = 1; id = 'bench'; name = 'Bench'; detect = @{ anyFile = @('Bench.sln') }; watch = @('services'); apps = @($profileApps) } |
    ConvertTo-Json -Depth 6 | Set-Content $profilePath
$state = Join-Path $work 'state'
New-Item -ItemType Directory -Force $state, (Join-Path $work 'profiles') | Out-Null
$config = Join-Path $work 'config.json'
@{ autoStartOnOpen = $true; startupChecklist = $false; notifications = $false; theme = 'default'; autoUpdate = $false } | ConvertTo-Json | Set-Content $config
$psEnvironment = @{
    LAZYRUNNER_STATE_DIR    = $state
    LAZYRUNNER_CONFIG_PATH  = $config
    LAZYRUNNER_PROFILE      = $profilePath
    LAZYRUNNER_PROFILES_DIR = Join-Path $work 'profiles'
    LAZYRUNNER_UPDATE_REPO  = 'none'
}
$pwsh = (Get-Process -Id $PID).Path
$powershell = Measure-Ui 'powershell' $pwsh "-NoLogo -NoProfile -File `"$(Join-Path $root 'lazyrunner.ps1')`"" $repo $psEnvironment
$powershell | Add-Member 'fps' '<= 10 (100 ms loop)'
$powershell | Add-Member 'frame p99 (ms)' 'n/a'
'{ "action": "quit" }' | Set-Content (Join-Path $state 'request.json')
if (-not $powershell.Process.WaitForExit(30000)) { Stop-Process -Id $powershell.Process.Id -Force }
# Whatever npm left behind.
Get-Process lazyrunner -ErrorAction SilentlyContinue | Where-Object { $_.Path -eq $exe } | Stop-Process -Force -ErrorAction SilentlyContinue

# ---------------------------------------------------------------- results
$rust | Add-Member 'first frame after start (ms)' $numbers.first_frame_ms
$powershell | Add-Member 'first frame after start (ms)' 'n/a'
$columns = 'launch -> first frame (ms)', 'first frame after start (ms)', 'cpu (% of a core)', 'memory peak (MB)', 'fps', 'frame p99 (ms)'
''
"$Apps apps x $Rate lines/s, measured over ${Seconds}s"
$rows = foreach ($column in $columns) { [pscustomobject]@{ ' ' = $column; Rust = $rust.$column; PowerShell = $powershell.$column } }
$rows | Format-Table -AutoSize | Out-String
'Last PowerShell frame (top rows):'
($powershell.Frame -split "`r?`n" | Select-Object -First 6) -join "`n"
