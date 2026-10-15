# lazyrunner tests

Plain PowerShell scripts, no Pester needed. Each prints one `OK` / `FAIL` line per check and exits 1 when a check
failed. They keep their state, config and profiles apart from a real lazyrunner session (`TestHelpers.ps1`), so they
can run while lazyrunner is open.

| Script | What | Time |
|---|---|---|
| `profile.Tests.ps1` | A profile loads into the settings lazyrunner runs with; no code compares an app type outside `lib\apptypes.ps1`; the schema; broken profiles stop lazyrunner with a clear message naming the field; per-app machine settings; several npm apps and the choice of variant | ~10 s |
| `projects.Tests.ps1` | Which profile a folder gets (a repo's `.lazyrunner.json`, your profiles folder, bundled), `extends`, the trust question (and asking again after a change), settings and state per project, and a branch that changes the profile | ~10 s |
| `profiletools.Tests.ps1` | `lazyrunner profile check` on the sample repo (OK, then a broken profile: missing launch profile, project, script, a variant for other branches, a port clash, each naming its field; `-Json`), checking from outside or inside the repo, `lazyrunner profile init` (apps, ports from scripts and files, `watch`, `detect`, notes, `-Save` never overwrites), unknown fields with suggestions, and that the reference in `projects\README.md` matches the schema | ~20 s |
| `sample.e2e.ps1` | lazyrunner on the sample repo (2 .NET APIs, 3 npm apps; profile `fixtures\sample.json`) in a minimized window: U starts all five and each gets ready, 5 focuses the fifth column, S keeps the app with `includeInAll: false` running | ~30 s |
| `update.Tests.ps1` | Self-update against a local release repo: versions and the changelog, what's new, where updates come from; a git clone that updates (and refuses over local changes or commits of its own), a copied folder that updates from a cache clone and keeps its own files, up to date, ahead, a repo that isn't there, auto-update off | ~25 s |
| `update.e2e.ps1` | lazyrunner v0.1.0 opens while v0.2.0 is published: the startup checklist installs it, lazyrunner reopens on it, and the new version's checklist shows what's new | ~25 s |
| `projects.e2e.ps1` | The real `lazyrunner` command in repo A (a trusted `.lazyrunner.json`): `>` saves a width for A; `lazyrunner start` for repo B (a profile in your folder) reopens it for B; each keeps its own settings and state folder; `q` ends `lazyrunner` | ~40 s |

```powershell
pwsh -NoProfile -File tests\profile.Tests.ps1
pwsh -NoProfile -File tests\projects.Tests.ps1
pwsh -NoProfile -File tests\profiletools.Tests.ps1
pwsh -NoProfile -File tests\sample.e2e.ps1
pwsh -NoProfile -File tests\projects.e2e.ps1
pwsh -NoProfile -File tests\update.Tests.ps1
pwsh -NoProfile -File tests\update.e2e.ps1
```

The tests set `LAZYRUNNER_PROFILES_DIR` to an empty folder of their own, so your profiles folder never takes part.
The `*.e2e.ps1` tests need the .NET SDK and Node.js, which the apps they run need anyway.

**`fixtures\New-ReleaseRepo.ps1`** (dot-source it): `New-ReleaseRepo` makes a bare release repo from this checkout's
files at a version, and `Publish-Version` pushes a higher one with a changelog entry.

**`fixtures\New-SampleRepo.ps1 -Path <folder>`** creates the sample repo for `fixtures\sample.json`: two ASP.NET APIs
(Orders 58201, Billing 58202) and three npm apps without packages - Shop (a dev server on 58301), Docs (a watch build
Orders serves) and Console (two variants; its package.json has only the second, a dev server on 58303).
