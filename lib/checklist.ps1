# Startup checklist: a short animated splash that checks the tools, ports, the database server and the frontend
# packages before lazyrunner starts anything. "startupChecklist": false (, then w in the panel) turns it off; , then r runs it any time.

function Get-ToolVersion([string]$File, [string[]]$Arguments) {
    try {
        $output = & $File @Arguments 2>$null | Select-Object -First 1
        if ($LASTEXITCODE -eq 0 -and $output) { return "$output".Trim() }
    }
    catch { }
    $null
}

# Milliseconds to connect, or -1 when nothing answers within the timeout.
function Test-TcpPort([string]$HostName, [int]$Port, [int]$TimeoutMs = 2000) {
    $client = [Net.Sockets.TcpClient]::new()
    try {
        $timer = [Diagnostics.Stopwatch]::StartNew()
        if ($client.ConnectAsync($HostName, $Port).Wait($TimeoutMs) -and $client.Connected) { return [int]$timer.ElapsedMilliseconds }
        -1
    }
    catch { -1 }
    finally { $client.Dispose() }
}

function Get-SqlEndpoint([string]$Server) {
    $value = $Server -replace '^(tcp|np|lpc):', ''
    if ($value -match '^\(localdb\)') { return @{ Skip = 'LocalDB - not checked' } }
    $port = 1433
    if ($value -match '^(.*?),\s*(\d+)$') { $value = $Matches[1]; $port = [int]$Matches[2] }
    if ($value -match '\\') { return @{ Skip = "named instance ($value) - its port is dynamic, not checked" } }
    @{ Host = $(if ($value -in '.', '(local)') { 'localhost' } else { $value }); Port = $port }
}

# Each check returns @(status, detail) with status ok / warn / fail / skip.
function Get-Checks {
    $checks = [Collections.Generic.List[object]]::new()
    $checks.Add(@{ Name = 'git'; Run = {
                $version = Get-ToolVersion 'git' @('--version')
                if ($version) { 'ok', ($version -replace '^git version ', '') } else { 'fail', 'not found on PATH' }
            }
        })
    $checks.Add(@{ Name = '.NET SDK'; Run = {
                $version = Get-ToolVersion 'dotnet' @('--version')
                if ($version) { 'ok', $version } else { 'fail', 'dotnet not found on PATH' }
            }
        })
    if ((Get-AppKeys 'Installs')) {
        $checks.Add(@{ Name = 'Node.js / npm'; Run = {
                    $node = Get-ToolVersion 'node' @('--version')
                    if (-not $node) { return 'fail', 'node not found on PATH' }
                    $npm = Get-NpmLauncher
                    $npmVersion = Get-ToolVersion $npm.File ($npm.Prefix + @('--version'))
                    'ok', "node $node · npm $npmVersion"
                }
            })
    }
    $checks.Add(@{ Name = 'Ports'; Run = {
                Update-Listening
                $taken = foreach ($key in $Apps.Keys) {
                    $port = $Ctl.Ports[$key]
                    if (-not $port -or (Test-AppRunning $key) -or $Ctl.Listening -notcontains $port) { continue }
                    $owner = Get-NetTCPConnection -LocalPort $port -State Listen -ErrorAction SilentlyContinue | Select-Object -First 1
                    "$port used by $((Get-LiveProcess $owner.OwningProcess).ProcessName ?? 'unknown')"
                }
                $ports = @($Apps.Keys | ForEach-Object { $Ctl.Ports[$_] } | Where-Object { $_ }) -join ', '
                if ($taken) { 'warn', ($taken -join '; ') } else { 'ok', "$ports free" }
            }
        })
    $checks.Add(@{ Name = 'Database'; Run = {
                $path = Get-SecretsPath
                if (-not $path -or -not (Test-Path $path)) { return 'warn', 'no secrets.json - the APIs use appsettings' }
                $active = @(@(Get-DbGroups $path)[0].Entries | Where-Object Active)[0]
                if (-not $active) { return 'warn', 'no active connection string in secrets.json' }
                $endpoint = Get-SqlEndpoint $active.Server
                if ($endpoint.Skip) { return 'skip', "$($active.Label) · $($endpoint.Skip)" }
                $ms = Test-TcpPort $endpoint.Host $endpoint.Port
                if ($ms -ge 0) { 'ok', "$($active.Label) · port $($endpoint.Port) answers ($ms ms)" }
                else { 'warn', "$($active.Label) · nothing answers on port $($endpoint.Port)" }
            }
        })
    foreach ($key in Get-AppKeys 'Installs') {   # Arg: the app, passed to Run (a scriptblock can't keep $key)
        $checks.Add(@{ Name = "$($Apps[$key].Name) packages"; Arg = $key; Run = {
                    param($Key)
                    $dir = Join-Path $Ctl.Root $Apps[$Key].Dir
                    if (-not (Test-Path (Join-Path $dir 'node_modules'))) { return 'fail', "$($Apps[$Key].Dir)\node_modules is missing - press i (npm ci)" }
                    $stamp = Get-NpmCiStamp $Key
                    $hash = Get-LockHash $Ctl.Root $Key
                    if (-not $stamp -or $stamp.root -ne $Ctl.Root) { return 'ok', 'installed (no npm ci from lazyrunner yet to compare with)' }
                    if ($stamp.lockHash -ne $hash) { return 'warn', 'package-lock.json changed since the last npm ci - press i' }
                    'ok', 'in sync with package-lock.json'
                }
            })
    }
    $checks.Add(@{ Name = 'Project'; Run = {
                $file = if ($ProjectProfile.Source -eq 'repo') { "this repo's $(Split-Path -Leaf $ProjectProfile.Path)" } else { "$($ProjectProfile.Source) profile $($ProjectProfile.Path)" }
                if ($Ctl.ProfileChanged) { return 'warn', "$($ProjectProfile.Name) from $file - this branch changed it (, then p reloads)" }
                'ok', "$($ProjectProfile.Name) from $file"
            }
        })
    $checks.Add(@{ Name = 'Branch'; Run = {
                $modes = foreach ($key in Get-AppKeys 'Variants') {
                    $mode = $Ctl.Modes[$key]
                    if ($mode) { " · $($Apps[$key].Name.ToLowerInvariant()): $($mode.Label) ($($mode.Script))" }
                }
                'ok', "$($Ctl.Branch)$(-join $modes)"
            }
        })
    $checks
}

