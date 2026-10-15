# lazyrunner

A terminal UI that runs your project's .NET APIs and npm frontends side by side, the lazygit way: it builds what
changed, starts everything on the right ports, tells you when each app is up, and keeps their logs in columns you can
scroll, search and zoom. One command instead of a row of terminal tabs.

```
 ◆ Orders :58201 ● 12 ms up in 3s ┬ ■ Billing :58202 ● 9 ms up in 3s ┬ ■ Shop :58301 ● 4 ms ┬ ■ Docs ● built, watching
 info: Microsoft.Hosting.Lifetime…  │ info: Microsoft.Hosting.Lifetime… │ Local: http://localh… │ Build complete. Watching…
──────────────────────────────────────────────────────────────────────────────────────────────────────────────────────
 lazyrunner  ● all up (4)   ⎇ main   DB shop @ localhost   C:\src\shop
 space start/stop  u/U start  s/S stop  r/R restart  b/B rebuild  d database  w open  x menu  / search  , settings  q quit
 10:27:54 Ready in 9s
```

- **.NET apps** are built with `dotnet build` (one at a time, skipped when nothing changed) and run with their launch
  profile; the port comes from `launchSettings.json`.
- **npm apps** run their dev server or watch build; which script runs can differ per branch.
- Every app runs inside a Windows job object: `q`, or even closing the terminal, leaves nothing running.
- What a project runs is described in a small JSON **profile** - in the repo (`.lazyrunner.json`) or in your own
  folder. `lazyrunner profile init` writes a first one for you.

Windows only (job objects, Windows Terminal integration). Needs PowerShell 7.

## Install

1. **Get the folder:** clone this repo (or unzip it) anywhere, e.g. `Documents\PowerShell\Scripts\lazyrunner`.
2. **Downloaded as a zip?** Windows blocks downloaded scripts; unblock them once, in that folder:

   ```powershell
   Get-ChildItem -Recurse | Unblock-File
   ```

   If PowerShell refuses to run scripts at all: `Set-ExecutionPolicy -Scope CurrentUser RemoteSigned`.
3. **Run the installer**, ideally from your project's repo root (that project becomes the default when you run
   lazyrunner elsewhere). Any PowerShell works: from Windows PowerShell 5.1 it installs PowerShell 7 with winget if
   needed, then continues there.

   ```powershell
   cd C:\src\shop
   & "$HOME\Documents\PowerShell\Scripts\lazyrunner\install.ps1"
   ```

   It checks for git, the .NET SDK and npm, writes `lazyrunner.config.json` next to `lazyrunner.ps1`, unblocks the
   scripts, and adds the `lazyrunner` and `lr` aliases to your PowerShell profile. Optional switches: `-AutoStart off`,
   `-AutoRestart on`, `-StopFrontend off`, `-Frontend off`, `-Notifications off`, `-StartupChecklist off`,
   `-AutoUpdate off`, `-Theme dracula`.
4. **Open a new terminal** and run `lazyrunner` (or `lr`) in your repo.

**Updating:** lazyrunner updates itself. When it opens, its startup checklist (or a quiet check when the checklist is
off) looks at the release branch, installs a higher `VERSION`, reopens on it and shows what's new; `lazyrunner update`
does the same on demand. A git clone fast-forwards, never over local changes or commits of your own; a folder from a
zip is refreshed from a cached clone, and your `lazyrunner.config.json` and own files (themes) stay. `,` then `v` (or
`"autoUpdate": false`) turns it off; `lazyrunner version` shows the version and where updates come from.

**Requirements:** Windows, PowerShell 7+, git; the .NET SDK for .NET apps and Node.js / npm for npm apps (nvm-windows
works). Any terminal works; Windows Terminal is best (tab colours and progress).

## Set up a project

lazyrunner needs a profile that says what your project runs. In your repo:

```powershell
lazyrunner profile init          # prints a first profile from what it finds: .NET projects with a launch profile, npm dev scripts
lazyrunner profile init -Save    # writes it to .lazyrunner.json
lazyrunner profile check         # checks it against the repo: projects, launch profiles, scripts, ports
```

A small one looks like this:

```json
{
  "schemaVersion": 1,
  "id": "shop",
  "name": "Shop",
  "watch": ["services"],
  "apps": [
    { "key": "api", "name": "API", "type": "dotnet", "project": "services/Api/Api.csproj", "launchProfile": "Api", "swagger": "/swagger" },
    { "key": "web", "name": "Web", "type": "npm", "dir": "web", "variants": [{ "script": "dev", "port": 5173 }] }
  ]
}
```

