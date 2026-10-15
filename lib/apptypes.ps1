# App types: the one place that knows what a dotnet or an npm app is. Everything else asks about a trait
# (Get-AppTrait $key 'Builds') or calls a function here, so a new type is a new entry plus its commands, not edits
# all over lazyrunner. profile.ps1 checks each type's fields when it loads a profile.
# Dot-sourced by lazyrunner.ps1; uses the settings defined there.

# What each type does:
#   Builds          built before it starts (dotnet build), one build at a time, skipped when nothing changed
#   UsesSecrets     reads the user secrets: restarts when the database is switched, gives secrets.json its path
#   CountsRequests  its log has one line per request (the req/s sparkline)
#   IndentedLogs    indented lines continue the previous log entry (and take its colour)
#   Installs        has a package-lock.json to install with npm ci (i)
#   Variants        runs one of the profile's variants, picked by which script package.json has
#   WatchesOwnFiles rebuilds by itself, so a branch change or code change does not restart it
#   ColorsErrors    red and yellow lines count as errors and warnings (no log levels)
#   Frontend        what the frontend settings ("frontend", "stopFrontendOnStop") apply to
#   StartVerb       shown while it starts
$AppTypes = @{
    dotnet = @{ Builds = $true; UsesSecrets = $true; CountsRequests = $true; IndentedLogs = $true; StartVerb = 'starting' }
    npm    = @{ Installs = $true; Variants = $true; WatchesOwnFiles = $true; ColorsErrors = $true; Frontend = $true; StartVerb = 'compiling' }
}

function Get-AppTrait([string]$Key, [string]$Trait) { $AppTypes[$Apps[$Key].Kind][$Trait] }

function Test-AppTrait([string]$Key, [string]$Trait) { [bool](Get-AppTrait $Key $Trait) }

# The apps with a trait, in column order.
function Get-AppKeys([string]$Trait) { @($Apps.Keys | Where-Object { Test-AppTrait $_ $Trait }) }

# lazyrunner.config.json can turn an app off on one machine: "apps": { "<key>": { "enabled": false } }, or every
# frontend at once with "frontend": false.
function Test-AppEnabled($Config, [string]$Key) {
    $setting = $Config.apps.$Key.enabled
    if ($null -ne $setting) { return [bool]$setting }
    if (Test-AppTrait $Key 'Frontend') { return [bool]$Config.frontend }
    $true
}

# Whether Stop all / Restart all include the app: "apps": { "<key>": { "includeInAll": false } }, else the
# frontend setting for frontends (stopFrontendOnStop, , then s), else yes.
function Test-InStopAll([string]$Key) {
    $setting = $Ctl.Config.apps.$Key.includeInAll
    if ($null -ne $setting) { return [bool]$setting }
    if (Test-AppTrait $Key 'Frontend') { return [bool]$Ctl.Config.stopFrontendOnStop }
    $true
}

# ---------------------------------------------------------------- variants (npm)

# The variant this checkout runs: the first whose when.packageScript is in Dir\package.json (one without when
# needs just its own script there), skipping a watch build whose servedBy app is turned off on this machine.
# @{ Mode ('serve' or 'build'); Script; Port; Label; ServedBy; ReadyFile }, or $null when none matches. A dev
# server's port comes from --port in its script when it has one.
function Get-AppMode([string]$RepoRoot, [string]$Key) {
    $app = $Apps[$Key]
    if (-not (Test-AppTrait $Key 'Variants')) { return $null }
    $package = Read-Json (Join-Path $RepoRoot (Join-Path $app.Dir 'package.json'))
    if (-not $package) { return $null }
    $scripts = if ($package.scripts) { $package.scripts } else { @{} }
    foreach ($variant in $app.Variants) {
        if (-not $scripts.ContainsKey($(if ($variant.When) { $variant.When } else { $variant.Script }))) { continue }
        if ($variant.ServedBy -and -not $Apps.Contains($variant.ServedBy)) { continue }
        $port = $null
        if ($variant.Port) { $port = if ("$($scripts[$variant.Script])" -match '--port[= ](\d+)') { [int]$Matches[1] } else { $variant.Port } }
        return @{
            Mode = if ($variant.Port) { 'serve' } else { 'build' }
            Script = $variant.Script; Port = $port; Label = $variant.Label; ServedBy = $variant.ServedBy; ReadyFile = $variant.ReadyFile
        }
    }
    $null
}

