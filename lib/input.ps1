# Keyboard: vim-style navigation and search, panel keys (from the action list in actions.ps1), focus/scroll/
# width, settings toggles and the database picker.

$ChordMs = 600   # gg: the second g has to follow within this time

function Read-UiKeys {
    while (-not $Ctl.Quit -and [Console]::KeyAvailable) { $null = Invoke-UiKey ([Console]::ReadKey($true)) }
}

# 'a', 'A', '/', 'Space', 'Ctrl+L', 'Enter', 'Escape', 'UpArrow', 'Shift+Tab'... Letters stay case-sensitive.
function Get-KeyName([ConsoleKeyInfo]$Key) {
    $ctrl = $Key.Modifiers -band [ConsoleModifiers]::Control
    if ($ctrl -and $Key.Key -ge [ConsoleKey]::A -and $Key.Key -le [ConsoleKey]::Z) { return "Ctrl+$($Key.Key)" }
    if ($Key.KeyChar -eq ' ') { return 'Space' }
    if ($Key.KeyChar -and -not [char]::IsControl($Key.KeyChar)) { return [string]$Key.KeyChar }
    $shift = if ($Key.Modifiers -band [ConsoleModifiers]::Shift) { 'Shift+' } else { '' }
    "$shift$($Key.Key)"
}

# With -Busy (during a dotnet build) only safe actions run; S (stop all) and q (quit) are returned so the build
# can be cancelled. npm ci runs in the background and blocks nothing.
function Invoke-UiKey([ConsoleKeyInfo]$Key, [switch]$Busy) {
    if ($Key.KeyChar -eq [char]3) { if ($Busy) { return 'quit' }; $Ctl.Quit = $true; return }   # Ctrl+C
    if ($Ctl.Overlay) { Invoke-OverlayKey $Key -Busy:$Busy; return }
    if ($Ctl.Picker) { Invoke-PickerKey $Key; return }   # also during a build: builds don't read secrets.json
    if ($Ctl.Search -and $Ctl.Search.Typing) { Invoke-SearchKey $Key; return }
    $name = Get-KeyName $Key
    $chord = $Ctl.PendingG -and ([datetime]::UtcNow - $Ctl.PendingG).TotalMilliseconds -lt $ChordMs
    $Ctl.PendingG = $null
    if ($name -ceq 'g') { if ($chord) { Move-Scroll $Ctl.Apps[$Ctl.Focus].Lines.Count } else { $Ctl.PendingG = [datetime]::UtcNow }; return }
    if (Invoke-NavigationKey $name) { return }
    if ($Busy) {
        if ($name -ceq 'S') { return 'stop' }
        if ($name -in 'q', 'Q') { return 'quit' }
    }
    $action = Find-Action $name
    if (-not $action) { return }
    if ($Busy -and -not $action.Safe) { Add-Log "Building - press S to cancel it first, then: $($action.Label)" 'Yellow'; return }
    & $action.Run
}

function Get-PageSize { [Math]::Max(2, [Console]::WindowHeight - 16) }

