# Live CPU and memory per app, summed over its process tree (dotnet run -> API, npm -> cmd -> node), and the
# short histories behind the sparklines (CPU, memory, response time). CPU is a share of the whole machine,
# like Task Manager shows it.

$SparkChars = '▁▂▃▄▅▆▇█'.ToCharArray()
$CoreCount = [Environment]::ProcessorCount
$MetricsIntervalSec = 2
$TreeIntervalSec = 6
$HistoryLength = 30

function Add-History($List, [double]$Value) {
    $List.Add($Value)
    if ($List.Count -gt $HistoryLength) { $List.RemoveAt(0) }
}

# $Floor keeps small values small (e.g. 3% CPU is a low bar, not a full one).
function Get-Sparkline($Values, [double]$Floor, [int]$Width = 10) {
    $recent = @($Values | Select-Object -Last $Width)
    if (-not $recent) { return '' }
    $max = [Math]::Max($Floor, ($recent | Measure-Object -Maximum).Maximum)
    -join @(foreach ($value in $recent) { $SparkChars[[int][Math]::Min(7, [Math]::Floor([double]$value / $max * 7.999))] })
}

# Over the last minute the newest samples average 20% above the oldest ones.
function Test-MemoryGrowing($State) {
    $history = $State.MemHistory
    if ($history.Count -lt $HistoryLength) { return $false }
    $old = ($history | Select-Object -First 5 | Measure-Object -Average).Average
    $new = ($history | Select-Object -Last 5 | Measure-Object -Average).Average
    $old -gt 0 -and $new -gt $old * 1.2
}

function Update-ProcessTrees {
    $children = @{}
    foreach ($process in Get-CimInstance Win32_Process -Property ProcessId, ParentProcessId -ErrorAction SilentlyContinue) {
        $parent = [int]$process.ParentProcessId
        if (-not $children.ContainsKey($parent)) { $children[$parent] = [Collections.Generic.List[int]]::new() }
        $children[$parent].Add([int]$process.ProcessId)
    }
    foreach ($state in $Ctl.Apps.Values) {
        if (-not $state.Proc) { $state.Pids = @(); continue }
        $ids = [Collections.Generic.List[int]]::new()
        $queue = [Collections.Generic.Queue[int]]::new()
        $queue.Enqueue($state.Proc.Id)
        while ($queue.Count) {
            $id = $queue.Dequeue()
            if ($ids.Contains($id)) { continue }
            $ids.Add($id)
            if ($children.ContainsKey($id)) { foreach ($child in $children[$id]) { $queue.Enqueue($child) } }
        }
        $state.Pids = $ids.ToArray()
    }
    $Ctl.NextTree = (Get-Date).AddSeconds($TreeIntervalSec)
}

function Update-Metrics {
    if ((Get-Date) -lt $Ctl.NextMetrics) { return }
    $Ctl.NextMetrics = (Get-Date).AddSeconds($MetricsIntervalSec)
    $stale = @($Ctl.Apps.Values | Where-Object { $_.Proc -and -not $_.Pids }).Count
    if ($stale -or (Get-Date) -ge $Ctl.NextTree) { Update-ProcessTrees }
    $now = [datetime]::UtcNow
    foreach ($state in $Ctl.Apps.Values) {
        if (-not $state.Proc -or -not $state.Pids) { $state.Cpu = $null; $state.LastCpu = $null; continue }
        $cpu = [timespan]::Zero
        $memory = 0L
        foreach ($process in @(Get-Process -Id $state.Pids -ErrorAction SilentlyContinue)) {
            try { $cpu += $process.TotalProcessorTime; $memory += $process.WorkingSet64 } catch { }   # it may exit meanwhile
        }
        # A changed process tree would make the CPU delta meaningless, so that sample only resets the baseline.
        $treeKey = $state.Pids -join ','
        if ($null -ne $state.LastCpu -and $state.TreeKey -eq $treeKey) {
            $percent = [Math]::Max(0, ($cpu - $state.LastCpu).TotalMilliseconds / ($now - $state.LastSampleAt).TotalMilliseconds / $CoreCount * 100)
            $state.Cpu = $percent
            Add-History $state.CpuHistory $percent
        }
        $state.LastCpu = $cpu
        $state.LastSampleAt = $now
        $state.TreeKey = $treeKey
        $state.MemMb = [int]($memory / 1MB)
        Add-History $state.MemHistory $state.MemMb
        if ($state.Traits.CountsRequests) {   # requests per second since the last sample
            Add-History $state.ReqHistory ($state.Requests / $MetricsIntervalSec)
            $state.Requests = 0
        }
    }
}