# Why an app has no variant on this checkout, for the log: "client\package.json has neither a nor b", plus any
# watch build left out because the app serving it is turned off.
function Get-NoModeReason([string]$RepoRoot, [string]$Key) {
    $app = $Apps[$Key]
    $package = Join-Path $app.Dir 'package.json'
    if (-not (Test-Path -LiteralPath (Join-Path $RepoRoot $package))) { return "$package is missing" }
    $scripts = @($app.Variants | ForEach-Object { if ($_.When) { $_.When } else { $_.Script } })
    $what = switch ($scripts.Count) {
        1 { "has no $($scripts[0])" }
        2 { "has neither $($scripts[0]) nor $($scripts[1])" }
        default { "has none of $($scripts -join ', ')" }
    }
    $off = @($app.Variants | Where-Object { $_.ServedBy -and -not $Apps.Contains($_.ServedBy) })
    $served = if ($off) { ", or $($off[0].ServedBy), which would serve $($off[0].Script), is turned off in lazyrunner.config.json" } else { '' }
    "$package $what$served"
}

# ---------------------------------------------------------------- ports and commands

function Get-AppPort([string]$RepoRoot, [string]$Key) {
    $app = $Apps[$Key]
    if (Test-AppTrait $Key 'Variants') { return (Get-AppMode $RepoRoot $Key).Port }
    $settings = Read-Json (Join-Path $RepoRoot (Join-Path (Split-Path $app.Project) 'Properties\launchSettings.json'))
    if (-not $settings) { return $null }
    $url = $settings.profiles[$app.Profile].applicationUrl
    if ($url) { ([uri]($url -split ';')[0]).Port } else { $null }
}

# How to run an app: @{ File; Args; Dir; Label; Script }. Script is the npm script, to notice when a branch needs
# another one.
function Get-ServerCommand([string]$Key, [string]$RepoRoot) {
    $app = $Apps[$Key]
    if (Test-AppTrait $Key 'Builds') {
        return @{
            File = 'dotnet'; Args = @('run', '--no-build', '--launch-profile', $app.Profile)
            Dir = Split-Path (Join-Path $RepoRoot $app.Project); Label = "dotnet run --launch-profile $($app.Profile)"; Script = $null
        }
    }
    $mode = Get-AppMode $RepoRoot $Key
    if (-not $mode) { throw "$(Get-NoModeReason $RepoRoot $Key) on this branch" }
    $dir = Join-Path $RepoRoot $app.Dir
    if (-not (Test-Path (Join-Path $dir 'node_modules'))) { throw "$($app.Dir)\node_modules is missing - run 'npm install' in $dir first" }
    $npm = Get-NpmLauncher
    @{ File = $npm.File; Args = $npm.Prefix + @('run', $mode.Script, '--verbose'); Dir = $dir; Label = "npm run $($mode.Script) --verbose ($($mode.Label))"; Script = $mode.Script }
}

# How to build an app that Builds: @{ File; Args; Dir; Project }.
function Get-BuildCommand([string]$Key, [string]$RepoRoot) {
    $project = Join-Path $RepoRoot $Apps[$Key].Project
    @{ File = 'dotnet'; Args = @('build', $project, '-nologo', '-v', 'm', '-tl:off', '-clp:NoSummary'); Dir = Split-Path $project; Project = $project }
}

# ---------------------------------------------------------------- profile check / init (lazyrunner profile)

function New-Problem([string]$Level, [string]$At, [string]$Message) { [pscustomobject]@{ Level = $Level; At = $At; Message = $Message } }