# Moving around: h/l, Tab and ←→ change the column, 1-3 jump to one; j/k, Ctrl+u/d, Ctrl+b/f, PgUp/PgDn, gg/G,
# Home/End scroll; n/N step through search matches; Esc clears the search.
function Invoke-NavigationKey([string]$Name) {
    $page = Get-PageSize
    switch -CaseSensitive ($Name) {
        { $_ -cin 'l', 'Tab', 'RightArrow' } { Move-Focus 1; return $true }
        { $_ -cin 'h', 'Shift+Tab', 'LeftArrow' } { Move-Focus -1; return $true }
        { $_ -cin 'k', 'UpArrow' } { Move-Scroll 1; return $true }
        { $_ -cin 'j', 'DownArrow' } { Move-Scroll -1; return $true }
        'Ctrl+U' { Move-Scroll ([int]($page / 2)); return $true }
        'Ctrl+D' { Move-Scroll (-[int]($page / 2)); return $true }
        { $_ -in 'Ctrl+B', 'PageUp' } { Move-Scroll $page; return $true }
        { $_ -in 'Ctrl+F', 'PageDown' } { Move-Scroll (-$page); return $true }
        'Home' { Move-Scroll $Ctl.Apps[$Ctl.Focus].Lines.Count; return $true }
        { $_ -cin 'G', 'End' } { $Ctl.Apps[$Ctl.Focus].Scroll = 0; return $true }
        'n' { Find-Match 'older'; return $true }
        'N' { Find-Match 'newer'; return $true }
        'Escape' { if ($Ctl.Search) { $Ctl.Search = $null; Clear-RenderCache }; return $true }
        { $_ -in '1', '2', '3', '4', '5', '6', '7', '8', '9' } {
            $keys = @($Apps.Keys)
            $index = [int]$_ - 1
            if ($index -lt $keys.Count) { $Ctl.Focus = $keys[$index] }
            return $true
        }
    }
    $false
}

# Simulated keys (request action "keys"), e.g. for scripted tests: "d", "j", "Space", "Ctrl+L", "Enter".
function ConvertTo-KeyInfo([string]$Name) {
    if ($Name -match '^Ctrl\+([A-Za-z])$') {
        $letter = $Matches[1].ToUpperInvariant()
        return [ConsoleKeyInfo]::new([char]([int][char]$letter - 64), [ConsoleKey]$letter, $false, $false, $true)
    }
    if ($Name -eq 'Space') { return [ConsoleKeyInfo]::new(' ', [ConsoleKey]::Spacebar, $false, $false, $false) }
    if ($Name.Length -eq 1) {
        $key = if ($Name -match '[a-zA-Z]') { [ConsoleKey]$Name.ToUpperInvariant() } elseif ($Name -match '\d') { [ConsoleKey]"D$Name" } else { [ConsoleKey]::NoName }
        return [ConsoleKeyInfo]::new($Name[0], $key, $Name -cmatch '[A-Z]', $false, $false)
    }
    $shift = $Name.StartsWith('Shift+')
    $consoleKey = [ConsoleKey]($Name -replace '^Shift\+', '')
    $char = switch ($consoleKey) { 'Enter' { "`r" } 'Escape' { [string][char]27 } 'Tab' { "`t" } 'Backspace' { [string][char]8 } default { [string][char]0 } }
    [ConsoleKeyInfo]::new($char[0], $consoleKey, $shift, $false, $false)
}

# ---------------------------------------------------------------- run actions on the focused app

# Space: stop the focused app if it is running, building, queued or installing; start it otherwise.
function Switch-FocusedApp {
    $key = $Ctl.Focus
    $busy = (Test-AppRunning $key) -or ($Ctl.Building -and $Ctl.Building.Key -eq $key) -or $key -in $Ctl.BuildQueue -or ($Ctl.NpmCi -and $Ctl.NpmCi.Key -eq $key)
    if ($busy) { Invoke-StopFocused } else { Invoke-Start -Only $Ctl.Focus }
}

function Open-Focused {
    if (Test-AppTrait $Ctl.Focus 'Variants') { Open-AppPage $Ctl.Focus; return }
    $url = "$(Get-AppUrl $Ctl.Focus)$($Apps[$Ctl.Focus].Swagger)"
    Start-Process $url
    Add-Log "Opened $url"
}

# ---------------------------------------------------------------- search (/ then n older, N newer)

function Open-Search { $Ctl.Search = @{ Typing = $true; Query = ''; Key = $Ctl.Focus; Index = -1; Note = '' } }

function Invoke-SearchKey([ConsoleKeyInfo]$Key) {
    $search = $Ctl.Search
    switch (Get-KeyName $Key) {
        'Escape' { $Ctl.Search = $null; Clear-RenderCache; return }
        'Backspace' { if ($search.Query) { $search.Query = $search.Query.Substring(0, $search.Query.Length - 1) }; return }
        'Enter' {
            $search.Typing = $false
            if (-not $search.Query) { $Ctl.Search = $null; return }
            Find-Match 'older'
            return
        }
    }
    if (-not [char]::IsControl($Key.KeyChar)) { $search.Query += $Key.KeyChar; Clear-RenderCache }
}

