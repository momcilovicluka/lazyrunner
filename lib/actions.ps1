# One list of every panel action. It drives the single-key shortcuts, the : command palette, the x menu and the ?
# key reference, so they never drift apart. Keys follow vim and lazygit/lazydocker and are case-sensitive:
# lowercase acts on the focused column, uppercase on every app. Safe actions also work while a build is running.

function Get-Actions {
    $installs = [bool](Get-AppKeys 'Installs')
    @(
        @{ Group = 'Run'; Key = 'Space'; Label = 'Start / stop the focused app'; Hint = 'whichever it is not'; Run = { Switch-FocusedApp }; Safe = $true; Focused = $true }
        @{ Group = 'Run'; Key = 'u'; Label = 'Start the focused app'; Hint = 'builds it first if it changed'; Run = { Invoke-Start -Only $Ctl.Focus }; Safe = $true; Focused = $true }
        @{ Group = 'Run'; Key = 's'; Label = 'Stop the focused app'; Hint = 'or cancel its build / npm ci'; Run = { Invoke-StopFocused }; Safe = $true; Focused = $true }
        @{ Group = 'Run'; Key = 'r'; Label = 'Restart the focused app'; Hint = 'rebuild if it changed'; Run = { Invoke-Restart 'requested from the panel' -Only $Ctl.Focus }; Focused = $true }
        @{ Group = 'Run'; Key = 'b'; Label = 'Rebuild the focused app'; Hint = 'restart with a full build'; Run = { Invoke-Restart 'full rebuild' -Only $Ctl.Focus -ForceBuild }; Focused = $true }
        @{ Group = 'Run'; Key = 'U'; Label = 'Start all'; Hint = 'build if needed, start whatever is stopped'; Run = { Invoke-Start } }
        @{ Group = 'Run'; Key = 'S'; Label = 'Stop all'; Hint = 'also cancels a running build'; Run = { Invoke-StopAll } }
        @{ Group = 'Run'; Key = 'R'; Label = 'Restart all'; Hint = 'rebuild if anything changed'; Run = { Invoke-Restart 'requested from the panel' } }
        @{ Group = 'Run'; Key = 'B'; Label = 'Rebuild all'; Hint = 'restart with a full build, never skipped'; Run = { Invoke-Restart 'full rebuild' -ForceBuild } }
        @{ Group = 'Run'; Key = '!'; Label = 'Kill whatever holds a needed port'; Run = { Resolve-PortConflicts }; Show = [bool]$Ctl.Conflicts }
        @{ Group = 'Run'; Key = 'q'; Alt = @('Q'); Label = 'Quit'; Hint = 'stop everything and exit'; Run = { $Ctl.Quit = $true } }
        @{ Group = 'Tools'; Key = 'd'; Label = 'Switch database'; Hint = 'connection strings in secrets.json'; Run = { Open-Picker }; Safe = $true }
        @{ Group = 'Tools'; Key = 'i'; Label = 'npm ci (reinstall packages)'; Hint = "the focused npm app's, in the background"; Run = { Invoke-NpmCi }; Show = $installs; Safe = $true }
        @{ Group = 'Tools'; Key = 'w'; Label = 'Open the focused app in the browser'; Hint = 'Swagger for an API'; Run = { Open-Focused }; Safe = $true; Focused = $true }
        @{ Group = 'Tools'; Key = 'W'; Label = 'Open everything in the browser'; Hint = 'every Swagger and npm app'; Run = { Open-Swagger; foreach ($key in Get-AppKeys 'Variants') { Open-AppPage $key } }; Safe = $true }
        @{ Group = 'Tools'; Key = 'o'; Label = "Open the focused app's log file"; Hint = 'its full output'; Run = { Open-AppLog }; Safe = $true; Focused = $true }
        @{ Group = 'Tools'; Key = 'x'; Label = 'Menu for the focused app'; Run = { Open-Overlay 'menu' }; Safe = $true }
        @{ Group = 'View'; Key = '/'; Label = "Search the focused app's output"; Hint = 'n older match, N newer'; Run = { Open-Search }; Safe = $true; Focused = $true }
        @{ Group = 'View'; Key = 'z'; Label = 'Zoom the focused column'; Run = { $Ctl.Zoom = -not $Ctl.Zoom }; Safe = $true; Focused = $true }
        @{ Group = 'View'; Key = '>'; Label = 'Widen the focused column'; Run = { Resize-Focus 5 }; Safe = $true }
        @{ Group = 'View'; Key = '<'; Label = 'Narrow the focused column'; Run = { Resize-Focus -5 }; Safe = $true }
        @{ Group = 'View'; Key = '='; Label = 'Equal column widths'; Run = { Reset-ColumnWidths }; Safe = $true }
        @{ Group = 'View'; Key = 'Ctrl+L'; Label = 'Clear output'; Run = { Clear-Panes }; Safe = $true }
        @{ Group = 'View'; Key = 't'; Label = "Theme: $($Ctl.ThemeName) → next"; Hint = "$($Themes.Count) in themes\ - add your own .json"; Run = { Switch-Theme }; Safe = $true }
        @{ Group = 'Help'; Key = ','; Label = 'Settings (a u s e w toggles, r checklist)'; Hint = 'auto-restart, auto-start, checklist…'; Run = { Open-Overlay 'settings' }; Safe = $true }
        @{ Group = 'Help'; Key = '?'; Label = 'Key reference'; Run = { Open-Overlay 'help' }; Safe = $true }
        @{ Group = 'Help'; Key = ':'; Alt = @('Ctrl+P'); Label = 'Command palette'; Hint = 'also Ctrl+P'; Run = { Open-Overlay 'palette' }; Safe = $true }
    )
}