# What lazyrunner profile check finds about an app in a checkout: problems (error / warning / info), each naming the field.
function Test-AppSetup([string]$RepoRoot, [string]$Key, [string]$At) {
    $app = $Apps[$Key]
    if (Test-AppTrait $Key 'Builds') {
        if (-not (Test-Path -LiteralPath (Join-Path $RepoRoot $app.Project))) { New-Problem 'error' "$At.project" "$($app.Project) doesn't exist"; return }
        if ((Get-Content -LiteralPath (Join-Path $RepoRoot $app.Project) -Raw) -match 'Microsoft\.NET\.Test\.Sdk') { New-Problem 'warning' "$At.project" "$($app.Project) is a test project (it references Microsoft.NET.Test.Sdk) - lazyrunner runs apps, not tests" }
        $settingsFile = Join-Path (Split-Path $app.Project) 'Properties\launchSettings.json'
        $settings = Read-Json (Join-Path $RepoRoot $settingsFile)
        if (-not $settings) { New-Problem 'error' "$At.launchProfile" "there is no $settingsFile, so lazyrunner can't run it with a launch profile or tell its port"; return }
        $names = @($settings.profiles.Keys)
        if ($app.Profile -notin $names) { New-Problem 'error' "$At.launchProfile" "'$($app.Profile)' is not in $settingsFile (it has: $($names -join ', '))"; return }
        if (-not (Get-AppPort $RepoRoot $Key)) { New-Problem 'error' "$At.launchProfile" "'$($app.Profile)' in $settingsFile has no applicationUrl, so lazyrunner can't tell its port" }
        return
    }
    if (Test-AppTrait $Key 'Variants') {
        $package = Join-Path $app.Dir 'package.json'
        $data = Read-Json (Join-Path $RepoRoot $package)
        if (-not $data) { New-Problem 'error' "$At.dir" "there is no $package"; return }
        $scripts = if ($data.scripts) { $data.scripts } else { @{} }
        for ($i = 0; $i -lt @($app.Variants).Count; $i++) {
            $variant = @($app.Variants)[$i]
            if ($variant.When -and -not $scripts.ContainsKey($variant.When)) { New-Problem 'info' "$At.variants[$i]" "$package has no $($variant.When) on this branch, so this variant doesn't apply here (it will where it has)" }
            elseif (-not $scripts.ContainsKey($variant.Script)) { New-Problem 'error' "$At.variants[$i].script" "$package has no script '$($variant.Script)'" }
        }
        if (-not (Get-AppMode $RepoRoot $Key)) { New-Problem 'warning' $At "no variant applies on this branch, so lazyrunner skips it ($(Get-NoModeReason $RepoRoot $Key))" }
        if (-not (Test-Path -LiteralPath (Join-Path $RepoRoot (Join-Path $app.Dir 'node_modules')))) { New-Problem 'info' $At "$($app.Dir)\node_modules is missing: install the packages first (i in lazyrunner runs npm ci)" }
    }
}

# The port an npm app's files name: the server file a `node <file>` script runs (a PORT constant, listen(4300)), the
# Vite config (server.port), angular.json (port) or .env (PORT=). $null when none says.
function Find-ConfigPort([string]$Dir, [string]$Command) {
    $candidates = @(
        if ($Command -match '\bnode\s+(?:--\S+\s+)*([\w./\\-]+\.[cm]?js)\b') { @{ File = $Matches[1]; Pattern = '(?i)\bport\s*[=:]\s*(\d{4,5})\b|\.listen\(\s*(\d{4,5})\b' } }
        @{ File = 'vite.config.ts'; Pattern = '\bport\s*:\s*(\d{4,5})\b' }
        @{ File = 'vite.config.js'; Pattern = '\bport\s*:\s*(\d{4,5})\b' }
        @{ File = 'vite.config.mjs'; Pattern = '\bport\s*:\s*(\d{4,5})\b' }
        @{ File = 'angular.json'; Pattern = '"port"\s*:\s*(\d{4,5})\b' }
        @{ File = '.env'; Pattern = '(?m)^\s*PORT\s*=\s*(\d{4,5})\b' }
    )
    foreach ($candidate in $candidates) {
        $file = Join-Path $Dir $candidate.File
        if (-not (Test-Path -LiteralPath $file -PathType Leaf)) { continue }
        if ((Get-Content -LiteralPath $file -Raw) -match $candidate.Pattern) { return [int]"$($Matches[1])$($Matches[2])" }
    }
    $null
}

