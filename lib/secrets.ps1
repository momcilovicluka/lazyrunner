# Database switcher: SQL connection strings in the APIs' user secrets (secrets.json), one active per key and the
# others commented out with //. Choosing one rewrites only those lines: the chosen one active, the rest of the
# same key commented. Passwords are never shown; entries are labelled "database @ server".

$ConnectionLinePattern = [regex]'^(?<indent>\s*)(?<comment>//\s*)?"(?<key>[^"]+)"\s*:\s*"(?<value>(?:[^"\\]|\\.)*)"\s*(?<comma>,?)\s*$'

function Get-SecretsPath {
    if ($env:LAZYRUNNER_SECRETS_PATH) { return $env:LAZYRUNNER_SECRETS_PATH }
    foreach ($key in Get-AppKeys 'UsesSecrets') {
        $project = Join-Path $Ctl.Root $Apps[$key].Project
        if ((Test-Path $project) -and ((Get-Content $project -Raw) -match '<UserSecretsId>\s*([^<\s]+)\s*</UserSecretsId>')) {
            return Join-Path $env:APPDATA "Microsoft\UserSecrets\$($Matches[1])\secrets.json"
        }
    }
    $null
}

function Get-DbConnections([string]$Path) {
    if (-not $Path -or -not (Test-Path $Path)) { return }
    $lines = [IO.File]::ReadAllLines($Path)
    for ($i = 0; $i -lt $lines.Count; $i++) {
        $match = $ConnectionLinePattern.Match($lines[$i])
        if (-not $match.Success) { continue }
        $value = $match.Groups['value'].Value
        $server = if ($value -match '(?i)(?:^|;)\s*(?:server|data source|addr|address)\s*=\s*([^;]+)') { $Matches[1].Trim() }
        $database = if ($value -match '(?i)(?:^|;)\s*(?:database|initial catalog)\s*=\s*([^;]+)') { $Matches[1].Trim() }
        if (-not $server -or -not $database) { continue }   # only SQL connection strings, not storage/service bus
        [pscustomobject]@{
            Line = $i; Key = $match.Groups['key'].Value; Active = -not $match.Groups['comment'].Success
            Server = $server; Database = $database; Label = "$database @ $server"
        }
    }
}

# Groups of entries per key; ASP.NET's usual DefaultConnection (or the biggest group) first.
function Get-DbGroups([string]$Path) {
    Get-DbConnections $Path | Group-Object Key |
        Sort-Object @{ Expression = { $_.Name -eq 'DefaultConnection' }; Descending = $true }, @{ Expression = 'Count'; Descending = $true } |
        ForEach-Object { [pscustomobject]@{ Key = $_.Name; Entries = @($_.Group) } }
}

function Get-ActiveDbLabel {
    $group = @(Get-DbGroups (Get-SecretsPath))[0]
    if (-not $group) { return $null }
    $active = @($group.Entries | Where-Object Active)
    if ($active.Count -eq 1) { return $active[0].Label }
    if ($active.Count -gt 1) { return "$($active[-1].Label) (+$($active.Count - 1) more active)" }
    "none active"
}

# Adds a copy of $Entry's line with another database name, commented out (inactive), right below it.
function Add-DbConnection([string]$Path, $Entry, [string]$Database) {
    if (-not $Database.Trim() -or $Database -match '[;"\\=]') { throw "'$Database' is not a usable database name" }
    $bytes = [IO.File]::ReadAllBytes($Path)
    $bom = $bytes.Length -ge 3 -and $bytes[0] -eq 0xEF -and $bytes[1] -eq 0xBB -and $bytes[2] -eq 0xBF
    $offset = if ($bom) { 3 } else { 0 }
    $text = [Text.Encoding]::UTF8.GetString($bytes, $offset, $bytes.Length - $offset)
    $newline = if ($text.Contains("`r`n")) { "`r`n" } else { "`n" }
    $lines = [Collections.Generic.List[string]]::new([string[]]($text -split '\r?\n'))
    $source = $lines[$Entry.Line]
    $indent = $ConnectionLinePattern.Match($source).Groups['indent'].Value
    $body = ($source.Substring($indent.Length) -replace '^//\s*', '').TrimEnd()
    $body = [regex]::Replace($body, '(?i)((?:database|initial catalog)\s*=\s*)[^;"]*', { param($m) $m.Groups[1].Value + $Database.Trim() })
    if ($body -notmatch ',$') { $body += ',' }
    $lines.Insert($Entry.Line + 1, "$indent//$body")
    $newText = $lines -join $newline
    $options = [Text.Json.JsonDocumentOptions]@{ CommentHandling = [Text.Json.JsonCommentHandling]::Skip; AllowTrailingCommas = $true }
    try { [Text.Json.JsonDocument]::Parse($newText, $options).Dispose() }
    catch { throw "secrets.json would not be valid JSON with the new line ($($_.Exception.Message)) - left unchanged" }
    Copy-Item $Path "$Path.lazyrunner-backup" -Force
    [IO.File]::WriteAllText($Path, $newText, [Text.UTF8Encoding]::new($bom))
}

function Set-DbConnection([string]$Path, $Entry) {
    $bytes = [IO.File]::ReadAllBytes($Path)
    $bom = $bytes.Length -ge 3 -and $bytes[0] -eq 0xEF -and $bytes[1] -eq 0xBB -and $bytes[2] -eq 0xBF
    $offset = if ($bom) { 3 } else { 0 }
    $text = [Text.Encoding]::UTF8.GetString($bytes, $offset, $bytes.Length - $offset)
    $newline = if ($text.Contains("`r`n")) { "`r`n" } else { "`n" }
    $lines = $text -split '\r?\n'

    foreach ($other in @(Get-DbConnections $Path | Where-Object Key -eq $Entry.Key)) {
        $indent = $ConnectionLinePattern.Match($lines[$other.Line]).Groups['indent'].Value
        $body = $lines[$other.Line].Substring($indent.Length) -replace '^//\s*', ''
        $lines[$other.Line] = if ($other.Line -eq $Entry.Line) { "$indent$body" } else { "$indent//$body" }
    }
    # The newly active line needs a comma when another active property follows it.
    $following = if ($Entry.Line -lt $lines.Count - 1) { $lines[($Entry.Line + 1)..($lines.Count - 1)] } else { @() }
    $followed = @($following | Where-Object { $_.Trim() -and $_.Trim() -notmatch '^(//|\})' }).Count -gt 0
    if ($followed -and $lines[$Entry.Line].TrimEnd() -notmatch ',$') { $lines[$Entry.Line] = $lines[$Entry.Line].TrimEnd() + ',' }

    $newText = $lines -join $newline
    $options = [Text.Json.JsonDocumentOptions]@{ CommentHandling = [Text.Json.JsonCommentHandling]::Skip; AllowTrailingCommas = $true }
    try { [Text.Json.JsonDocument]::Parse($newText, $options).Dispose() }
    catch { throw "secrets.json would not be valid JSON after the switch ($($_.Exception.Message)) - left unchanged" }
    Copy-Item $Path "$Path.lazyrunner-backup" -Force
    [IO.File]::WriteAllText($Path, $newText, [Text.UTF8Encoding]::new($bom))
}