# The on/off settings and the checklist, in the , menu (same letters as the old panel toggles).
function Get-SettingActions {
    $check = { param($On) if ($On) { '[x]' } else { '[ ]' } }
    $config = $Ctl.Config
    @(
        @{ Key = 'a'; On = $config.autoRestartOnBranchChange; Label = 'Auto-restart on branch change'; Run = { Switch-Setting 'autoRestartOnBranchChange' 'Auto-restart on branch change' }; Safe = $true }
        @{ Key = 'u'; On = $config.autoStartOnOpen; Label = 'Auto-start when lazyrunner opens'; Run = { Switch-Setting 'autoStartOnOpen' 'Auto-start when lazyrunner opens' }; Safe = $true }
        @{ Key = 's'; On = $config.stopFrontendOnStop; Label = 'Stop/Restart all include the frontend'; Run = { Switch-Setting 'stopFrontendOnStop' 'Stop/Restart all include the frontend' }; Show = [bool](Get-AppKeys 'Frontend'); Safe = $true }
        @{ Key = 'e'; On = $config.restartOnCodeChange; Label = 'Restart the APIs when code changes'; Run = { Switch-Setting 'restartOnCodeChange' 'Restart the APIs when code changes' }; Safe = $true }
        @{ Key = 'w'; On = $config.startupChecklist; Label = 'Startup checklist when lazyrunner opens'; Run = { Switch-Setting 'startupChecklist' 'Startup checklist when lazyrunner opens' }; Safe = $true }
        @{ Key = 'r'; Label = 'Run the startup checklist now'; Run = { $Ctl.Overlay = $null; Invoke-Checklist -Manual } }
        @{ Key = 'p'; Label = "Reload the project profile$(if ($Ctl.ProfileChanged) { ' (this branch changed it)' })"; Run = { Invoke-ReloadProfile } }
    ) | Where-Object { $_.Show -ne $false } | ForEach-Object {
        if ($null -ne $_.On) { $_.Label = "$(& $check $_.On) $($_.Label)" }
        $_
    }
}

function Find-Action([string]$Name) {
    Get-Actions | Where-Object { $_.Show -ne $false -and ($_.Key -ceq $Name -or $_.Alt -ccontains $Name) } | Select-Object -First 1
}

function Open-Overlay([string]$Kind) { $Ctl.Overlay = @{ Kind = $Kind; Query = ''; Index = 0 } }

# What the open overlay lists: the x menu (actions for the focused app) or the , settings.
function Get-OverlayItems([string]$Kind) {
    switch ($Kind) {
        'menu' {
            $items = @(Get-Actions | Where-Object { $_.Focused -and $_.Show -ne $false })
            if (Test-AppTrait $Ctl.Focus 'Installs') { $items += @(Get-Actions | Where-Object Key -ceq 'i') }
            return $items
        }
        'settings' { return @(Get-SettingActions) }
    }
    @()
}