function Test-LineMatch($Line, [string]$Query) { $Line.Text.IndexOf($Query, [StringComparison]::OrdinalIgnoreCase) -ge 0 }

# Steps to the next match above ('older') or below ('newer') the current one - or, the first time, the bottom of
# what the column shows - wrapping around at the ends, and scrolls it into the middle of the column.
function Find-Match([ValidateSet('older', 'newer')][string]$Direction) {
    $search = $Ctl.Search
    if (-not $search -or -not $search.Query) { Add-Log 'Press / to search the focused column first.' 'DarkGray'; return }
    if ($search.Key -ne $Ctl.Focus) { $search.Key = $Ctl.Focus; $search.Index = -1 }
    $state = $Ctl.Apps[$Ctl.Focus]
    $lines = $state.Lines
    $count = $lines.Count
    $start = if ($search.Index -ge 0 -and $search.Index -lt $count) { $search.Index } elseif ($Direction -eq 'older') { $count - $state.Scroll } else { -1 }
    $step = if ($Direction -eq 'older') { -1 } else { 1 }
    $found = -1
    $search.Note = ''
    for ($n = 1; $n -le $count; $n++) {
        $i = (($start + $step * $n) % $count + $count) % $count
        if (Test-LineMatch $lines[$i] $search.Query) {
            $found = $i
            if (($step -lt 0 -and $i -gt $start) -or ($step -gt 0 -and $i -lt $start)) { $search.Note = 'wrapped' }
            break
        }
    }
    if ($found -lt 0) { $search.Note = 'no match'; $search.Total = 0; return }
    $search.Index = $found
    $search.Total = @($lines | Where-Object { Test-LineMatch $_ $search.Query }).Count
    $search.Position = @(for ($i = $found; $i -lt $count; $i++) { if (Test-LineMatch $lines[$i] $search.Query) { 1 } }).Count   # 1 = newest
    $state.Scroll = [int][Math]::Max(0, [Math]::Min($count - 1, $count - 1 - $found - [int](($Ctl.BodyRows ?? 20) / 2)))
    Clear-RenderCache
}

function Clear-RenderCache { foreach ($state in $Ctl.Apps.Values) { $state.Render = $null } }

# ---------------------------------------------------------------- focus, scroll, width

function Move-Focus([int]$Step) {
    $keys = @($Apps.Keys)
    $index = [array]::IndexOf($keys, $Ctl.Focus)
    $Ctl.Focus = $keys[($index + $Step + $keys.Count) % $keys.Count]
}

function Move-Scroll([long]$Lines) {
    $state = $Ctl.Apps[$Ctl.Focus]
    $state.Scroll = [int][Math]::Max(0, [Math]::Min([Math]::Max(0, $state.Lines.Count - 1), $state.Scroll + $Lines))
}

function Resize-Focus([int]$Delta) {
    $current = (Get-Config).columnWidths ?? @{}
    $widths = @{}
    foreach ($key in $Apps.Keys) { $widths[$key] = [double]($current[$key] ?? 33) }
    $widths[$Ctl.Focus] = [Math]::Max(5, [Math]::Min(90, $widths[$Ctl.Focus] + $Delta))
    $Ctl.Config = Set-ConfigValue 'columnWidths' $widths
}

function Reset-ColumnWidths {
    $Ctl.Config = Set-ConfigValue 'columnWidths' $ConfigDefaults.columnWidths.Clone()
    Add-Log 'Column widths reset to equal (saved)' 'Cyan'
}

function Switch-Setting([string]$Name, [string]$Label) {
    $Ctl.Config = Set-ConfigValue $Name (-not (Get-Config)[$Name])
    Add-Log "${Label}: $(if ($Ctl.Config[$Name]) { 'on' } else { 'off' }) (saved)" 'Cyan'
}

