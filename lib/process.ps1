# Child processes: each server (and each build) runs with its output redirected into its column, inside a
# Windows job object so everything lazyrunner started dies with it - even when its terminal window is closed.

$ProcessHelperSource = @'
using System;
using System.Collections.Concurrent;
using System.Diagnostics;
using System.Runtime.InteropServices;
using System.Text;

public static class LazyRunnerJob
{
    [StructLayout(LayoutKind.Sequential)]
    private struct BasicLimits
    {
        public long PerProcessUserTimeLimit;
        public long PerJobUserTimeLimit;
        public uint LimitFlags;
        public UIntPtr MinimumWorkingSetSize;
        public UIntPtr MaximumWorkingSetSize;
        public uint ActiveProcessLimit;
        public UIntPtr Affinity;
        public uint PriorityClass;
        public uint SchedulingClass;
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct IoCounters
    {
        public ulong ReadOperationCount, WriteOperationCount, OtherOperationCount;
        public ulong ReadTransferCount, WriteTransferCount, OtherTransferCount;
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct ExtendedLimits
    {
        public BasicLimits BasicLimitInformation;
        public IoCounters IoInfo;
        public UIntPtr ProcessMemoryLimit;
        public UIntPtr JobMemoryLimit;
        public UIntPtr PeakProcessMemoryUsed;
        public UIntPtr PeakJobMemoryUsed;
    }

    private const int ExtendedLimitInformationClass = 9;
    private const uint KillOnJobClose = 0x2000;
    private static IntPtr job = IntPtr.Zero;

    [DllImport("kernel32.dll", SetLastError = true, CharSet = CharSet.Unicode)]
    private static extern IntPtr CreateJobObject(IntPtr attributes, string name);

    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern bool SetInformationJobObject(IntPtr job, int infoClass, ref ExtendedLimits info, uint length);

    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern bool AssignProcessToJobObject(IntPtr job, IntPtr process);

    // The job is closed when lazyrunner's process ends (however it ends), which kills every process in it.
    public static bool Assign(Process process)
    {
        if (job == IntPtr.Zero)
        {
            job = CreateJobObject(IntPtr.Zero, null);
            var limits = new ExtendedLimits();
            limits.BasicLimitInformation.LimitFlags = KillOnJobClose;
            SetInformationJobObject(job, ExtendedLimitInformationClass, ref limits, (uint)Marshal.SizeOf<ExtendedLimits>());
        }
        return AssignProcessToJobObject(job, process.Handle);
    }
}

public sealed class LazyRunnerProcess
{
    private readonly Process process;

    private LazyRunnerProcess(Process process) { this.process = process; }

    public ConcurrentQueue<string> Lines { get; } = new ConcurrentQueue<string>();
    public int Id => process.Id;
    public bool HasExited => process.HasExited;
    public int ExitCode => process.ExitCode;
    public bool WaitForExit(int milliseconds) => process.WaitForExit(milliseconds);

    public static LazyRunnerProcess Start(string file, string[] arguments, string directory)
    {
        var info = new ProcessStartInfo(file)
        {
            WorkingDirectory = directory,
            UseShellExecute = false,
            CreateNoWindow = true,          // its own hidden console: lazyrunner's keys and Ctrl+C never reach it
            RedirectStandardInput = true,   // so it never waits on a prompt
            RedirectStandardOutput = true,
            RedirectStandardError = true,
            StandardOutputEncoding = Encoding.UTF8,
            StandardErrorEncoding = Encoding.UTF8,
        };
        foreach (var argument in arguments) info.ArgumentList.Add(argument);
        var process = new Process { StartInfo = info };
        var result = new LazyRunnerProcess(process);
        process.OutputDataReceived += (sender, e) => { if (e.Data != null) result.Lines.Enqueue(e.Data); };
        process.ErrorDataReceived += (sender, e) => { if (e.Data != null) result.Lines.Enqueue(e.Data); };
        process.Start();
        LazyRunnerJob.Assign(process);
        process.BeginOutputReadLine();
        process.BeginErrorReadLine();
        return result;
    }
}

// Watches the API sources for "restart on code change"; lazyrunner polls LastChange from its loop.
public static class LazyRunnerWatcher
{
    private static System.IO.FileSystemWatcher watcher;
    private static readonly string[] Extensions = { ".cs", ".csproj", ".json", ".config", ".props", ".targets", ".cshtml", ".razor", ".resx" };
    public static long LastChange;
    public static string LastFile = "";

