# Project profiles, phase 5: the tools for writing one - lazyrunner profile check (fields, schema, the checkout) and
# lazyrunner profile init (a first profile from a checkout) - and the field reference in projects\README.md, which must match
# profile.schema.json. They run lazyrunner itself, on the sample repo. Run: pwsh -NoProfile -File tests\profiletools.Tests.ps1
. "$PSScriptRoot\TestHelpers.ps1"
. (Get-LazyRunnerLoadScript)
$lazyrunner = Join-Path $LazyRunnerRoot 'lazyrunner.ps1'
$repo = & "$PSScriptRoot\fixtures\New-SampleRepo.ps1" -Path (Join-Path $TestRun 'repo')
$sampleJson = Join-Path $PSScriptRoot 'fixtures\sample.json'

# lazyrunner profile … in a folder: @{ Code; Out (stdout); Err (stderr: init's notes when stdout is redirected) }.
function Invoke-LazyRunner([string]$Dir, [string[]]$Arguments) {
    Push-Location $Dir
    try {
        $all = @(pwsh -NoProfile -File $lazyrunner profile @Arguments 2>&1)
        @{ Code = $LASTEXITCODE; Out = (@($all | Where-Object { $_ -isnot [Management.Automation.ErrorRecord] }) -join "`n"); Err = (@($all | Where-Object { $_ -is [Management.Automation.ErrorRecord] }) -join "`n") }
    }
    finally { Pop-Location }
}
function Get-Check([string]$Dir, [string]$File) {
    $result = Invoke-LazyRunner $Dir (@('check') + @($File | Where-Object { $_ }) + '-Json')
    $json = $result.Out | ConvertFrom-Json -AsHashtable
    $json.code = $result.Code
    $json
}
function Save-Profile($Data, [string]$Path) { $Data | ConvertTo-Json -Depth 10 | Set-Content $Path }

"== the field reference in projects\README.md"
$readme = (Get-Content (Join-Path $LazyRunnerRoot 'projects\README.md') -Raw) -replace "`r`n", "`n"
$section = [regex]::Match($readme, '(?s)<!-- reference: [^\n]*-->\n(.*?)\n<!-- end of reference -->').Groups[1].Value.Trim()
$reference = Get-ProfileReference
Check "it is made from profile.schema.json ($(@($reference -split "`n" | Where-Object { $_ -match '^\| `' }).Count) fields) - run projects\Update-Reference.ps1 after changing the schema" ($section -eq $reference.Trim())
$fields = @([regex]::Matches($reference, '(?m)^\| `([^`]+)`') | ForEach-Object { ($_.Groups[1].Value -split '\.')[-1] }) | Sort-Object -Unique
$known = @($ProfileFields.Values | ForEach-Object { $_ }) | Sort-Object -Unique
Check "it lists exactly the fields the loader knows ($($known.Count))" ((@($fields) -join ',') -eq (@($known) -join ','))

