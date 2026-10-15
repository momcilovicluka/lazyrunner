# Project profiles: a profile loads into the settings lazyrunner runs with; no code compares an app type outside
# lib\apptypes.ps1; several npm apps and variants work; broken profiles stop lazyrunner with clear messages.
# Run: pwsh -NoProfile -File tests\profile.Tests.ps1
. "$PSScriptRoot\TestHelpers.ps1"
$samplePath = Join-Path $PSScriptRoot 'fixtures\sample.json'
$schemaPath = Join-Path $LazyRunnerRoot 'projects\profile.schema.json'

# Code only, by PowerShell's own tokens (comments may say anything): each match of $Pattern in lib\, lazyrunner.ps1
# and install.ps1 as file:line, leaving out the files in $Except.
function Find-InCode([string]$Pattern, [string[]]$Except = @()) {
    $files = @(Get-ChildItem (Join-Path $LazyRunnerRoot 'lib\*.ps1')) + (Get-Item (Join-Path $LazyRunnerRoot 'lazyrunner.ps1'), (Join-Path $LazyRunnerRoot 'install.ps1'))
    foreach ($file in $files | Where-Object Name -notin $Except) {
        $tokens = $null
        $null = [Management.Automation.Language.Parser]::ParseFile($file.FullName, [ref]$tokens, [ref]$null)
        $code = [Text.StringBuilder]::new()
        $line = 1
        foreach ($token in $tokens | Where-Object Kind -ne 'Comment') {
            while ($line -lt $token.Extent.StartLineNumber) { $null = $code.Append("`n"); $line++ }
            $null = $code.Append(' ').Append($token.Text)
            $line = $token.Extent.EndLineNumber
        }
        $lines = $code.ToString() -split "`n"
        for ($i = 0; $i -lt $lines.Count; $i++) { if ($lines[$i] -match $Pattern) { "$($file.Name):$($i + 1)" } }
    }
}

function Format-Found($Found) { "$(@($Found).Count) found$(if ($Found) { ': ' + ((@($Found) | Select-Object -First 3) -join ', ') })" }
function Read-Sample { Get-Content $samplePath -Raw | ConvertFrom-Json -AsHashtable }

"== the settings lazyrunner runs with"
. (Get-LazyRunnerLoadScript)
Use-ProjectProfile (Import-ProjectProfile $samplePath)
Check "the apps keep their order (the columns): $(@($Apps.Keys) -join ', ')" ((@($Apps.Keys) -join ',') -eq 'orders,billing,shop,docs,console' -and $Apps -is [Collections.Specialized.OrderedDictionary])
Check "the profile's name, id, marker and watch folder: $($ProjectProfile.Name) ($($ProjectProfile.Id)), $RepoMarker, $SourceDir" ($ProjectProfile.Name -eq 'Sample' -and $ProjectProfile.Id -eq 'sample' -and $RepoMarker -eq 'Sample.sln' -and $SourceDir -eq 'services')
Check "a .NET app: $($Apps.orders.Project), launch profile $($Apps.orders.Profile), Swagger at $($Apps.orders.Swagger) (Windows separators)" ($Apps.orders.Project -eq 'services\Orders\Orders.csproj' -and $Apps.orders.Profile -eq 'Orders' -and $Apps.orders.Swagger -eq '/swagger')
Check "icons from the profile: $(($Apps.Keys | Where-Object { $AppIcons[$_] } | ForEach-Object { "$_ $($AppIcons[$_])" }) -join ', ') (the rest get ■)" ($AppIcons.orders -eq '◆' -and $AppIcons.Count -eq 1)
Check "a dev server's port is an int ($($Apps.shop.Variants[0].Port.GetType().Name)), its default label '$($Apps.shop.Variants[0].Label)'" ($Apps.shop.Variants[0].Port -is [int] -and $Apps.shop.Variants[0].Label -eq 'dev server')
$docs = $Apps.docs.Variants[0]
Check "a watch build: served by $($docs.ServedBy), ready at $($docs.ReadyFile), label '$($docs.Label)'" ($docs.ServedBy -eq 'orders' -and $docs.ReadyFile -eq 'dist\index.html' -and $docs.Label -eq 'watch build' -and -not $docs.Port)
$variants = $Apps.console.Variants
Check "variants in order: $(($variants | ForEach-Object { "$($_.Label) (when: $(if ($_.When) { $_.When } else { 'always' }))" }) -join ', ')" ($variants.Count -eq 2 -and $variants[0].When -eq 'dev:mock' -and -not $variants[1].When -and $variants[1].Port -eq 58303)

