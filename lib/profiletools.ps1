# lazyrunner profile: check a profile - its fields, the schema, and the checkout it is for (check) - and propose one for a
# checkout (init); plus the field reference in projects\README.md, made from profile.schema.json. Written so a person
# and an LLM can both write a profile with them (projects\README.md, "Writing a profile for a repo").
# Dot-sourced by lazyrunner.ps1; uses profile.ps1, apptypes.ps1 and common.ps1.

function Get-ProfileSchemaPath { Join-Path $LazyRunnerRoot 'projects\profile.schema.json' }

# lazyrunner profile check|init …: returns the exit code.
function Invoke-ProfileCommand([string]$Action, [string]$Path, [switch]$Json, [switch]$Save) {
    switch ($Action) {
        'check' { return Invoke-ProfileCheck $Path -Json:$Json }
        'init' { return Invoke-ProfileInit $Path -Save:$Save }
    }
    Write-Host 'lazyrunner profile check [file] [-Json]   check a profile: its fields, and against the checkout it is for'
    Write-Host 'lazyrunner profile init [folder] [-Save]  propose a .lazyrunner.json for a checkout (-Save writes it)'
    Write-Host 'projects\README.md describes profiles and how to write one.'
    if ($Action) { return 1 }
    0
}

# ---------------------------------------------------------------- check

# Which profile a check is about: the file given, else the one lazyrunner would use here. @{ Path; Source; Root } (Root
# $null when the checkout isn't known).
function Get-CheckTarget([string]$Path) {
    $here = (Get-Location).ProviderPath
    if (-not $Path) {
        $found = Find-ProjectProfile $here $LazyRunnerRoot
        if (-not $found) { throw "lazyrunner: no profile applies here - give a file: lazyrunner profile check <file>, or write one: lazyrunner profile init" }
        return $found
    }
    $full = (Resolve-Path -LiteralPath $Path -ErrorAction Stop).ProviderPath
    if ((Split-Path -Leaf $full) -eq $RepoProfileFile) { return @{ Path = $full; Source = 'repo'; Root = (Split-Path $full) } }
    $data = Read-ProfileData $full
    if ($data.extends -or -not $data.detect) { return @{ Path = $full; Source = 'repo'; Root = $here } }   # a draft of a repo's profile
    $marker = @($data.detect.anyFile)[0]
    $dir = $here
    while ($dir -and -not ($marker -and (Test-Path -LiteralPath (Join-Path $dir $marker)))) { $dir = Split-Path $dir -Parent }
    @{ Path = $full; Source = 'given'; Root = $dir }
}

# Every problem with a profile: @{ Level (error / warning / info); At (the field); Message }, plus the profile when it
# loads. The checkout checks need $Target.Root.
function Get-ProfileProblems($Target) {
    $problems = [Collections.Generic.List[object]]::new()
    $project = $null
    try { $project = Resolve-ProjectProfile $Target $LazyRunnerRoot }
    catch {
        foreach ($line in $_.Exception.Message -split "`n") {
            $text = $line.Trim(' ', '-')
            if (-not $text -or $text -match '^lazyrunner: .* has (a problem|\d+ problems):$') { continue }
            $text = $text -replace '^lazyrunner: ', ''
            $at = if ($text -match '^([\w$]+(\[\d+\])?( \(\w+\))?(\.\w+(\[\d+\])?)*) ') { $Matches[1] } else { '' }
            $problems.Add((New-Problem 'error' $at $text.Substring($at.Length).Trim()))
        }
        return @{ Project = $null; Problems = $problems }
    }
    $schemaErrors = $null
    if (-not (Test-Json -Path $Target.Path -SchemaFile (Get-ProfileSchemaPath) -ErrorVariable schemaErrors -ErrorAction SilentlyContinue)) {
        foreach ($schemaError in $schemaErrors) { $problems.Add((New-Problem 'error' 'schema' ($schemaError.Exception.Message -replace '^The JSON is not valid with the schema: ', ''))) }
    }
    if (-not $Target.Root) {
        $problems.Add((New-Problem 'info' 'detect' "no folder from here up has $($project.RepoMarker), so the checkout wasn't checked - run it inside the repo"))
        return @{ Project = $project; Problems = $problems }
    }
    foreach ($problem in Test-ProfileCheckout $project $Target.Root) { $problems.Add($problem) }
    @{ Project = $project; Problems = $problems }
}