Commit `.lazyrunner.json` to share it with your team, or keep it in `%APPDATA%\lazyrunner\projects\` for yourself.
The first time lazyrunner meets a repo's profile (and whenever it changes) it shows what it would run and asks you to
trust it. [projects/README.md](projects/README.md) is the full guide: where lazyrunner looks, every field, recipes
(watch builds served by an API, per-branch variants, a team's changes to a profile), and how to write a profile for
any repo - written so an LLM can follow it too.

## Commands

```
lazyrunner            open lazyrunner in this terminal and start everything (unless auto-start is off)
lazyrunner start      the same, but always starts; if lazyrunner is open in another terminal, starts what is stopped there
lazyrunner stop       stop everything in the open lazyrunner (also cancels a running build)
lazyrunner restart    stop, rebuild if anything changed, run again
lazyrunner profile …  init / check a project profile (above)
lazyrunner update     update lazyrunner now (asks for a git login if needed)
lazyrunner version    the installed version, where updates come from, and what's new in it
lazyrunner help
```

`lazyrunner start` in another project's checkout while lazyrunner is open makes it reopen there, with that project's
apps and settings.

## Keys

The keys follow vim and lazygit. Letters are case-sensitive: **lowercase acts on the focused column, uppercase on
every app**. `?` shows them all, and `:` (or `Ctrl+P`) finds any command by name.

**Run**

| Key | Action |
|-----|--------|
| `space` | Start or stop the focused app, whichever it isn't |
| `u` / `U` | Start the focused app / start everything (builds first if the code changed) |
| `s` / `S` | Stop the focused app (or cancel its build or `npm ci`) / stop everything, cancelling a running build |
| `r` / `R` | Restart the focused app / everything (rebuild if anything changed) |
| `b` / `B` | Rebuild the focused app / everything: restart with a full build, never skipped |
| `!` | Kill whatever holds a port the apps need (offered when a port is taken) |
| `q` / `Ctrl+C` | Quit: stop everything, back to the prompt |

**Tools**

| Key | Action |
|-----|--------|
| `d` | **Database picker**: switch the active connection string in the .NET apps' user secrets (below) |
| `i` | `npm ci` for an npm app, in the background (it stops the app first and starts it again after) |
| `w` / `W` | Open the focused app in the browser (Swagger for an API) / open everything |
| `o` | Open the focused app's full log file (or the failed build's log) |
| `x` | Menu of everything you can do with the focused app |
| `,` | **Settings** menu (below) |

**Moving around and view**

| Key | Action |
|-----|--------|
| `h` / `l`, `Tab` / `Shift+Tab`, `←` `→` | Focus the previous / next column |
| `1`-`9` | Jump to a column |
| `j` / `k`, `↓` `↑` | Scroll the focused column one line |
| `Ctrl+d` / `Ctrl+u`, `Ctrl+f` / `Ctrl+b`, `PgDn` `PgUp` | Scroll half a page / a page |
| `gg` / `G` (`Home` / `End`) | Oldest output / follow new output again |
| `/`, `n` / `N`, `Esc` | Search the focused column (matches highlighted), older / newer match, clear |
| `z` | Zoom: only the focused column |
| `>` / `<` / `=` | Widen / narrow the focused column / make all equal (saved) |
| `Ctrl+L` | Clear all columns and the panel log |
| `t` | Next theme (saved); see [Themes](#themes) |
| `?` | Key reference |
| `:` / `Ctrl+P` | Command palette: fuzzy-find any action or setting |

**Settings (`,` then a letter)**, saved in `lazyrunner.config.json` and always shown in the panel:

| Key | Setting |
|-----|---------|
| `a` | Auto-restart when the git branch changes |
| `u` | Auto-start when lazyrunner opens |
| `s` | Stop/Restart all include the npm apps (off: `S` / `R` / `B` leave them running) |
| `e` | Restart the .NET apps when code changes: once saving in the watched folder has been quiet for 2 s, rebuild and restart the running ones (npm apps have their own watchers) |
| `w` | Startup checklist when lazyrunner opens |
| `r` | Run the startup checklist now |
| `p` | Reload the project profile (reopens lazyrunner) |
| `v` | Auto-update lazyrunner when it opens |
| `c` | What's new: the changelog |

### During a build

.NET builds run one at a time; `R`, `B`, `r`, `b`, `U` and `!` wait for the current one (`S` cancels it). Everything
else keeps working: the database picker, `npm ci` next to the build, starting an npm app, `s` to cancel just the
focused app's build or take it out of the queue, and `q`.

## What it shows

- **Each column's title:** a spinner and progress bar while building or starting, with an estimate from previous runs
  (`23s / ~41s`); a pulsing dot until the app is up; then its URL, response time (`12 ms`, `slow 1.4s`, `not
  responding`) and how long it took to start. The response time comes from requesting the app's Swagger page (`/`
  otherwise) every 5 s.
- **Each column's info line:** CPU and memory of the app's whole process tree with sparklines, requests per second for
  .NET apps, and error / warning counters.
- **Builds** show real progress: projects done out of those in the dependency graph (`23/41`), warnings counted
  instead of flooding the column.
- **Log lines** are coloured by level, and every app's full output is saved to
  `%LOCALAPPDATA%\lazyrunner\<project>\logs\<app>.log`, so `o` lets you search all of it.
- **The terminal tab** title and progress show the overall state; **Windows notifications** cover a failed build, a
  crash, and a slow start finishing.
- **After "Ready"**, the log shows a startup timeline per app.

## Startup checklist

When lazyrunner opens, a short splash checks git, the .NET SDK and Node.js / npm, whether the apps' ports are free,
whether the active database's server answers, each npm app's packages against its `package-lock.json`, and which
project and branch it runs. Problems also go to the panel log. Turn it off with `,` then `w`.

## Database picker (`d`)

.NET apps that use user secrets read their connection strings from `%APPDATA%\Microsoft\UserSecrets\<id>\secrets.json`.
lazyrunner lists every SQL connection string there as `database @ server`, never showing passwords. Choosing one makes
that line active and comments out (`//`) the others with the same key, checks the file is still valid JSON, keeps
the previous version as `secrets.json.lazyrunner-backup`, and restarts the running .NET apps (no build needed). `n`
adds an entry: a copy of the selected one with another database name.