"== no code checks an app type outside lib\apptypes.ps1"
$typed = @(Find-InCode "-c?(eq|ne|in|notin)\s+'(dotnet|npm)'" -Except 'apptypes.ps1', 'profile.ps1')
Check "app types are compared only in lib\apptypes.ps1 (and checked in profile.ps1) ($(Format-Found $typed))" ($typed.Count -eq 0)
Check "the apps' traits: build $((Get-AppKeys 'Builds') -join ','), install $((Get-AppKeys 'Installs') -join ','), secrets $((Get-AppKeys 'UsesSecrets') -join ',')" (((Get-AppKeys 'Builds') -join ',') -eq 'orders,billing' -and ((Get-AppKeys 'Installs') -join ',') -eq 'shop,docs,console' -and ((Get-AppKeys 'UsesSecrets') -join ',') -eq 'orders,billing')

"== the schema"
Check 'tests\fixtures\sample.json passes profile.schema.json' (Test-Json -Path $samplePath -SchemaFile $schemaPath -ErrorAction SilentlyContinue)

"== broken profiles: one clear message, naming the field"
# Each case changes a copy of sample.json; the loader's message must match, and the schema must reject it too
# (except the cases only the loader checks).
function Test-Broken([string]$Label, [scriptblock]$Change, [string]$Expect, [switch]$LoaderOnly, [string]$Raw) {
    $file = Join-Path $TestRun "broken-$([guid]::NewGuid().ToString('n').Substring(0, 6)).json"
    if ($Raw) { Set-Content $file $Raw }
    else {
        $data = Read-Sample
        & $Change $data
        $data | ConvertTo-Json -Depth 10 | Set-Content $file
    }
    $message = try { $null = Import-ProjectProfile $file; '(loaded without an error)' } catch { $_.Exception.Message }
    $schemaRejects = $Raw -or -not (Test-Json -Path $file -SchemaFile $schemaPath -ErrorAction SilentlyContinue)
    $line = ($message -split "`n" | Where-Object { $_ -match [regex]::Escape($Expect) } | Select-Object -First 1)
    Check "$Label -> $(if ($line) { $line.Trim(' ', '-') } else { $message -replace "`n", ' | ' })" ([bool]$line -and ($LoaderOnly -or $schemaRejects))
}
Test-Broken 'not JSON' {} 'is not valid JSON' -Raw '{ "apps": [ '
Test-Broken 'a newer format' { param($d) $d.schemaVersion = 2 } 'schemaVersion 2 needs a newer lazyrunner'
Test-Broken 'no apps' { param($d) $d.Remove('apps') } 'apps is required'
Test-Broken 'dotnet app without a launch profile' { param($d) $d.apps[0].Remove('launchProfile') } 'apps[0] (orders).launchProfile is required'
Test-Broken 'unknown app type' { param($d) $d.apps[1].type = 'python' } "apps[1] (billing).type must be 'dotnet' or 'npm'"
Test-Broken 'two apps with one key' { param($d) $d.apps[1].key = 'orders' } "apps[1].key 'orders' is used twice" -LoaderOnly
Test-Broken 'an absolute path' { param($d) $d.apps[0].project = 'C:\src\Orders\Orders.csproj' } 'must be relative to the repo root'
Test-Broken 'not a .csproj' { param($d) $d.apps[0].project = 'services/Orders/Orders.fsproj' } 'must be a .csproj file'
Test-Broken 'an icon of two characters' { param($d) $d.apps[0].icon = 'ab' } 'apps[0] (orders).icon must be one character'
Test-Broken 'a bad port' { param($d) $d.apps[2].variants[0].port = 70000 } 'port must be a port number'
Test-Broken 'no variants' { param($d) $d.apps[2].variants = @() } 'apps[2] (shop).variants is required'
Test-Broken 'a variant without when before the last' { param($d) $d.apps[4].variants[0].Remove('when') } 'apps[4] (console).variants[0] has no when, so it always applies' -LoaderOnly
Test-Broken 'a variant with both port and servedBy' { param($d) $d.apps[3].variants[0].port = 4300 } 'needs either port (a dev server) or servedBy (a watch build), not both'
Test-Broken 'two marker files' { param($d) $d.detect.anyFile = @('Sample.sln', 'Other.sln') } 'detect.anyFile: one file for now'
Test-Broken 'served by an app that does not exist' { param($d) $d.apps[3].variants[0].servedBy = 'nope' } "servedBy 'nope' must be the key of a dotnet app" -LoaderOnly
$file = Join-Path $TestRun 'many.json'
$data = Read-Sample
$data.name = ''; $data.apps[0].Remove('project'); $data.apps[2].Remove('dir')
$data | ConvertTo-Json -Depth 10 | Set-Content $file
$message = try { $null = Import-ProjectProfile $file; '' } catch { $_.Exception.Message }
Check "several problems are listed together: '$(($message -split "`n")[0])'" ($message -match 'has 3 problems' -and $message -match 'name is required' -and $message -match 'apps\[0\] \(orders\)\.project is required' -and $message -match 'apps\[2\] \(shop\)\.dir is required')

"== on one machine"
$Ctl = @{ Ports = @{ orders = 5100; billing = 5101 }; Modes = @{ docs = @{ ServedBy = 'orders' } } }
Check "a watch build opens on its server's port: $(Get-AppUrl 'docs')" ((Get-AppUrl 'docs') -eq 'http://localhost:5100')
Check 'an app is on unless lazyrunner.config.json turns it off: "apps": { "<key>": { "enabled": false } }, or "frontend": false for npm apps' (
    (Test-AppEnabled @{ frontend = $true } 'shop') -and -not (Test-AppEnabled @{ frontend = $false } 'shop') -and (Test-AppEnabled @{ frontend = $false } 'orders') -and
    -not (Test-AppEnabled @{ frontend = $true; apps = @{ orders = @{ enabled = $false } } } 'orders') -and (Test-AppEnabled @{ frontend = $false; apps = @{ shop = @{ enabled = $true } } } 'shop'))
$Ctl.Config = @{ stopFrontendOnStop = $false; apps = @{ billing = @{ includeInAll = $false } } }
Check "Stop all leaves out npm apps when stopFrontendOnStop is off, and any app with includeInAll false: $((@($Apps.Keys | Where-Object { -not (Test-InStopAll $_) })) -join ', ')" ((@($Apps.Keys | Where-Object { -not (Test-InStopAll $_) }) -join ',') -eq 'billing,shop,docs,console')

"== variants: the first that applies, on every start"
# Three npm apps: one with a dev server or a watch build, one with just a dev server (no when), one with two dev
# servers. Get-AppMode picks the first variant whose script package.json has.
$sample = Join-Path $TestRun 'variants'
foreach ($dir in 'shop', 'docs', 'admin-ui') { New-Item -ItemType Directory -Force (Join-Path $sample $dir) | Out-Null }
$data = Read-Sample
$data.apps = @(
    $data.apps[0]
    @{ key = 'shop'; name = 'Shop'; type = 'npm'; dir = 'shop'; variants = @(
            @{ when = @{ packageScript = 'serve' }; script = 'serve'; port = 4300 }
            @{ when = @{ packageScript = 'watch' }; script = 'watch'; servedBy = 'orders'; ready = @{ file = 'dist/index.html' } }) }
    @{ key = 'docs'; name = 'Docs'; type = 'npm'; dir = 'docs'; variants = @(@{ script = 'dev'; port = 4400 }) }
    @{ key = 'adminui'; name = 'Admin UI'; type = 'npm'; dir = 'admin-ui'; variants = @(
            @{ label = 'mock'; when = @{ packageScript = 'dev:mock' }; script = 'dev:mock'; port = 4500 }
            @{ label = 'real'; when = @{ packageScript = 'dev' }; script = 'dev'; port = 4501 }) }
)
$multi = Join-Path $TestRun 'multi.json'
$data | ConvertTo-Json -Depth 10 | Set-Content $multi
Check 'the schema takes three npm apps with one or two variants' (Test-Json -Path $multi -SchemaFile $schemaPath -ErrorAction SilentlyContinue)
$loadedMulti = try { Import-ProjectProfile $multi } catch { $_.Exception.Message }
Check "the loader takes them too: $(if ($loadedMulti -is [hashtable]) { @($loadedMulti.Apps.Keys) -join ', ' } else { $loadedMulti })" ($loadedMulti -is [hashtable] -and (@($loadedMulti.Apps.Keys) -join ',') -eq 'orders,shop,docs,adminui')
$Apps = $loadedMulti.Apps
function Set-Scripts([string]$Dir, [hashtable]$Scripts) { @{ name = $Dir; scripts = $Scripts } | ConvertTo-Json | Set-Content (Join-Path $sample "$Dir\package.json") }
Set-Scripts 'shop' @{ watch = 'node w.js' }
Set-Scripts 'docs' @{ dev = 'vite --port 4410' }
Set-Scripts 'admin-ui' @{ dev = 'vite'; 'dev:mock' = 'vite --mode mock' }
$shop = Get-AppMode $sample 'shop'; $docsMode = Get-AppMode $sample 'docs'; $adminui = Get-AppMode $sample 'adminui'
Check "shop has only 'watch': its watch build, served by $($shop.ServedBy), ready at $($shop.ReadyFile)" ($shop.Mode -eq 'build' -and $shop.ServedBy -eq 'orders' -and $shop.ReadyFile -eq 'dist\index.html' -and -not $shop.Port)
Check "docs: its one variant needs no when; --port in the script wins ($($docsMode.Port))" ($docsMode.Mode -eq 'serve' -and $docsMode.Port -eq 4410 -and $docsMode.Label -eq 'dev server')
Check "admin-ui has both scripts: the first variant wins ($($adminui.Label), port $($adminui.Port))" ($adminui.Label -eq 'mock' -and $adminui.Port -eq 4500)
Set-Scripts 'shop' @{ build = 'x' }
Check "no variant matches: none, and the log says why ('$(Get-NoModeReason $sample 'shop')')" ($null -eq (Get-AppMode $sample 'shop') -and (Get-NoModeReason $sample 'shop') -eq 'shop\package.json has neither serve nor watch')
Set-Scripts 'docs' @{ build = 'vite build' }
Check "a variant without when still needs its script: '$(Get-NoModeReason $sample 'docs')'" ($null -eq (Get-AppMode $sample 'docs') -and (Get-NoModeReason $sample 'docs') -eq 'docs\package.json has no dev')
Remove-Item (Join-Path $sample 'docs\package.json')
Check "no package.json: none either ('$(Get-NoModeReason $sample 'docs')')" ($null -eq (Get-AppMode $sample 'docs') -and (Get-NoModeReason $sample 'docs') -eq 'docs\package.json is missing')
Set-Scripts 'shop' @{ watch = 'node w.js' }
$Apps.Remove('orders')   # "apps": { "orders": { "enabled": false } } on this machine
Check "a watch build whose server is turned off doesn't run: '$(Get-NoModeReason $sample 'shop')'" ($null -eq (Get-AppMode $sample 'shop') -and (Get-NoModeReason $sample 'shop') -match 'or orders, which would serve watch, is turned off')

"== a broken profile stops lazyrunner"
# lazyrunner in a folder the broken profile (LAZYRUNNER_PROFILE) matches: just the message, red, no PowerShell code
# excerpt.
$checkout = New-Item -ItemType Directory -Force (Join-Path $TestRun 'checkout')
Set-Content (Join-Path $checkout 'Sample.sln') ''
$env:LAZYRUNNER_PROFILE = $file
Push-Location $checkout
$out = pwsh -NoProfile -File (Join-Path $LazyRunnerRoot 'lazyrunner.ps1') start 2>&1 | Out-String
Pop-Location
Check "lazyrunner stops with just the message, no code excerpt: '$((($out -split "`n") | Where-Object { $_ -match 'lazyrunner:' } | Select-Object -First 1).Trim())'" ($LASTEXITCODE -ne 0 -and $out -match 'lazyrunner: many\.json has 3 problems' -and $out -notmatch 'Line \||throw "')
Remove-Item Env:LAZYRUNNER_PROFILE

Complete-Tests
