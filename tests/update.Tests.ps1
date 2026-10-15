# Self-update: versions and the changelog, and lazyrunner update against a local release repo - a git-clone install
# fast-forwards (never over local changes or commits), a copied install updates from a cache clone and keeps your
# own files, and the other answers (up to date, ahead, a bad repo, auto-update off).
# Run: pwsh -NoProfile -File tests\update.Tests.ps1
. "$PSScriptRoot\TestHelpers.ps1"
. "$PSScriptRoot\fixtures\New-ReleaseRepo.ps1"
. (Get-LazyRunnerLoadScript)
New-Item -ItemType Directory -Force $StateDir | Out-Null

# lazyrunner <command> from an install: its output as one string, and its exit code.
function Invoke-Install([string]$Install, [string]$Command) {
    $out = pwsh -NoProfile -File (Join-Path $Install 'lazyrunner.ps1') $Command 2>&1 | Out-String
    @{ Out = $out; Code = $LASTEXITCODE }
}
function Get-InstallVersion([string]$Install) { (Get-Content (Join-Path $Install 'VERSION') -Raw).Trim() }
function Set-TestConfig([hashtable]$Values) { $Values | ConvertTo-Json | Set-Content $env:LAZYRUNNER_CONFIG_PATH }
function Find-Line([string]$Text, [string]$Pattern) { (($Text -split "`r?`n") | Where-Object { $_ -match $Pattern } | Select-Object -First 1).Trim() }

"== versions and the changelog"
Check "versions: '1.2.3', 'v1.2.3', '1.2.3-beta' are 1.2.3; 'main' is none" ((ConvertTo-Version '1.2.3') -eq [version]'1.2.3' -and (ConvertTo-Version 'v1.2.3') -eq [version]'1.2.3' -and (ConvertTo-Version '1.2.3-beta') -eq [version]'1.2.3' -and $null -eq (ConvertTo-Version 'main'))
$changelog = "# Changelog`n`nIntro.`n`n## [0.3.0] - 2026-10-17`n`n### Added`n- Third`n`n## [0.2.0] - 2026-10-16`n- Second`n`n## 0.1.0`n- First`n"
$entries = @(Get-ChangelogEntries $changelog '0.1.0' '0.3.0')
Check "the entries after 0.1.0 up to 0.3.0: $(($entries | ForEach-Object Version) -join ', '), newest first, with their dates" ((($entries | ForEach-Object Version) -join ',') -eq '0.3.0,0.2.0' -and $entries[0].Date -eq '2026-10-17')
$lines = @(Format-ChangelogLines $entries)
Check "shown as: '$($lines[0].Text)' / '$($lines[1].Text)' / '$($lines[2].Text)'" ($lines[0].Text -eq 'v0.3.0 · 2026-10-17' -and $lines[1].Text -eq 'Added' -and $lines[2].Text -eq '  • Third')
$script:VersionPath = Join-Path $TestRun 'VERSION'; $script:ChangelogPath = Join-Path $TestRun 'CHANGELOG.md'
Set-Content $VersionPath '0.1.0'; Set-Content $ChangelogPath $changelog
Check "what's new on the first run: nothing (and v$(Get-LazyRunnerVersion) is remembered)" (@(Get-WhatsNew).Count -eq 0 -and (Read-Json $SeenVersionPath).seen -eq '0.1.0')
Set-Content $VersionPath '0.3.0'
$news = @(Get-WhatsNew)
Check "after an update to v0.3.0: $(($news | ForEach-Object Version) -join ', '); then nothing again" ((($news | ForEach-Object Version) -join ',') -eq '0.3.0,0.2.0' -and @(Get-WhatsNew).Count -eq 0)
$progress = Get-FetchProgress "remote: Counting objects: 100% (5/5)`rReceiving objects:  40% (12/30), 1.20 MiB | 2.00 MiB/s`r"
Check "git's fetch progress as one bar: $([int]$progress.Percent)%, $($progress.Size)" ([int]$progress.Percent -eq 40 -and $progress.Size -eq '1.20 MiB')

"== where updates come from"
Set-TestConfig @{ }
Check 'no repo configured: no source' ($null -eq (Get-UpdateSource))
Set-TestConfig @{ updateRepoUrl = 'https://example.com/x.git'; updateBranch = 'release'; autoUpdate = $false }
$source = Get-UpdateSource
Check "the config's repo and branch: $($source.Url) ($($source.Branch))" ($source.Url -eq 'https://example.com/x.git' -and $source.Branch -eq 'release')
$result = Update-LazyRunner
Check "auto-update off: nothing checked ('$($result.Detail)')" ($result.Status -eq 'skip' -and $result.Detail -match 'auto-update off')

