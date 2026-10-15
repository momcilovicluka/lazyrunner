# Full-screen rendering: one column per app (title, info line, output), the control panel below, overlays
# (database picker, command palette, key reference, startup checklist), colour themes and the terminal tab
# status. Rows are diffed and only changed rows are rewritten.

$SpinnerFrames = '⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏'.ToCharArray()
$OrbitFrames = '◐◓◑◒'.ToCharArray()
$SparkleFrames = '✦✧'.ToCharArray()
$ProgressBlocks = ' ▏▎▍▌▋▊▉'.ToCharArray()
$BrightColors = @{ Gray = 'White'; DarkGray = 'Gray'; DarkCyan = 'Cyan'; DarkYellow = 'Yellow'; DarkGreen = 'Green'; DarkRed = 'Red'; DarkMagenta = 'Magenta' }
$Logo = @(   # startup splash; a light sweeps across it
    '█   ▄▀▄ ▀▀█ █ █ █▀▄ █ █ █▀▄ █▀▄ █▀▀ █▀▄'
    '█   █▀█ ▄▀  ▀█▀ █▀▄ █ █ █ █ █ █ █▀  █▀▄'
    '▀▀▀ ▀ ▀ ▀▀▀  ▀  ▀ ▀ ▀▀▀ ▀ ▀ ▀ ▀ ▀▀▀ ▀ ▀'
)
$ToastAppId = '{1AC14E77-02E7-4E5D-B744-2EB1AE5198B7}\WindowsPowerShell\v1.0\powershell.exe'

# ---------------------------------------------------------------- themes

# Themes come from themes\*.json (lib\themes.ps1); Get-AnsiStyle turns a colour name into its escape code.

function Get-AnsiStyle([string]$Color, [string]$Back) {
    $cacheKey = "$Color|$Back"
    $style = $Ctl.StyleCache[$cacheKey]
    if ($style) { return $style }
    $style = "`e[0;$($Ctl.Theme.Fg[$Color] ?? '39')"
    if ($Back) { $style += ";$($Ctl.Theme.Bg[$Back] ?? '7')" }
    $style += 'm'
    $Ctl.StyleCache[$cacheKey] = $style
    $style
}

# ---------------------------------------------------------------- basics

function New-Seg([string]$Text, [string]$Color = 'Gray', [string]$Back) { [pscustomobject]@{ Text = $Text; Color = $Color; Back = $Back } }

# Exactly $Width visible cells: segments are truncated or padded.
function ConvertTo-Row($Segments, [int]$Width) {
    $builder = [Text.StringBuilder]::new($Width + 64)
    $used = 0
    foreach ($seg in $Segments) {
        if ($used -ge $Width) { break }
        $text = $seg.Text
        if (-not $text) { continue }
        if ($used + $text.Length -gt $Width) { $text = $text.Substring(0, $Width - $used) }
        [void]$builder.Append((Get-AnsiStyle $seg.Color $seg.Back)).Append($text)
        $used += $text.Length
    }
    [void]$builder.Append("`e[0m")
    if ($used -lt $Width) { [void]$builder.Append([char]' ', $Width - $used) }
    $builder.ToString()
}

function Format-Cell([string]$Text, [string]$Color, [int]$Width, [string]$Back) {
    if ($Text.Length -gt $Width) { $Text = $Text.Substring(0, $Width) }
    if ($Back) { $Text = $Text.PadRight($Width) }   # a highlighted line is highlighted to the column's edge
    "$(Get-AnsiStyle $Color $Back)$Text`e[0m$(' ' * ($Width - $Text.Length))"
}

function Get-SegmentsLength($Segments) { [int](($Segments | ForEach-Object { $_.Text.Length } | Measure-Object -Sum).Sum) }

function Get-Spinner { [string]$SpinnerFrames[$Ctl.Frame % $SpinnerFrames.Count] }

function Format-Duration([double]$Seconds) {
    if ($Seconds -lt 60) { return "$([int]$Seconds)s" }
    '{0}m{1:00}s' -f [int][Math]::Floor($Seconds / 60), [int]($Seconds % 60)
}

function Get-SecondsSince([datetime]$UtcTime) { ([datetime]::UtcNow - $UtcTime).TotalSeconds }

# Progress bar that glides towards its target (no jumps) and shimmers while it waits. $Id keeps its position.
function Get-Bar([double]$Fraction, [string]$Id = '', [int]$Width = 10) {
    $target = [Math]::Max(0.0, [Math]::Min(1.0, $Fraction))
    if ($Id) {
        $shown = $Ctl.Bars[$Id]
        if ($null -eq $shown -or $target -lt $shown) { $shown = $target }   # restart or reset: no gliding backwards
        $shown += ($target - $shown) * 0.35
        if ([Math]::Abs($target - $shown) -lt 0.004) { $shown = $target }
        $Ctl.Bars[$Id] = $shown
        $target = $shown
    }
    $cells = $target * $Width
    $full = [int][Math]::Floor($cells)
    $partial = if ($full -lt $Width) { [string]$ProgressBlocks[[int][Math]::Floor(($cells - $full) * 8)] } else { '' }
    $filled = [char[]]('█' * $full)
    if ($full -gt 1 -and $target -lt 1) { $filled[$Ctl.Frame % $full] = '▓' }   # a light glint runs along the bar
    '▕' + (-join $filled) + $partial + (' ' * [Math]::Max(0, $Width - $full - $partial.Length)) + '▏ '
}

# ---------------------------------------------------------------- terminal status (tab title + taskbar)