"== unknown fields"
$data = Get-Content $sampleJson -Raw | ConvertFrom-Json -AsHashtable
$data.apps[0].swager = '/swagger'; $data.apps[2].variants[0].prot = 1; $data.colour = 'blue'
$typo = Join-Path $TestRun 'typo.json'; Save-Profile $data $typo
$message = try { $null = Import-ProjectProfile $typo; '' } catch { $_.Exception.Message }
Check "a typo gets a suggestion: '$((($message -split "`n") | Where-Object { $_ -match 'swager' }).Trim(' ', '-'))'" ($message -match 'apps\[0\] \(orders\)\.swager is not a profile field - did you mean swagger\?')
Check "  in a variant too: '$((($message -split "`n") | Where-Object { $_ -match 'prot' }).Trim(' ', '-'))'" ($message -match 'variants\[0\]\.prot is not a profile field - did you mean port\?')
Check "  anything else lists what there is: '$((($message -split "`n") | Where-Object { $_ -match 'colour' }).Trim(' ', '-'))'" ($message -match 'colour is not a profile field - it can have: \$schema, schemaVersion')

"== lazyrunner profile check"
Copy-Item $sampleJson (Join-Path $repo '.lazyrunner.json')
$check = Get-Check $repo
$levels = @($check.problems | ForEach-Object level)
Check "the sample repo's .lazyrunner.json: ok $($check.ok), exit $($check.code), problems: $(if ($levels) { $levels -join ', ' } else { 'none' }) (its mock variant is for other branches)" ($check.ok -and $check.code -eq 0 -and (@($levels | Where-Object { $_ -ne 'info' }).Count -eq 0) -and $check.source -eq 'repo' -and $check.root -eq $repo)
$text = Invoke-LazyRunner $repo @('check')
Check "as text: '$(($text.Out -split "`n")[-1])'" ($text.Code -eq 0 -and $text.Out -match 'OK - 5 apps: Orders, Billing, Shop, Docs, Console')
$data = Get-Content $sampleJson -Raw | ConvertFrom-Json -AsHashtable
$data.apps[0].launchProfile = 'Ordres'
$data.apps[1].project = 'services/Billing/Billing2.csproj'
$data.apps[3].variants[0].script = 'serve'
$data.apps[4].variants[1].port = 58301   # Shop's port
$data.apps[4].variants[0].when.packageScript = 'dev:mock'
$broken = Join-Path $repo 'broken.lazyrunner.json'; Save-Profile $data $broken
$check = Get-Check $repo $broken
$at = @($check.problems | ForEach-Object { "$($_.level) $($_.at)" })
function Find-Problem([string]$Pattern) { @($check.problems | Where-Object { "$($_.level) $($_.at): $($_.message)" -match $Pattern })[0] }
function Format-Problem($Problem) { if ($Problem) { "$($Problem.at): $($Problem.message)" } else { '(not found)' } }
Check "a broken one fails (exit $($check.code)), each problem naming its field: $($at -join '; ')" (-not $check.ok -and $check.code -eq 1)
$p = Find-Problem "^error apps\[0\] \(orders\)\.launchProfile: 'Ordres' is not in .*launchSettings\.json \(it has: Orders\)"
Check "  a launch profile that isn't there: '$(Format-Problem $p)'" ([bool]$p)
$p = Find-Problem '^error apps\[1\] \(billing\)\.project: services\\Billing\\Billing2\.csproj doesn''t exist'
Check "  a project that isn't there: '$(Format-Problem $p)'" ([bool]$p)
$p = Find-Problem "^error apps\[3\] \(docs\)\.variants\[0\]\.script: .*package\.json has no script 'serve'"
Check "  a script that isn't there: '$(Format-Problem $p)'" ([bool]$p)
$p = Find-Problem '^info apps\[4\] \(console\)\.variants\[0\]: .*has no dev:mock on this branch'
Check "  a variant for other branches is information, not an error: '$(Format-Problem $p)'" ([bool]$p)
$p = Find-Problem '^error apps: Shop and Console both use port 58301'
Check "  two apps on one port: '$(Format-Problem $p)'" ([bool]$p)
$elsewhere = New-Item -ItemType Directory -Force (Join-Path $TestRun 'elsewhere')
$check = Get-Check $elsewhere.FullName $sampleJson
Check "a profile checked outside its repo: '$($check.problems[0].message)'" ($check.ok -and $check.problems[0].level -eq 'info' -and $check.problems[0].message -match "no folder from here up has Sample\.sln")
$check = Get-Check (Join-Path $repo 'services') $sampleJson
Check "inside its repo, a profile file from elsewhere finds the checkout by its detect file ($($check.root))" ($check.ok -and $check.root -eq $repo -and $check.source -eq 'given')
Remove-Item $broken
$nowhere = Invoke-LazyRunner $elsewhere.FullName @('check')
Check "no profile here: '$($nowhere.Out.Trim())'" ($nowhere.Code -eq 1 -and $nowhere.Out -match 'no profile applies here')