# The apps lazyrunner profile init proposes for a checkout: each runnable .NET project (a launch profile with an
# applicationUrl, not a test project) and each package.json with a dev / start / serve script. @{ Apps; Notes }.
function Find-AppCandidates([string]$RepoRoot, [string[]]$Files) {
    $apps = [Collections.Generic.List[object]]::new()
    $notes = [Collections.Generic.List[string]]::new()
    foreach ($file in $Files | Where-Object { $_ -like '*.csproj' }) {
        $relative = [IO.Path]::GetRelativePath($RepoRoot, $file).Replace('\', '/')
        $text = Get-Content -LiteralPath $file -Raw
        if ($text -match 'Microsoft\.NET\.Test\.Sdk') { continue }
        $settings = Read-Json (Join-Path (Split-Path $file) 'Properties\launchSettings.json')
        $name = [IO.Path]::GetFileNameWithoutExtension($file)
        $profiles = if ($settings.profiles -is [hashtable]) { $settings.profiles } else { @{} }   # none: a library or a worker
        $launch = @($profiles.Keys | Where-Object { $profiles[$_].applicationUrl -and $profiles[$_].commandName -eq 'Project' } | Sort-Object)
        if (-not $launch) { $notes.Add("skipped $relative - no launch profile with an applicationUrl, so lazyrunner can't tell where it listens"); continue }
        $profileName = if ($name -in $launch) { $name } else { $launch[0] }
        $app = [ordered]@{ key = ''; name = $name; type = 'dotnet'; project = $relative; launchProfile = $profileName }
        $program = Join-Path (Split-Path $file) 'Program.cs'
        if ($text -match 'Swashbuckle|NSwag' -or ((Test-Path -LiteralPath $program) -and (Get-Content -LiteralPath $program -Raw) -match 'UseSwagger')) { $app.swagger = '/swagger' }
        $apps.Add($app)
    }
    foreach ($file in $Files | Where-Object { (Split-Path -Leaf $_) -eq 'package.json' }) {
        $dir = Split-Path $file
        $relative = [IO.Path]::GetRelativePath($RepoRoot, $dir).Replace('\', '/')
        $data = Read-Json $file
        $scripts = if ($data.scripts) { $data.scripts } else { @{} }
        $script = @('dev', 'start', 'serve') | Where-Object { $scripts.ContainsKey($_) } | Select-Object -First 1
        if (-not $script) {
            $watch = @($scripts.Keys | Where-Object { $_ -match 'watch' })
            if ($watch) { $notes.Add("$relative/package.json has no dev, start or serve script but has $($watch -join ', '): if a .NET app serves that build, add it as a watch build (servedBy)") }
            continue
        }
        $command = "$($scripts[$script])"
        # The port: said in the script (--port 4300, -p 4300, PORT=4300, or a lone number), else in the files it uses,
        # else the tool's default.
        $filePort = Find-ConfigPort $dir $command
        $port = if ($command -match '(?:--port|(?:^|\s)-p)[= ]?(\d{2,5})\b|\bPORT=(\d{2,5})\b') { [int]"$($Matches[1])$($Matches[2])" }
        elseif ($command -match '\s(\d{4,5})(\s|$)') { [int]$Matches[1] }
        elseif ($filePort) { $filePort }
        elseif ($command -match '\bvite\b') { 5173 }
        elseif ($command -match '\bng\b.*\bserve\b') { 4200 }
        elseif ($command -match '\bwebpack\b') { 8080 }
        else { $notes.Add("$relative/package.json: '$script' runs '$command' - port 3000 is a guess, check it"); 3000 }
        $name = if ($relative -eq '.') { "$($data.name)" } else { Split-Path -Leaf $dir }
        if (-not $name) { $name = Split-Path -Leaf $RepoRoot }
        $name = $name.Substring(0, 1).ToUpperInvariant() + $name.Substring(1)
        $apps.Add([ordered]@{ key = ''; name = $name; type = 'npm'; dir = $relative; variants = @([ordered]@{ script = $script; port = $port }) })
    }
    # The .NET code a change to which needs a rebuild: every project but the tests, runnable or not (libraries too).
    $code = @($Files | Where-Object { $_ -like '*.csproj' -and (Get-Content -LiteralPath $_ -Raw) -notmatch 'Microsoft\.NET\.Test\.Sdk' } |
            ForEach-Object { [IO.Path]::GetRelativePath($RepoRoot, (Split-Path $_)).Replace('\', '/') })
    @{ Apps = $apps; Notes = $notes; CodeFolders = $code }
}
