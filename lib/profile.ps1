# Project profiles: what a project is - the file that marks its repo, the folder whose changes need a rebuild and its
# apps - as data, apart from lazyrunner's code. It finds the profile for a folder,
# checks it and turns it into the settings the rest of lazyrunner uses: $RepoMarker, $SourceDir and $Apps (an npm app also
# carries its folder and variants). lib\apptypes.ps1 says what each app type does. projects\README.md describes the
# format; projects\profile.schema.json checks it in editors.
# Dot-sourced by lazyrunner.ps1 and install.ps1, so it uses nothing from the other libraries.
#
# Where profiles come from, checked in this order in each folder from the current one up:
#   repo     .lazyrunner.json at the repo root: a whole profile, or "extends": "<id>" with changes to a known one.
#            Versioned with the code; used only once you trust it (Test-ProfileTrusted).
#   user     %APPDATA%\lazyrunner\projects\<id>.json (LAZYRUNNER_PROFILES_DIR): your own, for projects whose repo has none.
#   bundled  projects\<id>.json next to lazyrunner.ps1.
# LAZYRUNNER_PROFILE (a file) makes that the only profile, for tests.

$ProfileSchemaVersion = 1
$RepoProfileFile = '.lazyrunner.json'

function Get-UserProfilesDir {
    if ($env:LAZYRUNNER_PROFILES_DIR) { return $env:LAZYRUNNER_PROFILES_DIR }
    Join-Path $env:APPDATA 'lazyrunner\projects'
}

function Read-ProfileData([string]$Path) {
    $file = Split-Path -Leaf $Path
    if (-not (Test-Path -LiteralPath $Path)) { throw "lazyrunner: project profile not found: $Path" }
    $data = try { Get-Content -LiteralPath $Path -Raw | ConvertFrom-Json -AsHashtable } catch { throw "lazyrunner: $file is not valid JSON - $($_.Exception.Message)" }
    if ($data -isnot [hashtable]) { throw "lazyrunner: $file must hold one JSON object" }
    $data
}

# The profiles lazyrunner knows besides a repo's own: @{ Path; Source; Data }, the user's first (one with the id of a bundled
# profile replaces it). A file that isn't a JSON object is left out, with its error in $script:ProfileWarnings.
function Get-KnownProfiles([string]$LazyRunnerRoot) {
    if ($env:LAZYRUNNER_PROFILE) { return @(@{ Path = $env:LAZYRUNNER_PROFILE; Source = 'given'; Data = (Read-ProfileData $env:LAZYRUNNER_PROFILE) }) }
    $script:ProfileWarnings = @()
    $seen = @{}
    foreach ($source in @{ Name = 'user'; Dir = (Get-UserProfilesDir) }, @{ Name = 'bundled'; Dir = (Join-Path $LazyRunnerRoot 'projects') }) {
        if (-not (Test-Path -LiteralPath $source.Dir)) { continue }
        foreach ($file in Get-ChildItem -LiteralPath $source.Dir -Filter *.json -File | Where-Object Name -NotLike '*.schema.json' | Sort-Object Name) {
            try { $data = Read-ProfileData $file.FullName }
            catch { $script:ProfileWarnings += $_.Exception.Message; continue }
            if ($seen.ContainsKey("$($data.id)")) { continue }
            $seen["$($data.id)"] = $true
            @{ Path = $file.FullName; Source = $source.Name; Data = $data }
        }
    }
}

# The project a folder belongs to: walking up from it, the first folder with a .lazyrunner.json or the marker file of a
# known profile. @{ Root; Path; Source; Data } ($null when there is none); Resolve-ProjectProfile loads it.
function Find-ProjectProfile([string]$StartDir, [string]$LazyRunnerRoot) {
    $known = @(Get-KnownProfiles $LazyRunnerRoot)
    $dir = $StartDir
    while ($dir) {
        $repoFile = Join-Path $dir $RepoProfileFile
        if (-not $env:LAZYRUNNER_PROFILE -and (Test-Path -LiteralPath $repoFile)) { return @{ Root = $dir; Path = $repoFile; Source = 'repo' } }
        foreach ($candidate in $known) {
            $marker = @($candidate.Data.detect.anyFile)[0]
            if ($marker -is [string] -and $marker -and (Test-Path -LiteralPath (Join-Path $dir $marker))) { return $candidate + @{ Root = $dir } }
        }
        $dir = Split-Path $dir -Parent
    }
    $null
}

