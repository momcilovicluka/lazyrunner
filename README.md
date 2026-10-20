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
- Every app runs as a process tree lazyrunner owns (a job object on Windows, a process group on Linux and macOS): `q`,
  or even closing the terminal, leaves nothing running.
- What a project runs is described in a small JSON **profile** - in the repo (`.lazyrunner.json`) or in your own
  folder. `lazyrunner profile init` writes a first one for you.

One binary, nothing else to install. Releases are for Windows for now; Linux and macOS build from source.

## Install

In PowerShell:

```powershell
powershell -ExecutionPolicy Bypass -c "irm https://github.com/momcilovicluka/lazyrunner/releases/latest/download/lazyrunner-installer.ps1 | iex"
```

It puts `lazyrunner.exe` in `%USERPROFILE%\.local\bin` and adds that to your PATH. On Linux or macOS, build it from
source (with [Rust](https://rustup.rs)): `cargo install --locked --git https://github.com/momcilovicluka/lazyrunner lazyrunner`.

Then open a new terminal and run `lazyrunner` in your repo. A shorter name: add an alias of your own
(`Set-Alias lr lazyrunner` in your PowerShell profile, `alias lr=lazyrunner` in your shell's).

**Updating:** lazyrunner updates itself. When it opens, its startup checklist (or a quiet check when the checklist is
off, in the background) asks the GitHub releases for the latest version, installs a higher one, reopens on it and shows
what's new; `lazyrunner update` does the same on demand. Downloads come over HTTPS only, and the release's SHA-256
catches a broken download. `,` then `v` (or `"autoUpdate": false`) turns it
off; `lazyrunner version` shows the version and where updates come from.

**Requirements:** git; the .NET SDK for .NET apps and Node.js / npm for npm apps (nvm works). Any terminal works;
Windows Terminal is best (tab colours and progress).

## Set up a project

lazyrunner needs a profile that says what your project runs. In your repo:

```sh
lazyrunner profile init          # prints a first profile from what it finds: .NET projects with a launch profile, npm dev scripts
lazyrunner profile init --save   # writes it to .lazyrunner.json
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

Commit `.lazyrunner.json` to share it with your team, or keep it in your own profiles folder (`projects` in the
settings folder, below) for yourself. The first time lazyrunner meets a repo's profile (and whenever it changes) it
shows what it would run and asks you to trust it. [projects/README.md](projects/README.md) is the full guide: where
lazyrunner looks, every field, recipes (watch builds served by an API, per-branch variants, a team's changes to a
profile), and how to write a profile for any repo - written so an LLM can follow it too.

## Commands

```
lazyrunner            open lazyrunner in this terminal and start everything (unless auto-start is off)
lazyrunner start      the same, but always starts; if lazyrunner is open in another terminal, starts what is stopped there
lazyrunner stop       stop everything in the open lazyrunner (also cancels a running build)
lazyrunner restart    stop, rebuild if anything changed, run again
lazyrunner profile …  init / check a project profile (above)
lazyrunner update     update lazyrunner now (also when auto-update is off)
lazyrunner version    the installed version, where updates come from, and what's new in it
lazyrunner help
```

`lazyrunner start` in another checkout of the same project while lazyrunner is open switches it there; in another
project's checkout it reopens there, with that project's apps and settings.

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
| `y` | Copy the focused app's last error with its stack trace (a failed build's errors with full paths), or the search match |
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
| `t` | Theme picker: preview each theme live, `Enter` keeps one (saved), `Esc` undoes; see [Themes](#themes) |
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
| `r` | Run the startup checklist now (and check for an update) |
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
- **Log lines** are coloured by level, and every app's full output is saved to `<project>/logs/<app>.log` in the
  state folder (below), so `o` lets you search all of it.
- **The terminal tab** title and progress show the overall state; **desktop notifications** cover a failed build, a
  crash, and a slow start finishing.
- **After "Ready"**, the log shows a startup timeline per app.

## Startup checklist

When lazyrunner opens, a short splash checks for a lazyrunner update, git, the .NET SDK and Node.js / npm, whether the
apps' ports are free, whether the active database's server answers, each npm app's packages against its
`package-lock.json`, and which project and branch it runs. Problems also go to the panel log. Turn it off with `,`
then `w`.

## Database picker (`d`)

.NET apps that use user secrets read their connection strings from `secrets.json` in the user secrets folder
(`%APPDATA%\Microsoft\UserSecrets\<id>\` on Windows, `~/.microsoft/usersecrets/<id>/` elsewhere). lazyrunner lists
every SQL connection string there as `database @ server`, never showing passwords. Choosing one makes that line active
and comments out (`//`) the others with the same key, checks the file is still valid JSON, keeps the previous version
as `secrets.json.lazyrunner-backup`, and restarts the running .NET apps (no build needed). `n` adds an entry: a copy
of the selected one with another database name; `y` copies the highlighted line as it is in the file,
password included.

## npm apps

Which script an npm app runs is decided on every start from its `package.json` (the profile's variants), so a branch
can use a dev server and another a watch build. npm apps start right away and compile while the .NET apps build.
Automatic restarts leave them alone, since `npm run` watches its own files - unless a branch needs another variant.
`i` runs `npm ci` in the background; once you've run it from lazyrunner, it warns when `package-lock.json` changes.

## Themes

`t` opens the theme picker: a narrow panel on the right, so the rest of the screen shows each theme as you move
through the list (`↑` `↓`, `PgUp` `PgDn`), the terminal's background included. Typing filters by name, `Enter` keeps
the selected theme (saved), `Esc` puts back the one you had. Built in: `default` (your terminal's own palette),
`dark`, `light`, `high-contrast`, `dracula`, `nord`, `gruvbox`, `catppuccin-mocha`, `tokyo-night` and
`solarized-light`; some also recolour the terminal background while lazyrunner runs. Your own: copy one from
[themes/](themes/) to `themes/<id>.json` in the settings folder and change it; the picker reads the themes again each
time it opens, so it shows up straight away. [themes/README.md](themes/README.md) describes every field.
`NO_COLOR` turns colour off.

## Configuration

`lazyrunner.config.json` is in the settings folder; lazyrunner writes it when you change a setting. The panel saves the
`,` settings, the theme and column widths there. Settings that depend on the project live in its section:

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
| `repoRoot` | | The project used when you run lazyrunner outside any project |
| `autoStartOnOpen` | `true` | Build and start everything as soon as lazyrunner opens |
| `autoRestartOnBranchChange` | `false` | Restart when the git branch changes |
| `skipUnchangedBuilds` | `true` | Skip `dotnet build` when nothing in the watched folder changed since the last build |
| `restartOnCodeChange` | `false` | Rebuild + restart the running .NET apps when code changes |
| `notifications` | `true` | Desktop notifications |
| `startupChecklist` | `true` | The startup checklist |
| `panelLogLines` | `5` | How many recent events the panel shows |
| `theme` | `default` | A theme's id (a built-in one, or a file in `themes` without `.json`) |
| `autoUpdate` | `true` | Install a newer lazyrunner when it opens |
| `updateRepoUrl` | | Where updates come from on this machine (a GitHub repo URL; `none`: no updates) |
| `projects.<id>.columnWidths` | equal | Relative width per app key (`>` / `<` / `=`) |
| `projects.<id>.apps.<key>.enabled` | `true` | `false` leaves that app out on this machine |
| `projects.<id>.apps.<key>.includeInAll` | | `false` keeps that app running on Stop all / Restart all |
| `projects.<id>.frontend` | `true` | `false` leaves out every npm app |
| `projects.<id>.stopFrontendOnStop` | `true` | Stop/Restart all include the npm apps (`,` then `s`) |

**Where things live:**

| | Settings (`lazyrunner.config.json`, `projects/`, `themes/`) | State (`lazyrunner.log`, trusted profiles, per-project logs and timings) |
|---|---|---|
| Windows | `%APPDATA%\lazyrunner` | `%LOCALAPPDATA%\lazyrunner` |
| macOS | `~/Library/Application Support/lazyrunner` | `~/Library/Application Support/lazyrunner/state` |
| Linux | `~/.config/lazyrunner` | `~/.local/state/lazyrunner` |

`LAZYRUNNER_CONFIG_PATH`, `LAZYRUNNER_PROFILES_DIR` and `LAZYRUNNER_STATE_DIR` replace them.

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

lazyrunner is Rust ([Ratatui](https://ratatui.rs)), in `crates/`: `lazyrunner-core` (profiles, settings, apps,
processes, output, the self-update), `lazyrunner-ui` (the terminal UI) and `lazyrunner` (the binary).
[docs/parity.md](docs/parity.md) lists everything it does against the PowerShell version (0.1), which it replaces.

```
cargo run --release                                   # run the project in this folder
cargo run -- profile check                            # check the profile that applies here
cargo run --release -- spike --bench 20               # the look-and-feel spike: 20 s of 5 apps x 2000 lines/s, then fps, CPU, memory
cargo test --workspace                                # unit tests, snapshots of every screen, end-to-end tests
cargo insta review                                    # look at changed screen snapshots
LAZYRUNNER_WRITE_REFERENCE=1 cargo test -p lazyrunner-core reference   # rewrite the guide's field reference
dist plan                                             # what a release would build (cargo-dist)
```

The end-to-end tests run the sample repo with real dotnet, npm and node (skipped without them), and update an installed
copy of lazyrunner from a local stand-in for GitHub Releases.

### Releasing

A release is a tag: pushing `v1.2.3` builds the Windows binary and publishes the GitHub release with it, its
.sha256 and the PowerShell installer (`.github/workflows/release.yml`, made by cargo-dist from `dist-workspace.toml`;
after changing that file, `dist generate`). Installs pick it up the next time they open.

1. Add a section on top of [CHANGELOG.md](CHANGELOG.md): `## [0.2.1] - 2026-11-02` and what changed, under
   `### Added` / `### Changed` / `### Fixed`. Installs show it after they update.
2. Bump `version` under `[workspace.package]` in `Cargo.toml` (semantic versioning: a new feature 0.3.0, a fix 0.2.1).
3. Run the tests and `dist plan`, commit, then tag and push:

   ```sh
   git tag v0.2.1
   git push origin main v0.2.1
   ```

The repo has to be public: the installer and the self-update download its releases without signing in.

## License

lazyrunner is source-available, not open source: free for personal use and for non-profit organisations, under the
[lazyrunner License](LICENSE). Any use at work or other commercial use needs written permission from the author; ask
through this repository. Copying, redistributing, or maintaining a separate fork is not allowed.

Pull requests are welcome. Forking is allowed only to prepare them, and contributions come in under the Contributor
Agreement in [CONTRIBUTING.md](CONTRIBUTING.md).