    public static void Watch(string directory)
    {
        Stop();
        if (!System.IO.Directory.Exists(directory)) return;
        watcher = new System.IO.FileSystemWatcher(directory) { IncludeSubdirectories = true, NotifyFilter = System.IO.NotifyFilters.LastWrite | System.IO.NotifyFilters.FileName };
        System.IO.FileSystemEventHandler changed = (sender, e) => Touch(e.FullPath);
        watcher.Changed += changed;
        watcher.Created += changed;
        watcher.Deleted += changed;
        watcher.Renamed += (sender, e) => Touch(e.FullPath);
        watcher.EnableRaisingEvents = true;
    }

    public static void Stop() { if (watcher != null) { watcher.Dispose(); watcher = null; } }

    private static void Touch(string path)
    {
        var lower = path.ToLowerInvariant();
        if (lower.Contains("\\bin\\") || lower.Contains("\\obj\\") || lower.Contains("\\node_modules\\")) return;
        if (Array.IndexOf(Extensions, System.IO.Path.GetExtension(lower)) < 0) return;
        LastFile = path;
        LastChange = DateTime.UtcNow.Ticks;
    }
}

public static class LazyRunnerConsole
{
    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern IntPtr GetStdHandle(int handle);

    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern bool GetConsoleMode(IntPtr handle, out uint mode);

    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern bool SetConsoleMode(IntPtr handle, uint mode);