function Update-TerminalStatus {
    $running = @($Apps.Keys | Where-Object { $Ctl.Apps[$_].State -eq 'running' })
    $up = @($running | Where-Object { $Ctl.UpGen[$_] -eq $Ctl.Apps[$_].Gen })
    $failed = $Ctl.LastBuildFailed -or @($Apps.Keys | Where-Object { $Ctl.Apps[$_].State -in 'exited', 'error' }).Count
    $slow = @($running | Where-Object { $last = $Ctl.Probes[$_].Last; $last -and (-not $last.Ok -or $last.Ms -ge $SlowResponseMs) }).Count
    # OSC 9;4 states: 1 = green, 2 = red, 4 = yellow (Windows Terminal shows them on the tab and taskbar button).
    $tools = @($(if ($Ctl.Building) { "building $($Apps[$Ctl.Building.Key].Name)…" }), $(if ($Ctl.NpmCi) { 'npm ci…' }) | Where-Object { $_ })
    $status = if ($tools) { '4;100', ($tools -join ' + '), 'Yellow' }
    elseif ($failed) { '2;100', 'something failed - see the log', 'Red' }
    elseif ($running.Count -and $up.Count -lt $running.Count) { '4;100', "starting ($($up.Count)/$($running.Count) up)…", 'Yellow' }
    elseif ($slow) { '4;100', 'up, but slow / not responding', 'Yellow' }
    elseif ($running.Count) { '1;100', "● all up ($($up.Count))", 'Green' }
    else { '0;0', '○ stopped', 'DarkGray' }
    $Ctl.Summary = @{ Text = $status[1]; Color = $status[2] }
    $database = if ($Ctl.Db) { "$(($Ctl.Db -split ' @ ')[0]) · " } else { '' }   # tab title: which database you're on
    $osc = "`e]9;4;$($status[0])`e\`e]0;lazyrunner · $database$($status[1])`a"
    if ($osc -ne $Ctl.LastOsc) { [Console]::Out.Write($osc); $Ctl.LastOsc = $osc }
}