# ---------------------------------------------------------------- open things

function Get-AppUrl([string]$Key) {
    # A watch build has no port of its own: the app its variant names in servedBy serves it.
    $port = if ($Ctl.Ports[$Key]) { $Ctl.Ports[$Key] } else { $Ctl.Ports[$Ctl.Modes[$Key].ServedBy] }
    "http://localhost:$port"
}

function Open-Swagger {
    foreach ($key in @($Apps.Keys | Where-Object { $Apps[$_].Swagger })) { Start-Process "$(Get-AppUrl $key)$($Apps[$key].Swagger)" }
    Add-Log 'Opened Swagger for the APIs.'
}

# An npm app's page: its dev server, or the app that serves its watch build.
function Open-AppPage([string]$Key) {
    if (-not $Ctl.Modes[$Key]) { Add-Log "$($Apps[$Key].Name) doesn't run on this branch ($(Get-NoModeReason $Ctl.Root $Key))."; return }
    Start-Process (Get-AppUrl $Key)
    Add-Log "Opened $($Apps[$Key].Name) ($(Get-AppUrl $Key))."
}

function Open-AppLog {
    $state = $Ctl.Apps[$Ctl.Focus]
    $path = if ($Ctl.LastBuildFailed -and $Ctl.LastFailedLog -and (Test-Path $Ctl.LastFailedLog)) { $Ctl.LastFailedLog } else { $state.LogPath }
    if ($state.Writer) { $state.Writer.Flush() }
    if (-not $path -or -not (Test-Path $path)) { Add-Log 'No log yet.'; return }
    try { Invoke-Item $path } catch { Start-Process notepad.exe $path }
    Add-Log "Opened $path"
}

function Clear-Panes {
    foreach ($state in $Ctl.Apps.Values) { $state.Lines.Clear(); $state.Scroll = 0; $state.Version++ }
    $Ctl.Log.Clear()
    $Ctl.Search = $null
}

# ---------------------------------------------------------------- database picker

function Open-Picker {
    $path = Get-SecretsPath
    $groups = @(Get-DbGroups $path)
    if (-not $groups) { Add-Log "No SQL connection strings found in $path" 'Yellow'; return }
    $Ctl.Picker = @{ Path = $path; Groups = $groups; Group = 0; Index = (Get-ActiveIndex $groups[0]); Top = 0; Filter = ''; Filtering = $false }
}

function Get-ActiveIndex($Group) { [Math]::Max(0, [array]::IndexOf([object[]]@($Group.Entries | ForEach-Object Active), $true)) }

# The current key's entries, narrowed by the / filter.
function Get-PickerEntries($Picker) {
    $entries = @($Picker.Groups[$Picker.Group].Entries)
    if (-not $Picker.Filter) { return $entries }
    @($entries | Where-Object { $_.Label.IndexOf($Picker.Filter, [StringComparison]::OrdinalIgnoreCase) -ge 0 })
}

function Invoke-PickerKey([ConsoleKeyInfo]$Key) {
    $picker = $Ctl.Picker
    if ($null -ne $picker.NewName) { Invoke-PickerNameKey $Key; return }
    if ($picker.Filtering) { Invoke-PickerFilterKey $Key; return }
    $count = @(Get-PickerEntries $picker).Count
    switch -CaseSensitive (Get-KeyName $Key) {
        { $_ -cin 'n', '+', 'Insert' } { $picker.NewName = '' }   # add a new database name
        '/'           { $picker.Filtering = $true }
        { $_ -cin 'Escape', 'q', 'd' } { if ($picker.Filter) { $picker.Filter = ''; $picker.Index = 0 } else { $Ctl.Picker = $null } }
        { $_ -cin 'k', 'UpArrow' }   { $picker.Index = [Math]::Max(0, $picker.Index - 1) }
        { $_ -cin 'j', 'DownArrow' } { $picker.Index = [Math]::Min($count - 1, $picker.Index + 1) }
        { $_ -cin 'Ctrl+U', 'PageUp' }   { $picker.Index = [Math]::Max(0, $picker.Index - 10) }
        { $_ -cin 'Ctrl+D', 'PageDown' } { $picker.Index = [Math]::Min($count - 1, $picker.Index + 10) }
        'Home'        { $picker.Index = 0 }
        { $_ -cin 'G', 'End' } { $picker.Index = $count - 1 }
        { $_ -cin 'h', 'l', 'LeftArrow', 'RightArrow' } {
            $step = if ($_ -cin 'l', 'RightArrow') { 1 } else { -1 }
            $picker.Group = ($picker.Group + $step + $picker.Groups.Count) % $picker.Groups.Count
            $picker.Filter = ''
            $picker.Index = Get-ActiveIndex $picker.Groups[$picker.Group]
            $picker.Top = 0
        }
        'Enter'       { if ($count) { Select-Database @(Get-PickerEntries $picker)[$picker.Index] } }
    }
}