    // Escape sequences (colours, cursor moves) also work in the classic console window, not only in Windows Terminal.
    public static void EnableVirtualTerminal()
    {
        var output = GetStdHandle(-11);
        if (GetConsoleMode(output, out uint mode)) SetConsoleMode(output, mode | 0x0004);
    }
}
'@

function Install-ProcessHelper {
    if ('LazyRunnerProcess' -as [type]) { return }
    Add-Type -TypeDefinition $ProcessHelperSource -ReferencedAssemblies System.Collections.Concurrent, System.Diagnostics.Process,
        System.ComponentModel.Primitives, System.Runtime.InteropServices, System.Runtime, System.Console, System.Text.Encoding.Extensions,
        System.IO.FileSystem.Watcher
}

# Strips colour/cursor escape sequences; lazyrunner colours lines itself (see Get-LineColor).
$AnsiPattern = [regex]'\x1b\[[0-9;?]*[ -/]*[@-~]|\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)'

$LevelColors = @{ info = 'Cyan'; warn = 'Yellow'; fail = 'Red'; crit = 'Red'; dbug = 'DarkGray'; trce = 'DarkGray' }
$ContinuationColors = @{ info = 'Gray'; warn = 'DarkYellow'; fail = 'Red'; crit = 'Red'; dbug = 'DarkGray'; trce = 'DarkGray' }

function Get-LineColor($State, [string]$Text) {
    # ASP.NET console logger: "info: Category[id]" then indented continuation lines of the same entry.
    if ($Text -match '^(info|warn|fail|crit|dbug|trce): ') { $State.LastLevel = $Matches[1]; return $LevelColors[$Matches[1]] }
    if ($Text -match 'Now listening on|Application started|Application bundle generation complete|[Cc]ompiled successfully|✔') { return 'Green' }
    if ($State.Traits.IndentedLogs -and $Text -match '^\s{2,}\S') { return $ContinuationColors[$State.LastLevel] }
    if ($Text -match '\b(ERROR|Error:|error (TS|NG|CS)\d+|Unhandled exception|Exception:)|✘|✖') { return 'Red' }
    if ($Text -match '\b(WARNING|Warning:)|▲') { return 'Yellow' }
    if ($Text -match '^npm (verbose|info|timing|http|sill)\b') { return 'DarkGray' }
    'Gray'
}

function New-AppState([string]$Key) {
    @{
        Key = $Key; Traits = $AppTypes[$Apps[$Key].Kind]; Proc = $null; State = 'stopped'; Gen = 0; StartedAt = $null; Script = $null
        ExitCode = $null; Error = $null; Label = $null; LastLevel = 'info'
        Lines = [Collections.Generic.List[object]]::new(); Scroll = 0; Version = 0; Render = $null
        Writer = $null; LogPath = $null
        # Live metrics (metrics.ps1) and counters shown on the column's info line
        Pids = @(); TreeKey = ''; Cpu = $null; LastCpu = $null; LastSampleAt = $null; MemMb = $null
        CpuHistory = [Collections.Generic.List[double]]::new(); MemHistory = [Collections.Generic.List[double]]::new()
        RespHistory = [Collections.Generic.List[double]]::new(); Errors = 0; Warnings = 0; FlashUntil = $null
        ReqHistory = [Collections.Generic.List[double]]::new(); Requests = 0; LastLineAt = [datetime]::MinValue
    }
}

function Add-PaneLine($State, [string]$Text, [string]$Color = 'Gray') {
    $State.Lines.Add([pscustomobject]@{ Text = $Text; Color = $Color; Born = [datetime]::UtcNow })   # Born: for the fade-in
    $State.LastLineAt = [datetime]::UtcNow
    if ($State.Scroll -gt 0) { $State.Scroll++ }   # keep a scrolled-back view still while new lines arrive
    $State.Version++
    if ($State.Writer) { $State.Writer.WriteLine($Text) }
}

function Limit-PaneLines($State) {
    $excess = $State.Lines.Count - $MaxPaneLines
    if ($excess -le 0) { return }
    $State.Lines.RemoveRange(0, $excess)
    $State.Scroll = [Math]::Min($State.Scroll, [Math]::Max(0, $State.Lines.Count - 1))
}

function Receive-AppOutput($State) {
    if (-not $State.Proc) { return }
    $line = $null
    $count = 0
    while ($count -lt $MaxLinesPerTick -and $State.Proc.Lines.TryDequeue([ref]$line)) {
        $text = $AnsiPattern.Replace($line, '').Replace("`t", '    ')
        $color = Get-LineColor $State $text
        # Count log entries, not every line of a stack trace; a new error flashes the column's title.
        if ($text -match '^(fail|crit): ' -or ($State.Traits.ColorsErrors -and $color -eq 'Red')) { $State.Errors++; $State.FlashUntil = [datetime]::UtcNow.AddSeconds(1.5) }
        elseif ($text -match '^warn: ' -or ($State.Traits.ColorsErrors -and $color -eq 'Yellow')) { $State.Warnings++ }
        if ($text -match 'Request finished ') { $State.Requests++ }   # ASP.NET request log: feeds the req/s sparkline
        Add-PaneLine $State $text $color
        $count++
    }
    if ($count) { Limit-PaneLines $State; if ($State.Writer) { $State.Writer.Flush() } }
}

function Receive-Output { foreach ($state in $Ctl.Apps.Values) { Receive-AppOutput $state } }

function Start-App([string]$Key) {
    $state = $Ctl.Apps[$Key]
    $state.Gen++
    $state.Errors = 0
    $state.Warnings = 0
    $state.Pids = @()
    $state.LastCpu = $null
    foreach ($history in $state.CpuHistory, $state.MemHistory, $state.RespHistory, $state.ReqHistory) { $history.Clear() }
    $state.Requests = 0
    if ($Ctl.ActionStarted) { (Get-TimelineEntry $Key).StartFrom = $Ctl.ActionStarted.Elapsed.TotalSeconds }
    try {
        $command = Get-ServerCommand $Key $Ctl.Root
        Add-PaneLine $state "──── ▶ $(Get-Date -Format 'HH:mm:ss') · $($Ctl.Branch) · $($command.Label) ────" 'Cyan'
        $state.Proc = [LazyRunnerProcess]::Start($command.File, [string[]]$command.Args, $command.Dir)
        $state.State = 'running'
        $state.StartedAt = [datetime]::UtcNow
        $state.ExitCode = $null
        $state.Error = $null
        $state.Label = $command.Label
        $state.Script = $command.Script   # to notice when a branch needs another variant
    }
    catch {
        $state.State = 'error'
        $state.Error = $_.Exception.Message
        Add-PaneLine $state "✗ $($_.Exception.Message)" 'Red'
        Add-Log "✗ $($Apps[$Key].Name): $($_.Exception.Message)" 'Red'
    }
}

function Stop-App([string]$Key, [string]$Reason = 'stopped') {
    $state = $Ctl.Apps[$Key]
    $process = $state.Proc
    if (-not $process) { return $false }
    if (-not $process.HasExited) {
        # Whole tree: dotnet run + the API, or npm + cmd + the dev server (Process.Kill(true) misses grandchildren).
        taskkill /PID $process.Id /T /F 2>&1 | Out-Null
        $null = $process.WaitForExit(15000)
    }
    Receive-AppOutput $state
    $state.Proc = $null
    $state.State = 'stopped'
    Add-PaneLine $state "■ $Reason" 'DarkGray'
    $true
}

function Open-PaneLogs {
    $dir = Join-Path $ProjectStateDir 'logs'
    New-Item -ItemType Directory -Force $dir | Out-Null
    foreach ($key in $Apps.Keys) {
        $path = Join-Path $dir "$key.log"
        if (Test-Path $path) { Move-Item $path (Join-Path $dir "$key.prev.log") -Force }
        $Ctl.Apps[$key].LogPath = $path
        $Ctl.Apps[$key].Writer = [IO.StreamWriter]::new($path, $false, [Text.UTF8Encoding]::new($false))
    }
}

function Close-PaneLogs {
    foreach ($state in $Ctl.Apps.Values) {
        if ($state.Writer) { $state.Writer.Dispose(); $state.Writer = $null }
    }
}