# The checkout: the marker and watch folder exist, each app is set up (Test-AppSetup), and no two apps want one port.
function Test-ProfileCheckout($Project, [string]$Root) {
    $Apps = $Project.Apps   # the apps of the profile being checked, for the apptypes.ps1 functions
    if ($Project.Source -ne 'repo' -and -not (Test-Path -LiteralPath (Join-Path $Root $Project.RepoMarker))) { New-Problem 'error' 'detect.anyFile' "$($Project.RepoMarker) is not in $Root" }
    if (-not (Test-Path -LiteralPath (Join-Path $Root $Project.SourceDir))) { New-Problem 'warning' 'watch' "$($Project.SourceDir) is not a folder in $Root, so changes are never noticed" }
    $index = 0
    $ports = @{}
    foreach ($key in @($Apps.Keys)) {
        Test-AppSetup $Root $key "apps[$index] ($key)"
        $port = Get-AppPort $Root $key
        if ($port) { $ports[$port] = @(@($ports[$port]) + $Apps[$key].Name | Where-Object { $_ }) }
        $index++
    }
    foreach ($port in $ports.Keys | Where-Object { $ports[$_].Count -gt 1 }) {
        New-Problem 'error' 'apps' "$($ports[$port] -join ' and ') both use port $port on this branch - only one can listen on it"
    }
}

function Invoke-ProfileCheck([string]$Path, [switch]$Json) {
    try { $target = Get-CheckTarget $Path } catch { Write-Host $_.Exception.Message -ForegroundColor Red; return 1 }
    $result = Get-ProfileProblems $target
    $errors = @($result.Problems | Where-Object Level -eq 'error').Count
    if ($Json) {
        [ordered]@{
            file = $target.Path; source = $target.Source; root = $target.Root; ok = $errors -eq 0
            problems = @($result.Problems | ForEach-Object { [ordered]@{ level = $_.Level; at = $_.At; message = $_.Message } })
        } | ConvertTo-Json -Depth 5
        return [int]($errors -gt 0)
    }
    $where = if ($target.Root) { " for $($target.Root)" } else { '' }
    Write-Host "$($target.Path) ($($target.Source) profile$where)" -ForegroundColor Cyan
    $marks = @{ error = @('x', 'Red'); warning = @('!', 'Yellow'); info = @('-', 'DarkGray') }   # plain marks: any console shows them
    foreach ($problem in $result.Problems) {
        $mark, $color = $marks[$problem.Level]
        Write-Host "  $mark $(if ($problem.At) { "$($problem.At): " })$($problem.Message)" -ForegroundColor $color
    }
    $warnings = @($result.Problems | Where-Object Level -eq 'warning').Count
    if ($errors) { Write-Host "$errors error$(if ($errors -ne 1) { 's' })$(if ($warnings) { ", $warnings warning$(if ($warnings -ne 1) { 's' })" }) - fix and check again" -ForegroundColor Red; return 1 }
    $apps = if ($result.Project) { " - $($result.Project.Apps.Count) apps: $(@($result.Project.Apps.Values | ForEach-Object Name) -join ', ')" } else { '' }
    Write-Host "OK$(if ($warnings) { " with $warnings warning$(if ($warnings -ne 1) { 's' })" })$apps" -ForegroundColor Green
    0
}

# ---------------------------------------------------------------- init

# init's notes for the person: on the console, or on stderr when stdout (the profile) goes to a file or a pipe.
function Write-Note([string]$Text, [string]$Color = 'Gray') {
    if ([Console]::IsOutputRedirected) { [Console]::Error.WriteLine($Text) } else { Write-Host $Text -ForegroundColor $Color }
}

# The files init looks at: solutions, projects and package.json, skipping build output, packages and the like.
function Get-ScaffoldFiles([string]$Root, [int]$Depth = 6) {
    $skip = 'node_modules', 'bin', 'obj', '.git', '.vs', '.idea', 'dist', 'out', 'build', 'packages', 'artifacts', 'coverage'
    $pending = [Collections.Generic.Queue[object]]::new()
    $pending.Enqueue(@($Root, 0))
    while ($pending.Count) {
        $dir, $level = $pending.Dequeue()
        foreach ($item in Get-ChildItem -LiteralPath $dir -Force -ErrorAction SilentlyContinue | Sort-Object Name) {
            if ($item.PSIsContainer) { if ($item.Name -notin $skip -and $level -lt $Depth) { $pending.Enqueue(@($item.FullName, ($level + 1))) }; continue }
            if ($item.Name -like '*.csproj' -or $item.Name -like '*.sln' -or $item.Name -eq 'package.json') { $item.FullName }
        }
    }
}