"== a git clone of the release repo"
$release = New-ReleaseRepo (Join-Path $TestRun 'release') '0.1.0'
$install = Join-Path $TestRun 'install'
Invoke-TestGit $TestRun @('clone', '--quiet', $release.Remote, $install) | Out-Null
Set-TestConfig @{ updateRepoUrl = $release.Remote; autoUpdate = $true }
$run = Invoke-Install $install 'version'
Check "lazyrunner version names the repo: '$(Find-Line $run.Out 'updates from')'" ($run.Out -match 'lazyrunner v0\.1\.0' -and $run.Out -match [regex]::Escape("updates from $($release.Remote) (main), auto-update on"))
$run = Invoke-Install $install 'update'
Check "nothing new: '$(Find-Line $run.Out 'up to date')'" ($run.Out -match 'v0\.1\.0 · up to date')
Publish-Version $release '0.2.0' 'Something new'
$run = Invoke-Install $install 'update'
Check "v0.2.0 published: '$(Find-Line $run.Out 'updated')', and what's new is shown" ($run.Out -match 'updated v0\.1\.0 → v0\.2\.0' -and $run.Out -match 'Something new' -and (Get-InstallVersion $install) -eq '0.2.0')
Check 'the clone fast-forwarded (its HEAD is the release)' ((git -C $install rev-parse HEAD) -eq (git -C $release.Work rev-parse HEAD))
Check 'the update is in lazyrunner.log' ((Get-Content $LogPath -Raw) -match 'updated lazyrunner v0\.1\.0 -> v0\.2\.0')
$run = Invoke-Install $install 'update'
Check 'checking again downloads nothing' ($run.Out -match 'up to date' -and $run.Out -notmatch 'downloading')
Add-Content (Join-Path $install 'README.md') 'my note'
Publish-Version $release '0.3.0' 'Third'
$run = Invoke-Install $install 'update'
Check "local changes are never overwritten: '$(Find-Line $run.Out 'not installed')'" ($run.Out -match 'v0\.3\.0 is out, but not installed: it has local changes' -and (Get-InstallVersion $install) -eq '0.2.0')
Invoke-TestGit $install @('checkout', '--quiet', '--', 'README.md') | Out-Null
$run = Invoke-Install $install 'update'
Check 'without them, it updates' ($run.Out -match 'updated v0\.2\.0 → v0\.3\.0' -and (Get-InstallVersion $install) -eq '0.3.0')
Add-Content (Join-Path $install 'README.md') 'my commit'
Invoke-TestGit $install @('commit', '--quiet', '-am', 'mine') | Out-Null
Publish-Version $release '0.4.0' 'Fourth'
$run = Invoke-Install $install 'update'
Check "nor commits of its own: '$(Find-Line $run.Out 'not installed')'" ($run.Out -match 'commits of its own' -and (Get-InstallVersion $install) -eq '0.3.0')

"== a copied folder (no .git)"
$copy = Join-Path $TestRun 'copy'
New-Item -ItemType Directory -Force $copy | Out-Null
git -C $release.Work archive --format=zip -o (Join-Path $TestRun 'v1.zip') HEAD~3
Expand-Archive (Join-Path $TestRun 'v1.zip') $copy
Set-Content (Join-Path $copy 'themes\mine.json') '{ "name": "Mine" }'
Check "the copy is v$(Get-InstallVersion $copy), with a theme of its own" ((Get-InstallVersion $copy) -eq '0.1.0')
$run = Invoke-Install $copy 'update'
Check "it updates from a cache clone: '$(Find-Line $run.Out 'updated')'" ($run.Out -match 'updated v0\.1\.0 → v0\.4\.0' -and (Get-InstallVersion $copy) -eq '0.4.0' -and (Test-Path (Join-Path $StateDir 'update\.git')))
Check 'your own files stay' ((Test-Path (Join-Path $copy 'themes\mine.json')) -and -not (Test-Path (Join-Path $copy '.git')))
Set-Content (Join-Path $copy 'VERSION') '9.0.0'
$run = Invoke-Install $copy 'update'
Check "a newer local version: '$(Find-Line $run.Out 'ahead')'" ($run.Out -match 'v9\.0\.0 · ahead of the released v0\.4\.0')

"== a repo that isn't there"
Set-TestConfig @{ updateRepoUrl = (Join-Path $TestRun 'nope.git'); autoUpdate = $true }
$run = Invoke-Install $copy 'update'
Check "it says why: '$(Find-Line $run.Out 'check for updates')'" ($run.Out -match "couldn't check for updates: no repo at" -and $run.Code -eq 0)

Complete-Tests