## npm apps

Which script an npm app runs is decided on every start from its `package.json` (the profile's variants), so a branch
can use a dev server and another a watch build. npm apps start right away and compile while the .NET apps build.
Automatic restarts leave them alone, since `npm run` watches its own files - unless a branch needs another variant.
`i` runs `npm ci` in the background; once you've run it from lazyrunner, it warns when `package-lock.json` changes.

## Themes

`t` cycles through the themes in `themes\`: `default` (your terminal's own palette), `dark`, `light`,
`high-contrast`, `dracula`, `nord`, `gruvbox`, `catppuccin-mocha`, `tokyo-night` and `solarized-light`. Some also
recolour the terminal background while lazyrunner runs. Your own: copy a file to `themes\<id>.json` and change it; it
shows up on the next `t`. [themes/README.md](themes/README.md) describes every field. `NO_COLOR` turns colour off.

## Configuration

`lazyrunner.config.json` sits next to `lazyrunner.ps1`; the installer creates it and git ignores it. The panel saves
the `,` settings, the theme and column widths here. Settings that depend on the project live in its section:

```json
{
  "repoRoot": "C:\\src\\shop",
  "autoStartOnOpen": true,
  "theme": "default",
  "projects": {
    "shop": { "columnWidths": { "api": 40, "web": 60 }, "apps": { "admin": { "enabled": false } } }
  }
}
```

| Key | Default | Meaning |
|-----|---------|---------|
| `repoRoot` | | The project used when you run lazyrunner outside any project (set by the installer) |
| `autoStartOnOpen` | `true` | Build and start everything as soon as lazyrunner opens |
| `autoRestartOnBranchChange` | `false` | Restart when the git branch changes |
| `skipUnchangedBuilds` | `true` | Skip `dotnet build` when nothing in the watched folder changed since the last build |
| `restartOnCodeChange` | `false` | Rebuild + restart the running .NET apps when code changes |
| `notifications` | `true` | Windows notifications |
| `startupChecklist` | `true` | The startup checklist |
| `panelLogLines` | `5` | How many recent events the panel shows |
| `theme` | `default` | A file name from `themes\` without `.json` |
| `autoUpdate` | `true` | Install a newer lazyrunner when it opens |
| `updateRepoUrl` / `updateBranch` | | Where updates come from on this machine, instead of `$UpdateRepo` in `lazyrunner.ps1` |
| `projects.<id>.columnWidths` | equal | Relative width per app key (`>` / `<` / `=`) |
| `projects.<id>.apps.<key>.enabled` | `true` | `false` leaves that app out on this machine |
| `projects.<id>.apps.<key>.includeInAll` | | `false` keeps that app running on Stop all / Restart all |
| `projects.<id>.frontend` | `true` | `false` leaves out every npm app |
| `projects.<id>.stopFrontendOnStop` | `true` | Stop/Restart all include the npm apps (`,` then `s`) |

Runtime files live in `%LOCALAPPDATA%\lazyrunner`: `lazyrunner.log`, the trusted repo profiles, and a folder per
project with its build logs and stamps, per-app logs and timings.

## Troubleshooting

- **Build failed:** the column shows the errors and `o` opens the full log.
- **A build was skipped but you want a clean one:** `b` (focused app) or `B` (everything).
- **Port already in use:** something else runs the app (an IDE, say). The panel names the process; `!` kills it and
  starts.
- **An app is quiet:** some apps log to other sinks than the console; the `● … ms` in the title shows it's up and
  answering.
- **The terminal is too small:** lazyrunner needs at least 60×15; `z` zooms one column to full width.
- **The profile is wrong:** `lazyrunner profile check` names each problem and its field.

## Development

lazyrunner is plain PowerShell 7: `lazyrunner.ps1` and the libraries in `lib\` (`lib\apptypes.ps1` is what a .NET or
an npm app is). The tests are plain scripts too; [tests/README.md](tests/README.md) lists them.

### Releasing

Installs update from `$UpdateRepo` in `lazyrunner.ps1` (the repo URL and its release branch; empty means no updates).
A release is a higher `VERSION` on that branch - pushing without bumping it updates nobody:

1. Add a section on top of [CHANGELOG.md](CHANGELOG.md): `## [0.2.0] - 2026-10-20` and what changed, under
   `### Added` / `### Changed` / `### Fixed`. Installs show it after they update.
2. Bump `VERSION` (semantic versioning: a new feature 0.2.0, a fix 0.1.1).
3. Run the tests, commit, and push the branch. Every install picks it up the next time it opens.