# A key from a name: lowercase letters and digits, starting with a letter, unique among $Taken.
function New-AppKey([string]$Name, $Taken) {
    $key = ($Name.ToLowerInvariant() -replace '[^a-z0-9]', '')
    if ($key -notmatch '^[a-z]') { $key = "app$key" }
    $unique = $key
    for ($n = 2; $Taken.Contains($unique); $n++) { $unique = "$key$n" }
    $null = $Taken.Add($unique)
    $unique
}

# The folder all these relative paths share ('.' when none).
function Get-CommonFolder([string[]]$Paths) {
    if (-not $Paths) { return '.' }
    $common = @($Paths[0] -split '[\\/]' | Where-Object { $_ })
    foreach ($path in $Paths) {
        $parts = @($path -split '[\\/]' | Where-Object { $_ })
        $length = 0
        while ($length -lt [Math]::Min($common.Count, $parts.Count) -and $common[$length] -eq $parts[$length]) { $length++ }
        $common = @($common | Select-Object -First $length)
    }
    if ($common) { $common -join '/' } else { '.' }
}

# A first profile for a checkout, to edit (Find-AppCandidates says which apps): @{ Profile; Notes }.
function New-ProfileScaffold([string]$Root) {
    $files = @(Get-ScaffoldFiles $Root)
    $found = Find-AppCandidates $Root $files
    $taken = [Collections.Generic.HashSet[string]]::new()
    foreach ($app in $found.Apps) { $app.key = New-AppKey $app.name $taken }
    $folder = Split-Path -Leaf $Root
    $id = ($folder.ToLowerInvariant() -replace '[^a-z0-9-]+', '-').Trim('-')
    if ($id -notmatch '^[a-z0-9]') { $id = 'project' }
    $profile = [ordered]@{ schemaVersion = 1; id = $id; name = $folder }
    $solution = @($files | Where-Object { $_ -like '*.sln' -and (Split-Path $_) -eq $Root })
    if ($solution) { $profile.detect = [ordered]@{ anyFile = @(Split-Path -Leaf $solution[0]) } }
    # watch: the folder all the .NET code shares (not the tests) - its changes are what needs a rebuild
    $profile.watch = @(Get-CommonFolder @($found.CodeFolders))
    $profile.apps = @($found.Apps)
    @{ Profile = $profile; Notes = $found.Notes }
}

function Invoke-ProfileInit([string]$Dir, [switch]$Save) {
    $start = if ($Dir) { $Dir } else { (Get-Location).ProviderPath }
    if (-not (Test-Path -LiteralPath $start -PathType Container)) { Write-Note "lazyrunner: $start is not a folder" 'Red'; return 1 }
    $root = (Resolve-Path -LiteralPath $start).ProviderPath
    if (-not $Dir) { $top = git -C $root rev-parse --show-toplevel 2>$null; if ($LASTEXITCODE -eq 0 -and $top) { $root = (Resolve-Path -LiteralPath $top).ProviderPath } }
    $target = Join-Path $root $RepoProfileFile
    if ($Save -and (Test-Path -LiteralPath $target)) { Write-Note "lazyrunner: $target exists already - edit it, then: lazyrunner profile check" 'Red'; return 1 }
    $scaffold = New-ProfileScaffold $root
    if (-not $scaffold.Profile.apps) {
        Write-Note "lazyrunner: found nothing to run in $root - no .csproj with a launch profile that has an applicationUrl, no package.json with a dev, start or serve script" 'Red'
        foreach ($note in $scaffold.Notes) { Write-Note "  - $note" 'DarkGray' }
        return 1
    }
    $json = $scaffold.Profile | ConvertTo-Json -Depth 10
    $file = if ($Save) { $target } else { Join-Path ([IO.Path]::GetTempPath()) "lazyrunner-init-$PID.json" }
    [IO.File]::WriteAllText($file, $json + "`n")
    if (-not $Save) { Write-Output $json }
    $result = Get-ProfileProblems @{ Path = $file; Source = 'repo'; Root = $root }
    if (-not $Save) { Remove-Item -LiteralPath $file -ErrorAction SilentlyContinue }
    $names = @($scaffold.Profile.apps | ForEach-Object name) -join ', '
    Write-Note "$(if ($Save) { "Saved $target" } else { "A start for $target (-Save writes it)" }): $($scaffold.Profile.apps.Count) apps - $names" 'Cyan'
    foreach ($note in $scaffold.Notes) { Write-Note "  - $note" 'DarkGray' }
    foreach ($problem in $result.Problems) {
        $isError = $problem.Level -eq 'error'
        Write-Note "  $(if ($isError) { 'x' } else { '-' }) $(if ($problem.At) { "$($problem.At): " })$($problem.Message)" $(if ($isError) { 'Red' } else { 'DarkGray' })
    }
    Write-Note 'Check the names, ports and variants (projects\README.md), then: lazyrunner profile check. lazyrunner asks you to trust it the first time it runs it.'
    0
}

