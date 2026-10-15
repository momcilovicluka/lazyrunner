# Colour themes: one JSON file per theme in themes\ (themes\nord.json is the theme "nord"). Code draws with the
# 16 console colour names plus the Focus* names; a theme maps each to an escape code. Themes are re-read on
# every T press, so a new or edited file shows up without restarting lazyrunner. themes\README.md has the format.

$PaletteNames = @(
    'Black', 'DarkRed', 'DarkGreen', 'DarkYellow', 'DarkBlue', 'DarkMagenta', 'DarkCyan', 'Gray'
    'DarkGray', 'Red', 'Green', 'Yellow', 'Blue', 'Magenta', 'Cyan', 'White'
)
$FallbackTheme = @{ Name = 'Fallback'; Description = ''; Fg = @{}; Bg = @{}; Terminal = $null }   # terminal defaults
$NoColorTheme = @{ Name = 'no-color'; Description = ''; Fg = @{}; Bg = @{}; Terminal = $null }   # NO_COLOR: backgrounds become reverse video

# "#rrggbb" → true colour, "91" / "1;91" / "38;5;208" → used as given, a palette name → that colour in $Codes.
function ConvertTo-ThemeCode([string]$Value, [ValidateSet('Fg', 'Bg')][string]$Layer, $Codes) {
    if ($Value -match '^#([0-9a-fA-F]{2})([0-9a-fA-F]{2})([0-9a-fA-F]{2})$') {
        $rgb = 1..3 | ForEach-Object { [Convert]::ToInt32($Matches[$_], 16) }
        return "$(if ($Layer -eq 'Fg') { 38 } else { 48 });2;$($rgb -join ';')"
    }
    if ($Codes -and $Codes.Contains($Value)) { return $Codes[$Value] }
    if ($Value -notmatch '^\d+(;\d+)*$') { throw "'$Value' is not #rrggbb, an SGR code like 91, or a colour name" }
    if ($Layer -eq 'Fg') { return $Value }
    # A foreground code used as a background: 38;… → 48;…, 30-37 / 90-97 → +10.
    if ($Value -match '^38;') { return "48;$($Value.Substring(3))" }
    $parts = $Value -split ';'
    $last = [int]$parts[-1]
    if ($last -in 30..37 + 90..97 + 39) { $parts[-1] = [string]($last + 10) }
    $parts -join ';'
}

function Read-ThemeFile([string]$Path) {
    $data = Get-Content $Path -Raw | ConvertFrom-Json -AsHashtable
    if ($data -isnot [hashtable]) { throw 'the file must hold one JSON object' }
    $data
}

# Builds one theme, following "extends" (every theme but default extends default unless it says otherwise).
function Resolve-Theme([string]$Id, [hashtable]$Files, [string[]]$Chain = @()) {
    if ($Chain -contains $Id) { throw "extends loops: $(($Chain + $Id) -join ' → ')" }
    $data = $Files[$Id]
    if (-not $data) { throw "extends '$Id', which isn't in the themes folder" }
    $parentId = if ($data.Contains('extends')) { $data.extends } elseif ($Id -ne 'default' -and $Files.Contains('default')) { 'default' }
    $parent = if ($parentId) { Resolve-Theme $parentId $Files ($Chain + $Id) } else { $FallbackTheme }
    $fg = @{} + $parent.Fg
    $bg = @{} + $parent.Bg
    foreach ($section in 'colors', 'foregrounds', 'backgrounds') {
        $map = $data[$section]
        if (-not $map) { continue }
        foreach ($name in $map.Keys) {
            if ($name -notin $PaletteNames) { throw "${section}: unknown colour '$name' (use: $($PaletteNames -join ', '))" }
            $value = [string]$map[$name]
            if ($section -ne 'backgrounds') { $fg[$name] = ConvertTo-ThemeCode $value 'Fg' $fg }
            if ($section -ne 'foregrounds') { $bg[$name] = ConvertTo-ThemeCode $value 'Bg' $bg }
        }
    }
    # The focused column: title bar background + text, and the heavy borders beside it.
    $focus = $data['focus'] ?? @{}
    foreach ($pair in @(('background', 'FocusBack', 'Bg'), ('text', 'FocusText', 'Fg'), ('border', 'FocusBorder', 'Fg'))) {
        $field, $name, $layer = $pair
        if (-not $focus[$field]) { continue }
        $codes = if ($layer -eq 'Fg') { $fg } else { $bg }
        $codes[$name] = ConvertTo-ThemeCode ([string]$focus[$field]) $layer $codes
    }
    $terminal = if ($data.Contains('terminal')) { $data.terminal } else { $parent.Terminal }
    foreach ($field in 'background', 'foreground') {
        if ($terminal -and $terminal[$field] -and $terminal[$field] -notmatch '^#[0-9a-fA-F]{6}$') { throw "terminal.$field must be #rrggbb" }
    }
    @{ Name = $data.name ?? $Id; Description = $data.description ?? ''; Fg = $fg; Bg = $bg; Terminal = $terminal }
}

# All themes in the folder, default first, then by display name. A broken file is skipped with a warning.
function Import-Themes {
    $files = @{}
    $warnings = [Collections.Generic.List[string]]::new()
    foreach ($file in Get-ChildItem $ThemesDir -Filter *.json -File -ErrorAction SilentlyContinue) {
        try { $files[$file.BaseName.ToLowerInvariant()] = Read-ThemeFile $file.FullName }
        catch { $warnings.Add("Theme $($file.Name) skipped: $($_.Exception.Message)") }
    }
    $resolved = @{}
    foreach ($id in $files.Keys) {
        try { $resolved[$id] = Resolve-Theme $id $files }
        catch { $warnings.Add("Theme $id.json skipped: $($_.Exception.Message)") }
    }
    $themes = [ordered]@{}
    if ($resolved.Contains('default')) { $themes['default'] = $resolved['default'] }
    foreach ($id in $resolved.Keys | Where-Object { $_ -ne 'default' } | Sort-Object { $resolved[$_].Name }) { $themes[$id] = $resolved[$id] }
    if (-not $themes.Count) { $themes['default'] = $FallbackTheme; $warnings.Add("No themes found in $ThemesDir - using plain colours") }
    $script:Themes = $themes
    $warnings
}

# OSC 10/11 recolour the terminal itself for themes that set it; OSC 110/111 put the user's colours back.
# Only once the UI owns the screen (Initialize-Terminal applies the start-up theme's); $null restores.
function Set-TerminalColors($Terminal) {
    if (-not $Ctl.Term) { return }
    $sequence = if ($Ctl.Term.Colored) { "`e]110`e\`e]111`e\" } else { '' }
    if ($Terminal.foreground) { $sequence += "`e]10;$($Terminal.foreground)`e\" }
    if ($Terminal.background) { $sequence += "`e]11;$($Terminal.background)`e\" }
    $Ctl.Term.Colored = [bool]($Terminal.foreground -or $Terminal.background)
    if ($sequence) { [Console]::Out.Write($sequence) }
}

function Set-Theme([string]$Id) {
    $Ctl.ThemeId = if ($Themes.Contains($Id)) { $Id } else { @($Themes.Keys)[0] }
    $Ctl.Theme = if ($env:NO_COLOR) { $NoColorTheme } else { $Themes[$Ctl.ThemeId] }
    $Ctl.ThemeName = $Ctl.Theme.Name
    Set-TerminalColors $Ctl.Theme.Terminal
    $Ctl.StyleCache = @{}
    $Ctl.PrevRows = @()
    foreach ($state in $Ctl.Apps.Values) { $state.Render = $null }
}