function Invoke-Checklist([switch]$Manual) {
    $items = @(Get-Checks | ForEach-Object { @{ Name = $_.Name; Run = $_.Run; Arg = $_.Arg; Status = 'pending'; Detail = '' } })
    $Ctl.Overlay = @{ Kind = 'checklist'; Items = $items; Footer = 'checking…'; Skip = $false }
    foreach ($item in $items) {
        $item.Status = 'running'
        $timer = [Diagnostics.Stopwatch]::StartNew()
        Show-Frame
        try { $status, $detail = & $item.Run $item.Arg } catch { $status = 'fail'; $detail = $_.Exception.Message }
        while ($timer.ElapsedMilliseconds -lt 150) { Start-Sleep -Milliseconds 40; Show-Frame }   # let each tick be seen
        $item.Status = $status
        $item.Detail = [string]$detail
    }
    $problems = @($items | Where-Object Status -in 'warn', 'fail')
    foreach ($problem in $problems) {
        Add-Log "$(if ($problem.Status -eq 'fail') { '✗' } else { '!' }) $($problem.Name): $($problem.Detail)" $(if ($problem.Status -eq 'fail') { 'Red' } else { 'Yellow' })
    }
    if (-not $problems) { Add-Log '✓ Startup checklist: all good' 'Green' }
    # Linger a moment (longer when something needs a look); any key continues.
    $holdSec = if ($Manual) { 60 } elseif ($problems) { 8 } else { 1.2 }
    $deadline = (Get-Date).AddSeconds($holdSec)
    while ((Get-Date) -lt $deadline -and -not $Ctl.Overlay.Skip -and -not $Ctl.Quit) {
        $left = [int][Math]::Ceiling(($deadline - (Get-Date)).TotalSeconds)
        $Ctl.Overlay.Footer = if ($Manual) { 'any key closes' }
        elseif ($problems) { "$($problems.Count) to look at (also in the log below) - any key continues ($left s)" }
        else { 'all good - starting…' }
        Read-UiKeys
        Show-Frame
        Start-Sleep -Milliseconds 100
    }
    $Ctl.Overlay = $null
}