# Fuzzy subsequence match: every typed character in order; consecutive and word-start hits score higher.
function Get-FuzzyScore([string]$Query, [string]$Text) {
    $text = $Text.ToLowerInvariant()
    $score = 0
    $from = 0
    $previous = -2
    foreach ($char in $Query.ToLowerInvariant().ToCharArray()) {
        if ($char -eq ' ') { continue }
        $found = $text.IndexOf($char, $from)
        if ($found -lt 0) { return 0 }
        $score += 1
        if ($found -eq $previous + 1) { $score += 3 }
        if ($found -eq 0 -or $text[$found - 1] -in ' ', '-', '/', '(', '[', '\') { $score += 2 }
        $previous = $found
        $from = $found + 1
    }
    $score
}

# Every action plus the settings (shown as ", a" etc.), best matches first.
function Get-PaletteMatches([string]$Query) {
    $settings = @(Get-SettingActions | ForEach-Object { @{ Group = 'Settings'; Key = ", $($_.Key)"; Label = $_.Label; Run = $_.Run; Safe = $_.Safe } })
    $actions = @(Get-Actions | Where-Object { $_.Show -ne $false -and $_.Key -ne ':' }) + $settings
    if (-not $Query.Trim()) { return $actions }
    $scored = foreach ($action in $actions) {
        $label = Get-FuzzyScore $Query $action.Label
        $score = if ($label) { $label + 10 } else { Get-FuzzyScore $Query "$($action.Label) $($action.Group) $($action.Hint) $($action.Key)" }
        if ($score -gt 0) { [pscustomobject]@{ Action = $action; Score = $score } }
    }
    @($scored | Sort-Object Score -Descending | ForEach-Object Action)
}

function Invoke-OverlayAction($Action, [switch]$Busy, [switch]$KeepOpen) {
    if (-not $KeepOpen) { $Ctl.Overlay = $null }
    if (-not $Action) { return }
    if ($Busy -and -not $Action.Safe) { Add-Log "Building - press S to cancel it first, then: $($Action.Label)" 'Yellow'; return }
    if (-not $KeepOpen) { Add-Log "▸ $($Action.Label)" 'DarkGray' }
    & $Action.Run
}

# Keys while an overlay is open: the palette types to filter; the x and , menus run an item by its key.
function Invoke-OverlayKey([ConsoleKeyInfo]$Key, [switch]$Busy) {
    $overlay = $Ctl.Overlay
    if ($overlay.Kind -eq 'checklist') { $overlay.Skip = $true; return }
    if ($overlay.Kind -eq 'help') { $Ctl.Overlay = $null; return }   # any key closes the reference
    $name = Get-KeyName $Key
    if ($overlay.Kind -eq 'palette') { Invoke-PaletteKey $overlay $name $Key -Busy:$Busy; return }
    $items = @(Get-OverlayItems $overlay.Kind)
    $overlay.Index = [Math]::Max(0, [Math]::Min($overlay.Index, $items.Count - 1))
    $closeKey = if ($overlay.Kind -eq 'menu') { 'x' } else { ',' }
    switch -CaseSensitive ($name) {
        { $_ -in 'Escape', 'q', $closeKey } { $Ctl.Overlay = $null; return }
        { $_ -in 'j', 'DownArrow', 'Ctrl+N' } { $overlay.Index = [Math]::Min($items.Count - 1, $overlay.Index + 1); return }
        { $_ -in 'k', 'UpArrow', 'Ctrl+P' } { $overlay.Index = [Math]::Max(0, $overlay.Index - 1); return }
        { $_ -in 'Enter', 'Space' } { Invoke-OverlayAction $items[$overlay.Index] -Busy:$Busy -KeepOpen:($overlay.Kind -eq 'settings'); return }
    }
    $item = $items | Where-Object { $_.Key -ceq $name } | Select-Object -First 1
    if ($item) { Invoke-OverlayAction $item -Busy:$Busy -KeepOpen:($overlay.Kind -eq 'settings') }
}

function Invoke-PaletteKey($Overlay, [string]$Name, [ConsoleKeyInfo]$Key, [switch]$Busy) {
    switch ($Name) {
        'Escape' { $Ctl.Overlay = $null; return }
        { $_ -in 'UpArrow', 'Ctrl+P' } { $Overlay.Index = [Math]::Max(0, $Overlay.Index - 1); return }
        { $_ -in 'DownArrow', 'Ctrl+N' } { $Overlay.Index++; return }
        'Backspace' {
            if ($Overlay.Query) { $Overlay.Query = $Overlay.Query.Substring(0, $Overlay.Query.Length - 1) }
            $Overlay.Index = 0
            return
        }
        'Enter' { Invoke-OverlayAction @(Get-PaletteMatches $Overlay.Query)[$Overlay.Index] -Busy:$Busy; return }
    }
    if (-not [char]::IsControl($Key.KeyChar)) { $Overlay.Query += $Key.KeyChar; $Overlay.Index = 0 }
}

# Re-reads themes\ first, so a new or edited theme file shows up without restarting lazyrunner.
function Switch-Theme {
    foreach ($warning in Import-Themes) { Add-Log $warning 'Yellow' }
    $names = @($Themes.Keys)
    $current = if ($Ctl.Config.theme) { $Ctl.Config.theme } else { 'default' }   # not ThemeId: an unknown saved id falls back
    $next = $names[([array]::IndexOf($names, $current) + 1) % $names.Count]
    $Ctl.Config = Set-ConfigValue 'theme' $next
    Set-Theme $next
    Add-Log "Theme: $($Ctl.ThemeName)$(if ($Ctl.Theme.Description) { " - $($Ctl.Theme.Description)" })$(if ($env:NO_COLOR) { ' (NO_COLOR is set, so colours stay off)' }) (saved)" 'Cyan'
}
