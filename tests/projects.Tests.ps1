# Which profile a folder gets (a repo's .lazyrunner.json, yours, bundled), a repo profile that extends a known one,
# trusting repo profiles, settings and state per project, and noticing a branch that changes the profile.
# Run: pwsh -NoProfile -File tests\projects.Tests.ps1
. "$PSScriptRoot\TestHelpers.ps1"
. (Get-LazyRunnerLoadScript)
$userDir = $env:LAZYRUNNER_PROFILES_DIR
New-Item -ItemType Directory -Force $userDir, $StateDir | Out-Null
$sampleJson = Join-Path $PSScriptRoot 'fixtures\sample.json'
$schemaPath = Join-Path $LazyRunnerRoot 'projects\profile.schema.json'

# A folder with these (empty) files.
function New-Folder([string]$Path, [string[]]$Files) {
    New-Item -ItemType Directory -Force $Path | Out-Null
    foreach ($file in $Files) {
        $full = Join-Path $Path $file
        New-Item -ItemType Directory -Force (Split-Path $full) | Out-Null
        Set-Content $full ''
    }
    $Path
}
function Read-Profile([string]$Path) { Get-Content $Path -Raw | ConvertFrom-Json -AsHashtable }
function Save-Profile($Data, [string]$Path) { $Data | ConvertTo-Json -Depth 10 | Set-Content $Path }
function Get-Project([string]$Dir) { Resolve-ProjectProfile (Find-ProjectProfile $Dir $LazyRunnerRoot) $LazyRunnerRoot }

"== which profile a folder gets"
# A lazyrunner folder of its own, with sample.json bundled in its projects\.
$appRoot = New-Folder (Join-Path $TestRun 'app') @()
New-Item -ItemType Directory -Force (Join-Path $appRoot 'projects') | Out-Null
Copy-Item $sampleJson (Join-Path $appRoot 'projects')
$sampleRepo = New-Folder (Join-Path $TestRun 'sample') 'Sample.sln', 'web\shop\src\main.ts'
$found = Find-ProjectProfile (Join-Path $sampleRepo 'web\shop\src') $appRoot
Check "a folder deep in a repo with Sample.sln: the bundled $($found.Data.id) profile, root found by walking up" ($found.Source -eq 'bundled' -and $found.Root -eq $sampleRepo -and $found.Data.id -eq 'sample')
Check 'a folder no profile knows: none' ($null -eq (Find-ProjectProfile (New-Folder (Join-Path $TestRun 'nothing') 'readme.md') $appRoot))
$mine = Read-Profile $sampleJson; $mine.name = 'Sample (mine)'
Save-Profile $mine (Join-Path $userDir 'sample.json')
$found = Find-ProjectProfile $sampleRepo $appRoot
Check "yours ($userDir) replaces a bundled one with the same id: '$($found.Data.name)', from $($found.Source)" ($found.Source -eq 'user' -and $found.Data.name -eq 'Sample (mine)')
Copy-Item $sampleJson $userDir -Force
$found = Find-ProjectProfile $sampleRepo $LazyRunnerRoot
Check "with nothing bundled, yours count on their own: $($found.Data.id), from $($found.Source)" ($found.Source -eq 'user' -and $found.Data.id -eq 'sample')
$own = Read-Profile $sampleJson; $own.id = 'shop'; $own.name = 'Shop repo'; $own.Remove('detect')
Save-Profile $own (Join-Path $sampleRepo '.lazyrunner.json')
$found = Find-ProjectProfile $sampleRepo $LazyRunnerRoot
$project = Resolve-ProjectProfile $found $LazyRunnerRoot
Check "the repo's own .lazyrunner.json wins: $($project.Id), from $($project.Source)" ($found.Source -eq 'repo' -and $project.Id -eq 'shop' -and $project.Root -eq $sampleRepo)
Check "it needs no detect: the file itself marks the repo ($($project.RepoMarker))" ($project.RepoMarker -eq '.lazyrunner.json')
Check 'the schema takes it without detect' (Test-Json -Path (Join-Path $sampleRepo '.lazyrunner.json') -SchemaFile $schemaPath -ErrorAction SilentlyContinue)