# ---------------------------------------------------------------- the field reference

# projects\README.md's field reference, as Markdown, from profile.schema.json: one table per part of a profile, a row
# per field with its type, whether it is required, and the schema's description, default and example.
function Get-ProfileReference([string]$SchemaPath = (Get-ProfileSchemaPath)) {
    $schema = Get-Content -LiteralPath $SchemaPath -Raw | ConvertFrom-Json -AsHashtable
    $defs = $schema['$defs']
    $lines = [Collections.Generic.List[string]]::new()
    Add-ReferenceTable $lines 'The profile' $schema.description $schema.properties (@($schema.required) + @($schema.else.required)) $defs
    foreach ($name in 'dotnetApp', 'npmApp', 'serveVariant', 'buildVariant') {
        $def = $defs[$name]
        Add-ReferenceTable $lines $def.title $def.description $def.properties @($def.required) $defs
    }
    ($lines -join "`n").TrimEnd()
}

function Add-ReferenceTable($Lines, [string]$Title, [string]$Description, $Properties, [string[]]$Required, $Defs) {
    foreach ($line in "#### $Title", '', $Description, '', '| Field | Type | Required | What it does |', '|---|---|---|---|') { $Lines.Add($line) }
    foreach ($name in $Properties.Keys) {
        $node = $Properties[$name]
        Add-ReferenceRow $Lines $name $node ($name -in $Required) $Defs
        $target = Resolve-SchemaRef $node $Defs
        if ($target.type -eq 'object' -and $target.properties) {
            foreach ($child in $target.properties.Keys) { Add-ReferenceRow $Lines "$name.$child" $target.properties[$child] ($child -in @($target.required)) $Defs }
        }
    }
    $Lines.Add('')
}

function Get-SchemaRefName($Node) {
    $ref = if ($Node['$ref']) { $Node['$ref'] } elseif ($Node.allOf) { @($Node.allOf)[0]['$ref'] } else { $null }
    if ($ref) { ($ref -split '/')[-1] } else { $null }
}

function Resolve-SchemaRef($Node, $Defs) {
    $name = Get-SchemaRefName $Node
    if ($name) { $Defs[$name] } else { $Node }
}

function Get-SchemaTypeName($Node) {
    if ($null -ne $Node.const) { return "``$(ConvertTo-Json -InputObject $Node.const -Compress)``" }
    $name = Get-SchemaRefName $Node
    if ($name) { return @{ relativePath = 'path'; appKey = 'app key'; icon = 'character'; when = 'object' }[$name] ?? 'object' }
    if ($Node.type -eq 'array') {
        if (-not $Node.items) { return 'array' }
        if ($Node.items.oneOf) { return 'array of objects' }
        return "array of $(Get-SchemaTypeName $Node.items)s"
    }
    "$($Node.type)"
}

function Add-ReferenceRow($Lines, [string]$Name, $Node, [bool]$Required, $Defs) {
    $target = Resolve-SchemaRef $Node $Defs
    $text = if ($Node.description) { $Node.description } else { "$($target.description)" }
    $default = if ($null -ne $Node.default) { $Node.default } else { $target.default }
    $examples = $Node.examples   # not an if expression: that would unroll an example that is an array
    if (-not $examples) { $examples = $target.examples }
    if ($null -ne $default) { $text += " Default: ``$default``." }
    if ($examples -and $null -eq $Node.const) { $text += " Example: ``$(ConvertTo-Json -InputObject @($examples)[0] -Compress -Depth 5)``." }
    $Lines.Add("| ``$Name`` | $(Get-SchemaTypeName $Node) | $(if ($Required) { 'yes' }) | $($text.Trim().Replace('|', '\|')) |")
}