function Send-Notification([string]$Title, [string]$Message) {
    if (-not $Ctl.Config.notifications) { return }
    # Toasts need WinRT, which only Windows PowerShell 5.1 projects, so it shows them for us.
    $title = [Security.SecurityElement]::Escape("lazyrunner · $Title")
    $text = [Security.SecurityElement]::Escape($Message)
    $script = @"
[Windows.UI.Notifications.ToastNotificationManager, Windows.UI.Notifications, ContentType = WindowsRuntime] > `$null
[Windows.Data.Xml.Dom.XmlDocument, Windows.Data.Xml.Dom.XmlDocument, ContentType = WindowsRuntime] > `$null
`$xml = New-Object Windows.Data.Xml.Dom.XmlDocument
`$xml.LoadXml('<toast><visual><binding template="ToastGeneric"><text>$title</text><text>$text</text></binding></visual></toast>')
[Windows.UI.Notifications.ToastNotificationManager]::CreateToastNotifier('$ToastAppId').Show([Windows.UI.Notifications.ToastNotification]::new(`$xml))
"@
    $encoded = [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes($script))
    try { Start-Process powershell.exe -WindowStyle Hidden -ArgumentList '-NoProfile', '-NonInteractive', '-EncodedCommand', $encoded }
    catch { Add-Content -Path $LogPath -Value "notification failed: $($_.Exception.Message)" -ErrorAction SilentlyContinue }
}

# ---------------------------------------------------------------- columns

# The dotnet build or the background npm ci working on this app's column, if any (both can run at once).
function Get-ToolProgress([string]$Key) {
    if ($Ctl.Building -and $Ctl.Building.Key -eq $Key) { return $Ctl.Building }
    if ($Ctl.NpmCi -and $Ctl.NpmCi.Progress.Key -eq $Key) { return $Ctl.NpmCi.Progress }
}

# Colour of an app's state for its title chip; it flashes red for a moment when an error line arrives.
function Get-AppTone([string]$Key) {
    $state = $Ctl.Apps[$Key]
    if ($state.FlashUntil -and [datetime]::UtcNow -lt $state.FlashUntil -and $Ctl.Frame % 4 -lt 2) { return 'Red' }
    if (Get-ToolProgress $Key) { return 'Yellow' }
    if ($state.State -eq 'running') { if ($Ctl.UpGen[$Key] -eq $state.Gen) { return 'Green' } else { return 'Yellow' } }
    if ($state.State -in 'exited', 'error') { return 'Red' }
    'DarkGray'
}

function Get-ProbeSegments([string]$Key) {
    $last = $Ctl.Probes[$Key].Last
    if (-not $last) { return @(New-Seg 'checking…' 'DarkGray') }
    if (-not $last.Ok) { return @(New-Seg 'not responding' 'Red') }
    $code = if ($last.Code -ge 400) { " HTTP $($last.Code)" } else { '' }
    if ($last.Ms -ge $SlowResponseMs) { return @(New-Seg "slow $([Math]::Round($last.Ms / 1000, 1))s$code" 'Yellow') }
    @(New-Seg "$($last.Ms) ms$code" 'Green')
}

function Get-AppStatusSegments([string]$Key) {
    $state = $Ctl.Apps[$Key]
    $building = Get-ToolProgress $Key
    if ($building) {
        $elapsed = $building.Timer.Elapsed.TotalSeconds
        if ($building.Total) {   # real progress: projects MSBuild reported done
            $done = [Math]::Min($building.Done, $building.Total)
            return @(New-Seg "$(Get-Spinner) building $(Get-Bar ($done / $building.Total) "build:$Key")$done/$($building.Total) · $(Format-Duration $elapsed)" 'Yellow')
        }
        $estimate = Get-TimingEstimate $building.Kind $Key
        $frames = if ($building.Kind -eq 'npmci') { $OrbitFrames } else { $SpinnerFrames }
        $verb = if ($building.Kind -ne 'npmci') { 'building' } elseif ($Ctl.NpmCi.StartAfter) { 'npm ci → start' } else { 'npm ci' }
        $bar = if ($estimate) { Get-Bar ($elapsed / $estimate) "$($building.Kind):$Key" } else { '' }
        $eta = if ($estimate) { " / ~$(Format-Duration $estimate)" } else { '' }
        return @(New-Seg "$([string]$frames[$Ctl.Frame % $frames.Count]) $verb $bar$(Format-Duration $elapsed)$eta" 'Yellow')
    }
    switch ($state.State) {
        'running' {
            if ($Ctl.UpGen[$Key] -eq $state.Gen) {
                $up = "  up in $(Format-Duration $Ctl.UpAfter[$Key])"
                if (-not $Ctl.Ports[$Key]) { return @((New-Seg '● built, watching' 'Green'), (New-Seg $up 'DarkGray')) }
                return @(New-Seg '● ' 'Green') + (Get-ProbeSegments $Key) + @(New-Seg $up 'DarkGray')
            }
            # Not up yet: the dot pulses and the bar fills against the usual startup time.
            $elapsed = Get-SecondsSince $state.StartedAt
            $estimate = Get-TimingEstimate 'start' $Key
            $dot = if ($Ctl.Frame % 6 -lt 3) { '●' } else { '○' }
            $verb = $state.Traits.StartVerb
            $bar = if ($estimate) { Get-Bar ($elapsed / $estimate) "start:$Key" } else { '' }
            $eta = if ($estimate) { " / ~$(Format-Duration $estimate)" } else { '' }
            return @(New-Seg "$dot $verb $bar$(Format-Duration $elapsed)$eta" 'Yellow')
        }
        'exited' { return @(New-Seg "✗ exited (code $($state.ExitCode))" 'Red') }
        'error'  { return @(New-Seg "✗ $($state.Error)" 'Red') }
        default  { return @(New-Seg '○ stopped' 'DarkGray') }
    }
}

# Border colour = state colour, dimmed when stopped (never flashes, so borders stay calm).
function Get-BorderColor([string]$Key) {
    $state = $Ctl.Apps[$Key]
    if (Get-ToolProgress $Key) { return 'DarkYellow' }
    if ($state.State -eq 'running') { if ($Ctl.UpGen[$Key] -eq $state.Gen) { return 'DarkGreen' } else { return 'DarkYellow' } }
    if ($state.State -in 'exited', 'error') { return 'DarkRed' }
    'DarkGray'
}

function Get-ColumnTitle([string]$Key, [int]$Width) {
    $focused = $Key -eq $Ctl.Focus
    $tone = Get-AppTone $Key
    $marker = if ($focused) { '▸ ' } else { '' }
    $segments = [Collections.Generic.List[object]]::new()
    $segments.Add((New-Seg " $marker$($AppIcons[$Key] ?? '■') $($Apps[$Key].Name) " $(if ($tone -eq 'DarkGray') { 'White' } else { 'Black' }) $tone))
    $segments.Add((New-Seg $(if ($Ctl.Ports[$Key]) { " :$($Ctl.Ports[$Key])  " } else { '  ' }) 'DarkGray'))
    foreach ($seg in Get-AppStatusSegments $Key) { $segments.Add($seg) }
    $used = Get-SegmentsLength $segments
    # The title line takes the app's state colour (heavy when focused), like its column border.
    $fill = New-Seg (' ' + ($(if ($focused) { '━' } else { '─' }) * [Math]::Max(0, $Width - $used - 1))) $(Get-BorderColor $Key)
    $segments.Add($fill)
    if ($focused) {
        # The focused pane's whole title bar takes the theme's focus colours (the state chip keeps its own).
        for ($i = 1; $i -lt $segments.Count; $i++) { $segments[$i] = New-Seg $segments[$i].Text 'FocusText' 'FocusBack' }
    }
    ConvertTo-Row $segments $Width
}

# Second title line: live CPU / memory with sparklines, response-time trend, error counters, mode, scroll.
function Get-ColumnInfo([string]$Key, [int]$Width) {
    $state = $Ctl.Apps[$Key]
    $segments = [Collections.Generic.List[object]]::new()
    $segments.Add((New-Seg ' '))
    # Scrolled back: said first, so a narrow column can't cut it off.
    if ($state.Scroll -gt 0) { $segments.Add((New-Seg "⏸ ↑$($state.Scroll) · G follows  " 'Magenta')) }
    if ($state.State -eq 'running' -and $null -ne $state.Cpu) {
        $cpuColor = if ($state.Cpu -ge 80) { 'Red' } elseif ($state.Cpu -ge 40) { 'Yellow' } else { 'Green' }
        $segments.Add((New-Seg 'cpu ' 'DarkGray'))
        $segments.Add((New-Seg ('{0,3:0}%' -f $state.Cpu) $cpuColor))
        $segments.Add((New-Seg " $(Get-Sparkline $state.CpuHistory 25)" 'DarkCyan'))
        $segments.Add((New-Seg '  mem ' 'DarkGray'))
        $segments.Add((New-Seg "$($state.MemMb) MB" 'Gray'))
        if (Test-MemoryGrowing $state) { $segments.Add((New-Seg ' ↗' 'Yellow')) }
    }
    if ($Ctl.Ports[$Key] -and $state.RespHistory.Count -and $state.State -eq 'running') {
        $last = $state.RespHistory[$state.RespHistory.Count - 1]
        $color = if ($last -ge $SlowResponseMs) { 'Yellow' } else { 'DarkCyan' }
        $segments.Add((New-Seg '  resp ' 'DarkGray'))
        $segments.Add((New-Seg (Get-Sparkline $state.RespHistory 200) $color))
    }
    if ($state.ReqHistory.Count -and $state.State -eq 'running') {
        $segments.Add((New-Seg '  req ' 'DarkGray'))
        $segments.Add((New-Seg ('{0:0.#}/s ' -f $state.ReqHistory[$state.ReqHistory.Count - 1]) 'Gray'))
        $segments.Add((New-Seg (Get-Sparkline $state.ReqHistory 5) 'DarkMagenta'))
    }
    if ($state.Errors) { $segments.Add((New-Seg "  ✗ $($state.Errors)" 'Red')) }
    if ($state.Warnings) { $segments.Add((New-Seg "  △ $($state.Warnings)" 'Yellow')) }
    $mode = $Ctl.Modes[$Key]
    if ($mode) { $segments.Add((New-Seg "  $($mode.Label) · $($mode.Script)" 'DarkGray')) }
    ConvertTo-Row $segments $Width
}

# Output rows for a column, newest at the bottom, wrapped to the width; cached until the output or size changes.
function Get-ColumnBody([string]$Key, [int]$Width, [int]$Rows) {
    $state = $Ctl.Apps[$Key]
    # New lines fade in (bright → normal over ~1 s), so the cache also changes while that happens.
    $now = [datetime]::UtcNow
    $fading = ($now - $state.LastLineAt).TotalMilliseconds -lt 1200
    # Search matches (/ in this column) are highlighted: the current one in the focus colour, the rest yellow.
    $query = if ($Ctl.Search -and $Ctl.Search.Query -and $Ctl.Search.Key -eq $Key) { $Ctl.Search.Query } else { '' }
    $current = if ($query) { $Ctl.Search.Index } else { -1 }
    $cacheKey = "$($state.Version)|$($state.Scroll)|$Width|$Rows|$(if ($fading) { $Ctl.Frame })|$query|$current"
    if ($state.Render -and $state.Render.Key -eq $cacheKey) { return $state.Render.Rows }
    $result = [Collections.Generic.List[string]]::new()
    $index = $state.Lines.Count - 1 - $state.Scroll
    while ($index -ge 0 -and $result.Count -lt $Rows) {
        $line = $state.Lines[$index]
        $text = $line.Text
        $chunks = [Collections.Generic.List[string]]::new()
        if ($text.Length -le $Width) { $chunks.Add($text) }
        else { for ($i = 0; $i -lt $text.Length; $i += $Width) { $chunks.Add($text.Substring($i, [Math]::Min($Width, $text.Length - $i))) } }
        $color = $line.Color
        $back = $null
        if ($fading -and $line.Born) {
            $age = ($now - $line.Born).TotalMilliseconds
            if ($age -lt 400) { $color = 'White' } elseif ($age -lt 1000) { $color = $BrightColors[$color] ?? $color }
        }
        if ($query -and $text.IndexOf($query, [StringComparison]::OrdinalIgnoreCase) -ge 0) {
            if ($index -eq $current) { $color = 'FocusText'; $back = 'FocusBack' } else { $color = 'Black'; $back = 'Yellow' }
        }
        for ($c = $chunks.Count - 1; $c -ge 0 -and $result.Count -lt $Rows; $c--) { $result.Insert(0, (Format-Cell $chunks[$c] $color $Width $back)) }
        $index--
    }
    $blank = ' ' * $Width
    while ($result.Count -lt $Rows) { $result.Insert(0, $blank) }
    $state.Render = @{ Key = $cacheKey; Rows = $result }
    $result
}

function Get-ColumnWidths([string[]]$Keys, [int]$Total) {
    $configured = $Ctl.Config.columnWidths ?? @{}
    $weights = @(foreach ($key in $Keys) { [double]($configured[$key] ?? 33) })
    $sum = ($weights | Measure-Object -Sum).Sum
    $widths = [int[]]@(foreach ($weight in $weights) { [Math]::Max(12, [int][Math]::Floor($Total * $weight / $sum)) })
    $widths[$widths.Count - 1] = [Math]::Max(1, $widths[$widths.Count - 1] + $Total - ($widths | Measure-Object -Sum).Sum)
    $widths
}

function Get-BodyRows([int]$Width, [int]$Rows) {
    if ($Ctl.Overlay -and $Ctl.Overlay.Kind -eq 'checklist') { return Get-ChecklistRows $Width $Rows }
    if ($Ctl.Picker) { return Get-PickerRows $Width $Rows }
    if ($Ctl.Overlay -and $Ctl.Overlay.Kind -eq 'palette') { return Get-PaletteRows $Width $Rows }
    if ($Ctl.Overlay -and $Ctl.Overlay.Kind -eq 'help') { return Get-HelpRows $Width $Rows }
    if ($Ctl.Overlay -and $Ctl.Overlay.Kind -in 'menu', 'settings') { return Get-MenuRows $Width $Rows }
    $Ctl.BodyRows = $Rows - 2   # search scrolls a match to the middle of this
    $keys = @(if ($Ctl.Zoom) { $Ctl.Focus } else { $Apps.Keys })   # @() so one zoomed column stays a list
    $widths = Get-ColumnWidths $keys ($Width - ($keys.Count - 1))
    $columns = [Collections.Generic.List[object]]::new()
    for ($i = 0; $i -lt $keys.Count; $i++) {
        $column = [Collections.Generic.List[string]]::new()
        $column.Add((Get-ColumnTitle $keys[$i] $widths[$i]))
        $column.Add((Get-ColumnInfo $keys[$i] $widths[$i]))
        $column.AddRange([string[]](Get-ColumnBody $keys[$i] $widths[$i] ($Rows - 2)))
        $columns.Add($column)
    }
    # Each border takes the state colour of the column on its left; both borders of the focused column are
    # heavy lines in the theme's focus colour.
    $separators = [string[]]::new([Math]::Max(0, $keys.Count - 1))
    $titleSeparators = [string[]]::new($separators.Count)
    for ($i = 0; $i -lt $separators.Count; $i++) {
        if ($Ctl.Focus -in $keys[$i], $keys[$i + 1]) { $color = 'FocusBorder'; $line = '┃'; $top = '┳' }
        else { $color = Get-BorderColor $keys[$i]; $line = '│'; $top = '┬' }
        $separators[$i] = "$(Get-AnsiStyle $color)$line`e[0m"
        $titleSeparators[$i] = "$(Get-AnsiStyle $color)$top`e[0m"
    }
    for ($row = 0; $row -lt $Rows; $row++) {
        $line = [Text.StringBuilder]::new()
        for ($c = 0; $c -lt $columns.Count; $c++) {
            if ($c -gt 0) { [void]$line.Append($(if ($row -eq 0) { $titleSeparators[$c - 1] } else { $separators[$c - 1] })) }
            [void]$line.Append($columns[$c][$row])
        }
        $line.ToString()
    }
}

# ---------------------------------------------------------------- overlays (boxes drawn over the columns)

# Cuts segments to $Width cells, ending in … when something was cut off.
function Limit-Segments($Segments, [int]$Width) {
    if ((Get-SegmentsLength $Segments) -le $Width) { return @($Segments) }
    $result = [Collections.Generic.List[object]]::new()
    $used = 0
    foreach ($seg in $Segments) {
        $room = $Width - $used
        if ($room -le 0) { break }
        $text = if ($seg.Text.Length -le $room) { $seg.Text } else { $seg.Text.Substring(0, $room - 1) + '…' }
        $result.Add((New-Seg $text $seg.Color $seg.Back))
        $used += $text.Length
    }
    $result.ToArray()
}

# Rows for a centred box: $Lines are segment arrays (one per inner line).
function Get-BoxRows([string]$Title, $Lines, [string]$Footer, [int]$Width, [int]$Rows, [int]$MaxInner = 96) {
    $inner = [Math]::Max(20, [Math]::Min($Width - 8, $MaxInner))
    $left = ' ' * [Math]::Max(0, [int](($Width - $inner - 4) / 2))
    $box = [Collections.Generic.List[object]]::new()
    $box.Add(@((New-Seg "$left┌─" 'Cyan'), (New-Seg " $Title " 'White'), (New-Seg ('─' * [Math]::Max(0, $inner - $Title.Length - 1) + '┐') 'Cyan')))
    foreach ($line in $Lines) {
        $segments = Limit-Segments @($line) $inner   # never push the right border out
        $pad = [Math]::Max(0, $inner - (Get-SegmentsLength $segments))
        $box.Add(@(New-Seg "$left│ " 'Cyan') + $segments + @((New-Seg (' ' * $pad)), (New-Seg ' │' 'Cyan')))
    }
    if ($Footer) {
        $box.Add(@((New-Seg "$left│ " 'Cyan'), (New-Seg $Footer.PadRight($inner).Substring(0, $inner) 'DarkGray'), (New-Seg ' │' 'Cyan')))
    }
    $box.Add(@(New-Seg "$left└$('─' * ($inner + 2))┘" 'Cyan'))
    $top = [Math]::Max(0, [int](($Rows - $box.Count) / 2))
    for ($r = 0; $r -lt $Rows; $r++) {
        $index = $r - $top
        if ($index -ge 0 -and $index -lt $box.Count) { ConvertTo-Row $box[$index] $Width } else { ' ' * $Width }
    }
}

function Get-PickerRows([int]$Width, [int]$Rows) {
    $picker = $Ctl.Picker
    $group = $picker.Groups[$picker.Group]
    $entries = @(Get-PickerEntries $picker)
    $inner = [Math]::Max(20, [Math]::Min($Width - 8, 96))
    $visible = [Math]::Max(1, [Math]::Min($entries.Count, $Rows - 8))
    if ($picker.Index -lt $picker.Top) { $picker.Top = $picker.Index }
    if ($picker.Index -ge $picker.Top + $visible) { $picker.Top = $picker.Index - $visible + 1 }
    $lines = [Collections.Generic.List[object]]::new()
    $lines.Add(@(New-Seg "secrets.json · $(@($group.Entries).Count) connection strings · active one marked ●" 'DarkGray'))
    if ($picker.Filtering -or $picker.Filter) {
        $cursor = if ($picker.Filtering -and $Ctl.Frame % 10 -lt 5) { '▏' } else { ' ' }
        $lines.Add(@((New-Seg '/ ' 'Cyan'), (New-Seg $picker.Filter 'White'), (New-Seg $cursor 'Cyan'), (New-Seg "  $($entries.Count) shown" 'DarkGray')))
    }
    if (-not $entries) { $lines.Add(@(New-Seg '  nothing matches' 'DarkGray')) }
    for ($i = $picker.Top; $i -lt [Math]::Min($entries.Count, $picker.Top + $visible); $i++) {
        $entry = $entries[$i]
        $marker = if ($entry.Active) { '●' } else { ' ' }
        $text = " $marker $($entry.Label)$(if ($entry.Active) { '   (active)' })"
        $text = $(if ($text.Length -gt $inner) { $text.Substring(0, $inner) } else { $text.PadRight($inner) })
        $segment = if ($i -eq $picker.Index) { New-Seg $text 'Black' 'Cyan' } elseif ($entry.Active) { New-Seg $text 'Green' } else { New-Seg $text 'Gray' }
        $lines.Add(@($segment))
    }
    $groupInfo = if ($picker.Groups.Count -gt 1) { "  ($($picker.Group + 1)/$($picker.Groups.Count), h l other keys)" } else { '' }
    if ($null -ne $picker.NewName) {   # "n": typing the new database name
        $cursor = if ($Ctl.Frame % 10 -lt 5) { '▏' } else { ' ' }
        $lines.Add(@((New-Seg '+ new database (copy of the selected one): ' 'Cyan'), (New-Seg $picker.NewName 'White'), (New-Seg $cursor 'Cyan')))
        return Get-BoxRows "Database · $($group.Key)" $lines 'type the database name · Enter adds it (inactive) · Esc cancels' $Width $Rows
    }
    $more = if ($entries.Count -gt $visible) { "  ($($picker.Top + 1)-$($picker.Top + $visible) of $($entries.Count))" } else { '' }
    $footer = if ($picker.Filtering) { 'type to filter · Enter keeps it · Esc clears it' } else { "j k choose · Enter switch + restart the APIs · n new · / filter · Esc close$more" }
    Get-BoxRows "Database · $($group.Key)$groupInfo" $lines $footer $Width $Rows
}

function Get-PaletteRows([int]$Width, [int]$Rows) {
    $palette = $Ctl.Overlay
    $found = @(Get-PaletteMatches $palette.Query)
    $palette.Index = [Math]::Max(0, [Math]::Min($palette.Index, $found.Count - 1))
    $inner = [Math]::Max(20, [Math]::Min($Width - 8, 80))
    $cursor = if ($Ctl.Frame % 10 -lt 5) { '▏' } else { ' ' }
    $lines = [Collections.Generic.List[object]]::new()
    $lines.Add(@((New-Seg '› ' 'Cyan'), (New-Seg $palette.Query 'White'), (New-Seg $cursor 'Cyan')))
    $lines.Add(@(New-Seg ('─' * $inner) 'DarkGray'))
    $visible = [Math]::Max(1, [Math]::Min($found.Count, $Rows - 8))
    $first = [Math]::Max(0, [Math]::Min($palette.Index - $visible + 1, $found.Count - $visible))
    if ($palette.Index -lt $first) { $first = $palette.Index }
    for ($i = $first; $i -lt [Math]::Min($found.Count, $first + $visible); $i++) {
        $action = $found[$i]
        $key = "[$($action.Key)]".PadRight(9)
        $label = " $($action.Label)"
        $hint = if ($action.Hint) { "  $($action.Hint)" } else { '' }
        if ($i -eq $palette.Index) {
            $text = "$key$label$hint"
            $lines.Add(@(New-Seg $(if ($text.Length -gt $inner) { $text.Substring(0, $inner) } else { $text.PadRight($inner) }) 'Black' 'Cyan'))
        }
        else { $lines.Add(@((New-Seg $key 'Cyan'), (New-Seg $label 'White'), (New-Seg $hint 'DarkGray'))) }
    }
    if (-not $found) { $lines.Add(@(New-Seg 'no matching command' 'DarkGray')) }
    Get-BoxRows 'Commands' $lines 'type to search · ↑↓ or Ctrl+N Ctrl+P choose · Enter run · Esc close' $Width $Rows 80
}

# The x menu (actions for the focused app) and the , settings: an item's key runs it, j k + Enter too.
function Get-MenuRows([int]$Width, [int]$Rows) {
    $overlay = $Ctl.Overlay
    $items = @(Get-OverlayItems $overlay.Kind)
    $overlay.Index = [Math]::Max(0, [Math]::Min($overlay.Index, $items.Count - 1))
    $inner = 64
    $lines = [Collections.Generic.List[object]]::new()
    for ($i = 0; $i -lt $items.Count; $i++) {
        $item = $items[$i]
        $key = " $($item.Key)".PadRight(8)
        $hint = if ($item.Hint) { "  $($item.Hint)" } else { '' }
        if ($i -eq $overlay.Index) {
            $text = "$key$($item.Label)$hint"
            $lines.Add(@(New-Seg $(if ($text.Length -gt $inner) { $text.Substring(0, $inner) } else { $text.PadRight($inner) }) 'Black' 'Cyan'))
            continue
        }
        $labelColor = if ($item.Label -like '`[x`]*') { 'Green' } else { 'White' }
        $lines.Add(@((New-Seg $key 'Cyan'), (New-Seg $item.Label $labelColor), (New-Seg $hint 'DarkGray')))
    }
    if ($overlay.Kind -eq 'menu') {
        Get-BoxRows "$($Apps[$Ctl.Focus].Name)" $lines 'press a key, or j k + Enter · Esc / x close' $Width $Rows $inner
    }
    else { Get-BoxRows 'Settings (saved)' $lines 'press a letter to toggle, or j k + Enter · Esc / , close' $Width $Rows $inner }
}

# The digits that focus a column, for the key reference: "1 2 3", or "1-9" for many columns.
function Get-ColumnDigits {
    $count = [Math]::Min(9, $Apps.Count)
    if ($count -le 4) { (1..$count) -join ' ' } else { "1-$count" }
}

function Get-HelpRows([int]$Width, [int]$Rows) {
    $entries = [Collections.Generic.List[object]]::new()
    $add = { param($Keys, $Label) $entries.Add(@((New-Seg ("  {0,-12}" -f $Keys) 'Yellow'), (New-Seg $Label 'Gray'))) }
    $actions = @(Get-Actions | Where-Object { $_.Show -ne $false })
    foreach ($group in @($actions | ForEach-Object Group | Select-Object -Unique)) {   # in the order the list defines
        $entries.Add(@(New-Seg $group 'Cyan'))
        foreach ($action in @($actions | Where-Object Group -eq $group)) {
            # A lowercase key and its uppercase twin share a line ("u U  Start · focused / all"); so do > and <.
            $twin = $actions | Where-Object { $_.Key -ceq $action.Key.ToUpperInvariant() -and $_.Key -cne $action.Key } | Select-Object -First 1
            if ($action.Key -cmatch '^[A-Z]$' -and ($actions | Where-Object { $_.Key -ceq $action.Key.ToLowerInvariant() })) { continue }
            if ($action.Key -eq '<') { continue }
            if ($twin) { & $add "$($action.Key) $($twin.Key)" "$($action.Label -replace ' the focused app', '') · focused / all"; continue }
            if ($action.Key -eq '>') { & $add '> <' 'Widen / narrow the focused column'; continue }
            & $add $action.Key $action.Label
        }
    }
    $entries.Add(@(New-Seg 'Navigate' 'Cyan'))
    $navigation = @(
        ('h l Tab', 'focus another column'), ((Get-ColumnDigits), 'jump to a column'), ('j k ↑ ↓', 'scroll a line'),
        ('^u ^d ^b ^f', 'half page / page (PgUp PgDn)'), ('gg G', 'oldest / follow new output'), ('n N Esc', 'search: older / newer / clear')
    )
    foreach ($pair in $navigation) { & $add $pair[0] $pair[1] }
    # As many columns side by side as the height needs, so the whole reference fits; the box is as wide as its
    # columns' longest lines.
    $columns = [Math]::Max(2, [int][Math]::Ceiling($entries.Count / [Math]::Max(1, $Rows - 4)))
    $perColumn = [int][Math]::Ceiling($entries.Count / $columns)
    $widths = @(for ($c = 0; $c -lt $columns; $c++) {
        $slice = @($entries | Select-Object -Skip ($c * $perColumn) -First $perColumn)
        [int](($slice | ForEach-Object { Get-SegmentsLength @($_) } | Measure-Object -Maximum).Maximum) + 4
    })
    $lines = [Collections.Generic.List[object]]::new()
    for ($i = 0; $i -lt $perColumn; $i++) {
        $line = [Collections.Generic.List[object]]::new()
        for ($c = 0; $c -lt $columns; $c++) {
            $index = $c * $perColumn + $i
            if ($index -ge $entries.Count) { break }
            $segments = @($entries[$index])
            $line.AddRange([object[]]$segments)
            if ($c -lt $columns - 1) { $line.Add((New-Seg (' ' * [Math]::Max(1, $widths[$c] - (Get-SegmentsLength $segments))))) }
        }
        $lines.Add($line.ToArray())
    }
    $inner = ($widths | Measure-Object -Sum).Sum - 4
    Get-BoxRows 'Keys' $lines 'any key closes · : searches every command · lowercase = focused app, uppercase = all' $Width $Rows $inner
}

function Get-ChecklistRows([int]$Width, [int]$Rows) {
    $overlay = $Ctl.Overlay
    $icons = @{ pending = '·'; ok = '✓'; warn = '!'; fail = '✗'; skip = '–' }
    $colors = @{ pending = 'DarkGray'; running = 'Yellow'; ok = 'Green'; warn = 'Yellow'; fail = 'Red'; skip = 'DarkGray' }
    $lines = [Collections.Generic.List[object]]::new()
    $lines.Add(@(New-Seg ''))
    $sweep = ($Ctl.Frame % ($Logo[0].Length + 15)) - 6   # column of the light; sweeps left to right, then pauses off-logo
    foreach ($logoLine in $Logo) {
        $segments = [Collections.Generic.List[object]]::new()
        $segments.Add((New-Seg '   '))
        for ($i = 0; $i -lt $logoLine.Length; $i++) {
            $distance = [Math]::Abs($i - $sweep)
            $segments.Add((New-Seg ([string]$logoLine[$i]) $(if ($distance -le 1) { 'White' } elseif ($distance -le 3) { 'Cyan' } else { 'DarkCyan' })))
        }
        $lines.Add($segments.ToArray())
    }
    $lines.Add(@(New-Seg ''))
    foreach ($item in $overlay.Items) {
        $icon = if ($item.Status -eq 'running') { Get-Spinner } else { $icons[$item.Status] }
        $lines.Add(@((New-Seg " $icon " $colors[$item.Status]), (New-Seg $item.Name.PadRight(19) 'White'), (New-Seg $item.Detail $(if ($item.Status -eq 'ok') { 'Gray' } else { $colors[$item.Status] }))))
    }
    $lines.Add(@(New-Seg ''))
    Get-BoxRows "$([string]$SparkleFrames[[int]($Ctl.Frame / 3) % 2]) lazyrunner · startup checklist" $lines $overlay.Footer $Width $Rows 84
}

# ---------------------------------------------------------------- control panel

function Get-CompactMenu([int]$Width) {
    $items = [Collections.Generic.List[object]]::new()
    if ($Ctl.Conflicts) { $items.Add(@{ Key = '!'; Label = 'kill port user'; Color = 'Red' }) }
    if ($Ctl.LockChanged) { $items.Add(@{ Key = 'i'; Label = 'npm ci'; Color = 'Yellow' }) }
    $menu = @(
        ('?', 'keys'), (':', 'commands'), ('space', 'start/stop'), ('u/U', 'start'), ('s/S', 'stop'), ('r/R', 'restart'),
        ('b/B', 'rebuild'), ('d', 'database'), ('w', 'open'), ('x', 'menu'), ('/', 'search'), (',', 'settings'), ('q', 'quit')
    )
    if ((Get-AppKeys 'Installs') -and -not $Ctl.LockChanged) { $menu += @(, ('i', 'npm ci')) }
    foreach ($pair in $menu) { $items.Add(@{ Key = $pair[0]; Label = $pair[1] }) }
    # Only what fits on one line; ? and : reach everything else. Lowercase = focused app, uppercase = all.
    $segments = [Collections.Generic.List[object]]::new()
    $used = 0
    foreach ($item in $items) {
        $length = $item.Key.Length + $item.Label.Length + 5
        if ($used + $length -gt $Width) { break }
        $segments.Add((New-Seg " $($item.Key)" 'Cyan'))
        $segments.Add((New-Seg " $($item.Label)  " ($item.Color ?? 'Gray')))
        $used += $length
    }
    $segments
}

# Every on/off setting, always visible; , opens the settings menu, where the letter shown toggles it. Wraps onto
# more rows when the window is narrow.
function Get-SettingsRows([int]$Width) {
    $settings = [Collections.Generic.List[object]]::new()
    $settings.Add(@('a', 'auto-restart on branch change', $Ctl.Config.autoRestartOnBranchChange))
    $settings.Add(@('u', 'auto-start when lazyrunner opens', $Ctl.Config.autoStartOnOpen))
    if ((Get-AppKeys 'Frontend')) { $settings.Add(@('s', 'stop/restart all include frontend', $Ctl.Config.stopFrontendOnStop)) }
    $settings.Add(@('e', 'restart APIs on code change', $Ctl.Config.restartOnCodeChange))
    $settings.Add(@('w', 'startup checklist', $Ctl.Config.startupChecklist))
    $rows = [Collections.Generic.List[object]]::new()
    $current = [Collections.Generic.List[object]]::new()
    $current.Add((New-Seg ' ,' 'Cyan'))
    $current.Add((New-Seg ' settings' 'DarkGray'))
    $used = 11
    foreach ($setting in $settings) {
        $box = if ($setting[2]) { '[x]' } else { '[ ]' }
        $length = $setting[0].Length + $box.Length + $setting[1].Length + 6
        if ($used + $length -gt $Width -and $current.Count -gt 1) {
            $rows.Add($current.ToArray())
            $current = [Collections.Generic.List[object]]::new()
            $current.Add((New-Seg '           '))
            $used = 11
        }
        $current.Add((New-Seg "   $($setting[0]) " 'DarkCyan'))
        $current.Add((New-Seg $box $(if ($setting[2]) { 'Green' } else { 'DarkGray' })))
        $current.Add((New-Seg " $($setting[1])" $(if ($setting[2]) { 'Gray' } else { 'DarkGray' })))
        $used += $length
    }
    $rows.Add($current.ToArray())
    $rows
}

function Get-NoticeSegments {
    $search = $Ctl.Search
    if ($search -and $search.Typing) {   # the / prompt, like vim's command line
        $cursor = if ($Ctl.Frame % 10 -lt 5) { '▏' } else { ' ' }
        return @((New-Seg ' /' 'Cyan'), (New-Seg $search.Query 'White'), (New-Seg $cursor 'Cyan'), (New-Seg "   search $($Apps[$search.Key].Name) · Enter finds · Esc cancels" 'DarkGray'))
    }
    if ($search) {
        $where = if ($search.Note -eq 'no match') { 'no match' } else { "match $($search.Position) of $($search.Total) (1 = newest)$(if ($search.Note) { " · $($search.Note)" })" }
        return @((New-Seg " /$($search.Query)" 'Cyan'), (New-Seg "   $where · n older · N newer · Esc clears" $(if ($search.Note -eq 'no match') { 'Yellow' } else { 'DarkGray' })))
    }
    if ($Ctl.Conflicts) {
        $owners = ($Ctl.Conflicts | ForEach-Object { "port $($_.Port) is used by $($_.Name) (PID $($_.Pid))" }) -join ', '
        return @((New-Seg " ! $owners - " 'Red'), (New-Seg '[!]' 'Cyan'), (New-Seg ' kills it and starts' 'Red'))
    }
    if ($Ctl.Busy) { return @(New-Seg " $(Get-Spinner) $($Ctl.Busy)" 'Yellow') }
    if ($Ctl.LockChanged) {
        return @((New-Seg " ! $($Apps[(Select-NpmCiKey)].Dir)\package-lock.json changed since the last npm ci - " 'Yellow'), (New-Seg '[i]' 'Cyan'), (New-Seg ' reinstalls the packages' 'Yellow'))
    }
    @(New-Seg '')
}

function Get-HeaderSegments {
    # The lazyrunner chip shimmers while something is building; a sparkle banner celebrates "ready".
    $chipBack = if (($Ctl.Building -or $Ctl.NpmCi) -and $Ctl.Frame % 8 -lt 4) { 'Blue' } else { 'Cyan' }
    $summary = if ($Ctl.CelebrateUntil -and [datetime]::UtcNow -lt $Ctl.CelebrateUntil) {
        $spark = [string]$SparkleFrames[[int]($Ctl.Frame / 2) % 2]
        New-Seg "  $spark $($Ctl.CelebrateText) $spark" 'Green'
    }
    else { New-Seg "  $($Ctl.Summary.Text)" $Ctl.Summary.Color }
    @(
        (New-Seg ' lazyrunner ' 'Black' $chipBack), $summary,
        (New-Seg "   ⎇ $($Ctl.Branch)" 'Magenta'), (New-Seg '   DB ' 'DarkGray'), (New-Seg ($Ctl.Db ?? 'n/a') 'Yellow'),
        (New-Seg "   $($Ctl.Root)" 'DarkGray')
    )
}

function Get-PanelRows([int]$Width) {
    $rows = [Collections.Generic.List[string]]::new()
    $themeTag = if ($Ctl.ThemeId -ne 'default' -or $env:NO_COLOR) { " $($Ctl.ThemeName) " } else { '' }
    $rows.Add((ConvertTo-Row @((New-Seg ('─' * [Math]::Max(0, $Width - $themeTag.Length)) 'DarkGray'), (New-Seg $themeTag 'DarkGray')) $Width))
    $rows.Add((ConvertTo-Row (Get-HeaderSegments) $Width))
    $rows.Add((ConvertTo-Row (Get-CompactMenu $Width) $Width))
    foreach ($settingsRow in Get-SettingsRows $Width) { $rows.Add((ConvertTo-Row $settingsRow $Width)) }
    $rows.Add((ConvertTo-Row (Get-NoticeSegments) $Width))
    $count = [int]($Ctl.Config.panelLogLines ?? 5)
    $entries = @($Ctl.Log | Select-Object -Last $count)
    for ($i = 0; $i -lt $count; $i++) {
        $rows.Add($(if ($i -lt $entries.Count) {
                    ConvertTo-Row @((New-Seg " $($entries[$i].Time) " 'DarkGray'), (New-Seg $entries[$i].Text $entries[$i].Color)) $Width
                } else { ' ' * $Width }))
    }
    $rows
}

# ---------------------------------------------------------------- frame

function Show-Frame {
    $Ctl.Frame++
    Update-TerminalStatus
    $width = [Console]::WindowWidth
    $height = [Console]::WindowHeight
    if ($width -lt 60 -or $height -lt 15) {
        Write-Frame @(ConvertTo-Row @(New-Seg ' lazyrunner: make the terminal a bit bigger (60x15 at least)' 'Yellow') $width)
        return
    }
    if ($Ctl.Overlay -and $Ctl.Overlay.Kind -eq 'help') { Write-Frame @(Get-HelpRows $width $height); return }   # the whole screen
    $panel = Get-PanelRows $width
    $body = Get-BodyRows $width ($height - $panel.Count)
    Write-Frame (@($body) + @($panel))
}

function Write-Frame([string[]]$Rows) {
    $size = "$([Console]::WindowWidth)x$([Console]::WindowHeight)"
    $builder = [Text.StringBuilder]::new()
    if ($size -ne $Ctl.LastSize) { [void]$builder.Append("`e[2J"); $Ctl.PrevRows = @(); $Ctl.LastSize = $size }
    for ($i = 0; $i -lt $Rows.Count; $i++) {
        if ($i -lt $Ctl.PrevRows.Count -and $Ctl.PrevRows[$i] -ceq $Rows[$i]) { continue }
        [void]$builder.Append("`e[$($i + 1);1H").Append($Rows[$i])
    }
    if ($builder.Length) { [Console]::Out.Write($builder.ToString()); [Console]::Out.Flush() }
    $Ctl.PrevRows = $Rows
    if ($env:LAZYRUNNER_FRAME_DUMP -and ([datetime]::UtcNow - $Ctl.LastDump).TotalMilliseconds -ge 500) {
        $Ctl.LastDump = [datetime]::UtcNow
        [IO.File]::WriteAllLines($env:LAZYRUNNER_FRAME_DUMP, [string[]]@($Rows | ForEach-Object { $AnsiPattern.Replace($_, '') }))
    }
}