"== a repo profile that extends a known one"
$team = @{
    schemaVersion = 1; extends = 'sample'; name = 'Sample (team)'
    apps = @(
        @{ key = 'shop'; name = 'Storefront' }
        @{ key = 'blog'; name = 'Blog'; type = 'npm'; dir = 'web/blog'; variants = @(@{ script = 'dev'; port = 58310 }) }
    )
}
Save-Profile $team (Join-Path $sampleRepo '.lazyrunner.json')
Check 'the schema takes just the changes' (Test-Json -Path (Join-Path $sampleRepo '.lazyrunner.json') -SchemaFile $schemaPath -ErrorAction SilentlyContinue)
$project = Get-Project $sampleRepo
Check "its fields replace the base's: '$($project.Name)', id $($project.Id) kept" ($project.Name -eq 'Sample (team)' -and $project.Id -eq 'sample')
Check "apps merge by key: $(@($project.Apps.Keys) -join ', '); shop is now '$($project.Apps.shop.Name)' and keeps its dir $($project.Apps.shop.Dir)" ((@($project.Apps.Keys) -join ',') -eq 'orders,billing,shop,docs,console,blog' -and $project.Apps.shop.Name -eq 'Storefront' -and $project.Apps.shop.Dir -eq 'web\shop')
$team.extends = 'nope'; Save-Profile $team (Join-Path $sampleRepo '.lazyrunner.json')
$message = try { $null = Get-Project $sampleRepo; '' } catch { $_.Exception.Message }
$short = $message -replace ' \(in .*', ''
Check "extending an unknown profile: '$short'" ($message -match "extends 'nope', which is not a known profile")
$team.extends = 'sample'; Save-Profile $team (Join-Path $sampleRepo '.lazyrunner.json')
$elsewhere = Join-Path $TestRun 'extends.json'; Save-Profile ((Read-Profile $sampleJson) + @{ extends = 'other' }) $elsewhere
$message = try { $null = Import-ProjectProfile $elsewhere; '' } catch { $_.Exception.Message }
Check "extends anywhere else: '$(($message -split "`n")[-1].Trim(' ', '-'))'" ($message -match "extends is only for a repo's \.lazyrunner\.json")

"== trusting a repo profile"
$project = Get-Project $sampleRepo
Check 'a repo profile is not trusted until you say so' (-not (Test-ProfileTrusted $project $TrustStorePath))
$asked = [Collections.Generic.List[string]]::new()
$answer = Confirm-ProfileTrust $project { param($Prompt) $asked.Add($Prompt); 'n' } 6>$null
Check "lazyrunner asks ('$($asked[0])'); no: not used, still not trusted" (-not $answer -and $asked.Count -eq 1 -and -not (Test-ProfileTrusted $project $TrustStorePath))
$summary = @(Get-ProfileSummary $project)
Check "it shows what would run, one line per app: '$($summary[0].Trim())' … '$($summary[2].Trim())'" ($summary.Count -eq 6 -and $summary[0] -match 'dotnet build \+ run services\\Orders\\Orders\.csproj' -and $summary[2] -match 'npm run dev in web\\shop')
$answer = Confirm-ProfileTrust $project { 'y' } 6>$null
Check 'yes: trusted, and not asked again' ($answer -and (Test-ProfileTrusted $project $TrustStorePath) -and (Confirm-ProfileTrust $project { throw 'asked again' } 6>$null))
$team.name = 'Sample (changed)'; Save-Profile $team (Join-Path $sampleRepo '.lazyrunner.json')
$changed = Get-Project $sampleRepo
Check 'once the file changes, lazyrunner asks again' (-not (Test-ProfileTrusted $changed $TrustStorePath))
$bundled = Resolve-ProjectProfile @{ Path = (Join-Path $appRoot 'projects\sample.json'); Source = 'bundled'; Root = $sampleRepo } $appRoot
$yours = Resolve-ProjectProfile @{ Path = (Join-Path $userDir 'sample.json'); Source = 'user'; Root = $sampleRepo } $LazyRunnerRoot
Check 'bundled and your own profiles need no trust' ((Test-ProfileTrusted $bundled $TrustStorePath) -and (Test-ProfileTrusted $yours $TrustStorePath))
$out = pwsh -NoProfile -File (Join-Path $LazyRunnerRoot 'lazyrunner.ps1') __ui -Root $sampleRepo 2>&1 | Out-String
Check "lazyrunner won't run an untrusted one: '$($out.Trim())'" ($LASTEXITCODE -eq 1 -and $out -match "isn't trusted yet")

"== settings per project"
# Two projects: sample (the repo's own profile) and other (the same apps under another id).
$otherData = Read-Profile $sampleJson; $otherData.id = 'other'; $otherData.name = 'Other'
$otherFile = Join-Path $TestRun 'other.json'; Save-Profile $otherData $otherFile
@{ theme = 'dark'; projects = @{ other = @{ frontend = $false } } } | ConvertTo-Json -Depth 5 | Set-Content $ConfigPath
Use-ProjectProfile $changed
$null = Set-ConfigValue 'columnWidths' @{ shop = 60 }
$saved = Read-Json $ConfigPath
Check "a project's setting is saved in its section: projects.sample.columnWidths.shop = $($saved.projects.sample.columnWidths.shop), not at the top" ($saved.projects.sample.columnWidths.shop -eq 60 -and -not $saved.ContainsKey('columnWidths'))
$null = Set-ConfigValue 'theme' 'light'
$saved = Read-Json $ConfigPath
Check 'a setting for every project stays at the top' ($saved.theme -eq 'light' -and -not $saved.projects.sample.ContainsKey('theme'))
Use-ProjectProfile (Import-ProjectProfile $otherFile)
$config = Get-Config
Check "another project has its own (widths: $($config.columnWidths.Count); frontend off, so its apps are $(@($Apps.Keys) -join ', ')) and shares the rest (theme $($config.theme))" ($config.columnWidths.Count -eq 0 -and (@($Apps.Keys) -join ',') -eq 'orders,billing' -and $config.theme -eq 'light')

"== state per project"
Use-ProjectProfile $changed
Initialize-ProjectState
$sampleState = $ProjectStateDir
Use-ProjectProfile (Import-ProjectProfile $otherFile)
Initialize-ProjectState
Check "each project gets its own state folder: $(Split-Path -Leaf $sampleState)\, $(Split-Path -Leaf $ProjectStateDir)\" ((Test-Path $sampleState) -and (Test-Path $ProjectStateDir) -and $sampleState -ne $ProjectStateDir -and (Split-Path $sampleState) -eq $StateDir)

"== a branch that changes the profile"
Approve-ProjectProfile $changed $TrustStorePath
Use-ProjectProfile $changed
$script:Ctl = @{ Root = $sampleRepo; Log = [Collections.Generic.List[object]]::new(); ProfileChanged = $false }
Test-ProfileChange
Check 'the same profile: nothing to say' ($Ctl.Log.Count -eq 0 -and -not $Ctl.ProfileChanged)
$team.apps += @{ key = 'wiki'; name = 'Wiki'; type = 'npm'; dir = 'web/wiki'; variants = @(@{ script = 'dev'; port = 58320 }) }
$team.apps[0].name = 'Store'
Save-Profile $team (Join-Path $sampleRepo '.lazyrunner.json')
Test-ProfileChange
$line = "$($Ctl.Log[-1].Text)"
Check "a branch with another one: '$line'" ($Ctl.ProfileChanged -and $line -match '\+ Wiki' -and $line -match '~ Store' -and $line -match ', then p reopens lazyrunner with it' -and $line -match 'needs your trust first')
Remove-Item (Join-Path $sampleRepo '.lazyrunner.json')
Test-ProfileChange
$line = "$($Ctl.Log[-1].Text)"
Check "a branch without it gets your sample.json: '$line'" ($line -match 'sample\.json' -and $line -match '- blog')

Complete-Tests