# A found profile, checked and turned into settings, plus Root, Source, FileHash (what trust is given to) and Hash
# (what lazyrunner runs, after extends: a branch that changes it is noticed). A broken profile stops lazyrunner with one message
# naming the file and every problem in it.
function Resolve-ProjectProfile($Found, [string]$LazyRunnerRoot) {
    $file = Split-Path -Leaf $Found.Path
    $data = Read-ProfileData $Found.Path
    $inRepo = $Found.Source -eq 'repo'
    if ($inRepo -and $data.extends) {
        $base = @(Get-KnownProfiles $LazyRunnerRoot | Where-Object { $_.Data.id -eq $data.extends })[0]
        if (-not $base) { throw "lazyrunner: $file extends '$($data.extends)', which is not a known profile (in $(Join-Path $LazyRunnerRoot 'projects') or $(Get-UserProfilesDir))" }
        $data = Merge-ProjectProfile $base.Data $data
    }
    $problems = @(Test-ProjectProfile $data -InRepo:$inRepo)
    if ($problems) { throw "lazyrunner: $file has $(if ($problems.Count -eq 1) { 'a problem' } else { "$($problems.Count) problems" }):`n  - $($problems -join "`n  - ")" }
    $settings = ConvertTo-ProjectSettings $data $Found.Path
    if ($inRepo) { $settings.RepoMarker = $RepoProfileFile }
    $settings.Root = $Found.Root
    $settings.Source = $Found.Source
    $settings.FileHash = Get-TextHash (Get-Content -LiteralPath $Found.Path -Raw)
    $settings.Hash = Get-TextHash (ConvertTo-SortedJson $data)
    $settings
}

# One profile file, without looking for it (LAZYRUNNER_PROFILE, tests).
function Import-ProjectProfile([string]$Path) { Resolve-ProjectProfile @{ Path = $Path; Source = 'given'; Root = $null } '' }

# A repo's changes on top of the profile it extends: each top-level field replaces the base's, except apps, which
# merge by key (an app's fields replace the base app's; a new key adds an app at the end).
function Merge-ProjectProfile($Base, $Override) {
    $merged = @{}
    foreach ($key in $Base.Keys) { $merged[$key] = $Base[$key] }
    foreach ($key in $Override.Keys) { if ($key -notin 'apps', 'extends', '$schema') { $merged[$key] = $Override[$key] } }
    if ($Override.apps) {
        $apps = [Collections.Generic.List[object]]::new()
        foreach ($app in @($Base.apps)) { $apps.Add($app.Clone()) }
        foreach ($app in @($Override.apps)) {
            $existing = @($apps | Where-Object { $_.key -eq $app.key })[0]
            if ($existing) { foreach ($field in $app.Keys) { $existing[$field] = $app[$field] } } else { $apps.Add($app) }
        }
        $merged.apps = @($apps)
    }
    $merged
}

function Get-TextHash([string]$Text) {
    [Convert]::ToHexString([Security.Cryptography.SHA256]::HashData([Text.Encoding]::UTF8.GetBytes($Text.Replace("`r`n", "`n"))))
}

# JSON with every object's keys sorted, so the same data always gives the same text (and hash).
function ConvertTo-SortedJson($Value) {
    function ConvertTo-Sorted($Item) {
        if ($Item -is [Collections.IDictionary]) {
            $sorted = [ordered]@{}
            foreach ($key in @($Item.Keys | Sort-Object)) { $sorted[$key] = ConvertTo-Sorted $Item[$key] }
            return $sorted
        }
        if ($Item -is [array]) { return , @($Item | ForEach-Object { ConvertTo-Sorted $_ }) }
        $Item
    }
    ConvertTo-Sorted $Value | ConvertTo-Json -Depth 20 -Compress
}

# ---------------------------------------------------------------- trust

# A repo profile says what lazyrunner runs, and a clone or a branch can bring one you haven't seen, so it is used only once
# you trust it - and again after its file changes: $StorePath keeps the hash you trusted per file. Bundled and user
# profiles come with lazyrunner or from you.
function Test-ProfileTrusted($Project, [string]$StorePath) {
    if ($Project.Source -ne 'repo') { return $true }
    (Read-TrustStore $StorePath)[$Project.Path] -eq $Project.FileHash
}

function Approve-ProjectProfile($Project, [string]$StorePath) {
    $store = Read-TrustStore $StorePath
    $store[$Project.Path] = $Project.FileHash
    New-Item -ItemType Directory -Force (Split-Path $StorePath) | Out-Null
    $store | ConvertTo-Json | Set-Content -LiteralPath $StorePath -Encoding utf8
}

function Read-TrustStore([string]$StorePath) {
    if (-not (Test-Path -LiteralPath $StorePath)) { return @{} }
    try { $store = Get-Content -LiteralPath $StorePath -Raw | ConvertFrom-Json -AsHashtable } catch { return @{} }
    if ($store -is [hashtable]) { $store } else { @{} }
}

# What a profile runs, one line per app, for the trust question.
function Get-ProfileSummary($Project) {
    foreach ($key in $Project.Apps.Keys) {
        $app = $Project.Apps[$key]
        if ($app.Variants) { "  $($app.Name): npm run $(@($app.Variants | ForEach-Object Script) -join ' or ') in $($app.Dir)" }
        else { "  $($app.Name): dotnet build + run $($app.Project) (launch profile $($app.Profile))" }
    }
}

# ---------------------------------------------------------------- checks

# Every problem in a profile, each naming the field it is about (apps[2].port …). Empty when it is fine. A repo's
# .lazyrunner.json needs no detect (the file itself marks the repo).
function Test-ProjectProfile($Data, [switch]$InRepo) {
    if ($Data.schemaVersion -isnot [long] -and $Data.schemaVersion -isnot [int]) { 'schemaVersion is required: 1'; return }
    if ($Data.schemaVersion -gt $ProfileSchemaVersion) { "schemaVersion $($Data.schemaVersion) needs a newer lazyrunner (this one reads profiles up to version $ProfileSchemaVersion)"; return }
    if ($Data.schemaVersion -lt 1) { 'schemaVersion must be 1'; return }
    Test-KnownFields $Data $ProfileFields.Top ''
    if ($Data.extends -and -not $InRepo) { "extends is only for a repo's $RepoProfileFile" }
    if ("$($Data.id)" -notmatch '^[a-z0-9][a-z0-9-]*$') { "id is required: a short lowercase name such as 'shop'" }
    if (-not "$($Data.name)".Trim()) { 'name is required' }
    $markers = @($Data.detect.anyFile)
    if (-not ($InRepo -and $null -eq $Data.detect)) {
        if ($Data.detect -isnot [hashtable] -or -not $Data.detect.anyFile) { 'detect.anyFile is required: the file that marks the repo, e.g. ["Shop.sln"]' }
        else {
            Test-KnownFields $Data.detect $ProfileFields.Detect 'detect'
            if ($markers.Count -ne 1) { "detect.anyFile: one file for now (it has $($markers.Count))" }
            else { Test-RelativePath $markers[0] 'detect.anyFile[0]' }
        }
    }
    $watch = @($Data.watch)
    if (-not $Data.watch) { 'watch is required: the folder whose changes need a rebuild, e.g. ["services"]' }
    elseif ($watch.Count -ne 1) { "watch: one folder for now (it has $($watch.Count))" }
    else { Test-RelativePath $watch[0] 'watch[0]' }
    $apps = @($Data.apps)
    if (-not $Data.apps) { 'apps is required: at least one app'; return }
    $keys = @{}
    for ($i = 0; $i -lt $apps.Count; $i++) {
        $app = $apps[$i]
        $at = "apps[$i]"
        if ($app -isnot [hashtable]) { "$at must be an object"; continue }
        if ("$($app.key)" -notmatch '^[a-z][a-z0-9]*$') { "$at.key is required: lowercase letters and digits, e.g. orders" }
        elseif ($keys.ContainsKey($app.key)) { "$at.key '$($app.key)' is used twice" }
        else { $keys[$app.key] = $app }
        if ($app.key) { $at = "$at ($($app.key))" }
        if (-not "$($app.name)".Trim()) { "$at.name is required" }
        if ($null -ne $app.icon -and ($app.icon -isnot [string] -or $app.icon.Length -ne 1)) { "$at.icon must be one character (it fills one cell of the title bar)" }
        switch ($app.type) {
            'dotnet' {
                Test-KnownFields $app $ProfileFields.Dotnet $at
                if (-not $app.project) { "$at.project is required: the .csproj to run" }
                else { Test-RelativePath $app.project "$at.project"; if ($app.project -notmatch '\.csproj$') { "$at.project must be a .csproj file" } }
                if (-not "$($app.launchProfile)".Trim()) { "$at.launchProfile is required: a profile from its Properties/launchSettings.json" }
                if ($null -ne $app.swagger -and "$($app.swagger)" -notmatch '^/') { "$at.swagger must start with /, e.g. '/swagger'" }
            }
            'npm' { Test-KnownFields $app $ProfileFields.Npm $at; Test-NpmApp $app $at }
            default { "$at.type must be 'dotnet' or 'npm' (it is '$($app.type)')" }
        }
    }
    $npm = @($apps | Where-Object { $_ -is [hashtable] -and $_.type -eq 'npm' })
    foreach ($variant in @($npm | ForEach-Object { @($_.variants) } | Where-Object { $_ -is [hashtable] -and $_.servedBy })) {
        if (-not $keys.ContainsKey($variant.servedBy) -or $keys[$variant.servedBy].type -ne 'dotnet') { "servedBy '$($variant.servedBy)' must be the key of a dotnet app" }
    }
}

# The fields each part of a profile can have (profile.schema.json has the same): anything else is most likely a typo,
# which lazyrunner would otherwise ignore without a word.
$ProfileFields = @{
    Top     = '$schema', 'schemaVersion', 'extends', 'id', 'name', 'detect', 'watch', 'apps'
    Detect  = @('anyFile')
    Dotnet  = 'key', 'name', 'icon', 'type', 'project', 'launchProfile', 'swagger'
    Npm     = 'key', 'name', 'icon', 'type', 'dir', 'variants'
    Variant = 'label', 'when', 'script', 'port', 'servedBy', 'ready'
    When    = @('packageScript')
    Ready   = @('file')
}

function Test-KnownFields([hashtable]$Object, [string[]]$Known, [string]$At) {
    foreach ($field in $Object.Keys | Where-Object { $_ -cnotin $Known }) {
        # A field one or two typos away (one for short names) is most likely the one meant.
        $closest = $Known | Sort-Object { Get-EditDistance $field $_ } | Select-Object -First 1
        $distance = Get-EditDistance $field $closest
        $hint = if ($distance -le $(if ($field.Length -ge 4) { 2 } else { 1 })) { "did you mean $($closest)?" } else { "it can have: $($Known -join ', ')" }
        "$(if ($At) { "$At." })$field is not a profile field - $hint"
    }
}

# How many letters to change, add or drop to get from one word to the other (case doesn't count).
function Get-EditDistance([string]$A, [string]$B) {
    $a = $A.ToLowerInvariant()
    $b = $B.ToLowerInvariant()
    $previous = [int[]](0..$b.Length)
    for ($i = 1; $i -le $a.Length; $i++) {
        $current = [int[]]::new($b.Length + 1)
        $current[0] = $i
        for ($j = 1; $j -le $b.Length; $j++) {
            $cost = [int]($a[$i - 1] -ne $b[$j - 1])
            $current[$j] = [Math]::Min([Math]::Min($previous[$j] + 1, $current[$j - 1] + 1), $previous[$j - 1] + $cost)
        }
        $previous = $current
    }
    $previous[$b.Length]
}

function Test-RelativePath($Value, [string]$At) {
    if ($Value -isnot [string] -or -not $Value.Trim()) { "$At must be a path"; return }
    if ($Value -match ':' -or $Value -match '^[\\/]') { "$At must be relative to the repo root (it is '$Value')" }
}

# An npm app: its folder and one or more ways to run, the first whose when.packageScript package.json has - a dev
# server on a port, or a watch build that a dotnet app serves. A variant without when always applies, so only the
# last one can leave it out.
function Test-NpmApp($App, [string]$At) {
    if (-not $App.dir) { "$At.dir is required: the folder with package.json" } else { Test-RelativePath $App.dir "$At.dir" }
    $variants = @($App.variants)
    if (-not $App.variants) { "$At.variants is required: how it runs (a dev server or a watch build)"; return }
    for ($v = 0; $v -lt $variants.Count; $v++) {
        $variant = $variants[$v]
        $vat = "$At.variants[$v]"
        if ($variant -isnot [hashtable]) { "$vat must be an object"; continue }
        Test-KnownFields $variant $ProfileFields.Variant $vat
        if ($variant.when -is [hashtable]) { Test-KnownFields $variant.when $ProfileFields.When "$vat.when" }
        if ($variant.ready -is [hashtable]) { Test-KnownFields $variant.ready $ProfileFields.Ready "$vat.ready" }
        if ($null -ne $variant.when -and -not "$($variant.when.packageScript)".Trim()) { "$vat.when.packageScript is required: the package.json script that selects it" }
        elseif ($null -eq $variant.when -and $v -lt $variants.Count - 1) { "$vat has no when, so it always applies: only the last variant can leave it out" }
        if (-not "$($variant.script)".Trim()) { "$vat.script is required: the npm script to run" }
        $hasPort = $null -ne $variant.port
        $hasServer = [bool]$variant.servedBy
        if ($hasPort -eq $hasServer) { "$vat needs either port (a dev server) or servedBy (a watch build), not $(if ($hasPort) { 'both' } else { 'neither' })" }
        if ($hasPort -and ($variant.port -isnot [long] -and $variant.port -isnot [int] -or $variant.port -lt 1 -or $variant.port -gt 65535)) { "$vat.port must be a port number (1-65535)" }
        if ($hasServer) { if (-not $variant.ready.file) { "$vat.ready.file is required: the file the build writes when it is done" } else { Test-RelativePath $variant.ready.file "$vat.ready.file" } }
    }
}

# The settings lazyrunner uses (Windows separators). A dotnet app has Name, Kind, Profile, Project and Swagger; an npm
# app has Dir and Variants: @{ Label; When; Script; Port (a dev server) or ServedBy + ReadyFile (a watch build) }.
function ConvertTo-ProjectSettings($Data, [string]$Path) {
    $apps = [ordered]@{}
    $icons = @{}
    foreach ($app in $Data.apps) {
        if ($app.icon) { $icons[$app.key] = $app.icon }
        if ($app.type -eq 'dotnet') {
            $entry = @{ Name = $app.name; Kind = 'dotnet'; Profile = $app.launchProfile; Project = (ConvertTo-WindowsPath $app.project) }
            if ($app.swagger) { $entry.Swagger = $app.swagger }
            $apps[$app.key] = $entry
            continue
        }
        $variants = @(foreach ($variant in $app.variants) {
                $serves = $null -ne $variant.port
                @{
                    Label = if ($variant.label) { $variant.label } elseif ($serves) { 'dev server' } else { 'watch build' }
                    When = $variant.when.packageScript; Script = $variant.script
                    Port = if ($serves) { [int]$variant.port } else { $null }
                    ServedBy = $variant.servedBy
                    ReadyFile = if ($variant.ready.file) { ConvertTo-WindowsPath $variant.ready.file } else { $null }
                }
            })
        $apps[$app.key] = @{ Name = $app.name; Kind = 'npm'; Dir = (ConvertTo-WindowsPath $app.dir); Variants = $variants }
    }
    @{
        Id = $Data.id; Name = $Data.name; Path = $Path
        RepoMarker = ConvertTo-WindowsPath @($Data.detect.anyFile)[0]
        SourceDir = ConvertTo-WindowsPath @($Data.watch)[0]
        Apps = $apps
        Icons = $icons
    }
}

function ConvertTo-WindowsPath([string]$Path) { ($Path -replace '/', '\').TrimEnd('\') }