function Invoke-PickerFilterKey([ConsoleKeyInfo]$Key) {
    $picker = $Ctl.Picker
    switch (Get-KeyName $Key) {
        'Escape'    { $picker.Filter = ''; $picker.Filtering = $false; $picker.Index = Get-ActiveIndex $picker.Groups[$picker.Group]; return }
        'Enter'     { $picker.Filtering = $false; return }
        'Backspace' { if ($picker.Filter) { $picker.Filter = $picker.Filter.Substring(0, $picker.Filter.Length - 1) }; $picker.Index = 0; return }
    }
    if (-not [char]::IsControl($Key.KeyChar)) { $picker.Filter += $Key.KeyChar; $picker.Index = 0; $picker.Top = 0 }
}

# Typing the name for "n": a copy of the selected connection string pointing at another database.
function Invoke-PickerNameKey([ConsoleKeyInfo]$Key) {
    $picker = $Ctl.Picker
    switch ($Key.Key) {
        'Escape'    { $picker.NewName = $null; return }
        'Backspace' { if ($picker.NewName) { $picker.NewName = $picker.NewName.Substring(0, $picker.NewName.Length - 1) }; return }
        'Enter' {
            $source = @(Get-PickerEntries $picker)[$picker.Index]
            $name = $picker.NewName
            $picker.NewName = $null
            if (-not $name -or -not $source) { return }
            try { Add-DbConnection $picker.Path $source $name }
            catch { Add-Log "✗ $($_.Exception.Message)" 'Red'; return }
            Add-Log "Added $name @ $($source.Server) to secrets.json (inactive) - Enter switches to it" 'Cyan'
            $picker.Groups = @(Get-DbGroups $picker.Path)
            $picker.Filter = ''
            $entries = @(Get-PickerEntries $picker)
            $picker.Index = [Math]::Max(0, [array]::IndexOf([object[]]@($entries | ForEach-Object Line), $source.Line + 1))
            return
        }
    }
    if (-not [char]::IsControl($Key.KeyChar)) { $picker.NewName += $Key.KeyChar }
}

function Select-Database($Entry) {
    $path = $Ctl.Picker.Path
    $Ctl.Picker = $null
    if ($Entry.Active -and @(Get-DbConnections $path | Where-Object { $_.Key -eq $Entry.Key -and $_.Active }).Count -eq 1) {
        Add-Log "Already using $($Entry.Label)."
        return
    }
    try { Set-DbConnection $path $Entry }
    catch { Add-Log "✗ $($_.Exception.Message)" 'Red'; return }
    Update-DbLabel
    Add-Log "Database → $($Entry.Label)  ($($Entry.Key); previous file saved as secrets.json.lazyrunner-backup)" 'Cyan'
    # Running (or still starting) APIs restart; ones building or waiting to build start on the new database anyway.
    $apis = @(Get-AppKeys 'UsesSecrets' | Where-Object { Test-AppRunning $_ })
    if ($apis) { Restart-ApiProcesses $apis 'database switched' }
}