"== lazyrunner profile init"
Remove-Item (Join-Path $repo '.lazyrunner.json')
$init = Invoke-LazyRunner $repo @('init')
$proposal = $init.Out | ConvertFrom-Json -AsHashtable
$apps = @($proposal.apps)
Check "it proposes the runnable apps: $(($apps | ForEach-Object { "$($_.key) ($($_.type))" }) -join ', ')" ($init.Code -eq 0 -and (@($apps.key) -join ',') -eq 'billing,orders,console,shop')
Check "with their launch profiles and the ports their scripts name: $(($apps | ForEach-Object { if ($_.variants) { "$($_.key) $($_.variants[0].port)" } else { "$($_.key) $($_.launchProfile)" } }) -join ', ')" ($apps[0].launchProfile -eq 'Billing' -and $apps[2].variants[0].port -eq 58303 -and $apps[3].variants[0].port -eq 58301)
Check "watch: the folder the .NET projects share ($($proposal.watch)); detect: the solution ($($proposal.detect.anyFile))" ($proposal.watch[0] -eq 'services' -and $proposal.detect.anyFile[0] -eq 'Sample.sln')
Check "its notes go to stderr, the profile alone to stdout: '$((($init.Err -split "`n") | Where-Object { $_ -match 'docs' }).Trim(' ', '-'))'" ($init.Err -match 'web/docs/package\.json has no dev, start or serve script but has watch')
$saved = Invoke-LazyRunner $repo @('init', '-Save')
Check "-Save writes .lazyrunner.json, which passes check" ($saved.Code -eq 0 -and (Test-Path (Join-Path $repo '.lazyrunner.json')) -and (Get-Check $repo).ok)
$again = Invoke-LazyRunner $repo @('init', '-Save')
Check "it never overwrites one: '$($again.Err.Trim())'" ($again.Code -eq 1 -and $again.Err -match 'exists already')
$empty = Invoke-LazyRunner $elsewhere.FullName @('init', $elsewhere.FullName)
Check "nothing to run: '$((($empty.Err -split "`n")[0]).Trim())'" ($empty.Code -eq 1 -and $empty.Err -match 'found nothing to run')

"== what init reads from the repo"
# A worker without launch settings (it used to stop init) and a test project: skipped, and watch still covers the
# .NET code without the tests.
New-Item -ItemType Directory -Force (Join-Path $repo 'services\Worker'), (Join-Path $repo 'tests\Shop.Tests\Properties') | Out-Null
Set-Content (Join-Path $repo 'services\Worker\Worker.csproj') '<Project Sdk="Microsoft.NET.Sdk.Worker"></Project>'
Set-Content (Join-Path $repo 'tests\Shop.Tests\Shop.Tests.csproj') '<Project Sdk="Microsoft.NET.Sdk"><ItemGroup><PackageReference Include="Microsoft.NET.Test.Sdk" Version="17.11.1" /></ItemGroup></Project>'
Set-Content (Join-Path $repo 'tests\Shop.Tests\Properties\launchSettings.json') '{ "profiles": { "Shop.Tests": { "commandName": "Project", "applicationUrl": "http://localhost:58499" } } }'
$init = Invoke-LazyRunner $repo @('init')
$proposal = $init.Out | ConvertFrom-Json -AsHashtable
Check "a worker without launch settings is skipped with a note, test projects left out: $(@($proposal.apps.key) -join ', ')" ($init.Code -eq 0 -and $init.Err -match 'skipped services/Worker/Worker\.csproj' -and (@($proposal.apps.key) -join ',') -eq 'billing,orders,console,shop')
Check "watch is the .NET code without the tests: $($proposal.watch)" ($proposal.watch[0] -eq 'services')
$client = New-Item -ItemType Directory -Force (Join-Path $TestRun 'client')
Set-Content (Join-Path $client 'server.mjs') "import http from 'node:http';`nconst PORT = 58410;`nhttp.createServer().listen(PORT);"
$fromCode = Find-ConfigPort $client.FullName 'node server.mjs'
Set-Content (Join-Path $client 'vite.config.ts') 'export default { server: { port: 5300 } }'
$fromVite = Find-ConfigPort $client.FullName 'vite'
Remove-Item (Join-Path $client 'vite.config.ts'); Set-Content (Join-Path $client '.env') "API=x`nPORT=4100"
$fromEnv = Find-ConfigPort $client.FullName 'vite'
Remove-Item (Join-Path $client '.env')
Check "a port the repo's files name: the server code $fromCode, vite.config $fromVite, .env $fromEnv, none: '$(Find-ConfigPort $client.FullName 'vite')'" ($fromCode -eq 58410 -and $fromVite -eq 5300 -and $fromEnv -eq 4100 -and $null -eq (Find-ConfigPort $client.FullName 'vite'))
$Apps = [ordered]@{ tests = @{ Name = 'Tests'; Kind = 'dotnet'; Project = 'tests\Shop.Tests\Shop.Tests.csproj'; Profile = 'Shop.Tests' } }
$warning = @(Test-AppSetup $repo 'tests' 'apps[0] (tests)' | Where-Object Level -eq 'warning')[0]
Check "check warns about a test project in apps: '$($warning.Message)'" ($warning.At -eq 'apps[0] (tests).project' -and $warning.Message -match 'is a test project')

Complete-Tests
