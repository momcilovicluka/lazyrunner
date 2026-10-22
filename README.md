# lazyrunner

[![CI](https://github.com/momcilovicluka/lazyrunner/actions/workflows/rust.yml/badge.svg)](https://github.com/momcilovicluka/lazyrunner/actions/workflows/rust.yml)
[![Latest release](https://img.shields.io/github/v/release/momcilovicluka/lazyrunner)](https://github.com/momcilovicluka/lazyrunner/releases/latest)

**Run your entire local development stack from one terminal.**

lazyrunner is a Rust terminal UI for running .NET APIs and npm frontends side by side. It builds what changed, starts
everything on the right ports, tells you when each app is up, and keeps live logs in columns you can scroll, search
and zoom.

Inspired by lazygit: one command instead of a row of terminal tabs.

> [!IMPORTANT]
> lazyrunner is source-available, not open source. It is free to use - personally, for a non-profit, or at work -
> under the [lazyrunner License](LICENSE). Redistributing it or selling it needs written permission.

## Features

- **One terminal for your stack** — run backend and frontend apps side by side.
- **Smart builds** — .NET builds run one at a time and skip unchanged work.
- **Live logs** — follow, search, and zoom each app's output.
- **Process management** — start/stop/restart apps while cleaning up child processes.
- **Project profiles** — keep stack setup in a shareable `.lazyrunner.json`.
- **Keyboard-first workflow** — fast controls inspired by lazygit.

## Quick start

Install lazyrunner, open a repository with your apps, then:

```sh
lazyrunner profile init --save
lazyrunner
```

Review the generated profile before the first run. See [Set up a project](#set-up-a-project) for profile details.

Windows binaries are published on the [releases page](https://github.com/momcilovicluka/lazyrunner/releases). Linux
and macOS currently build from source.

## Preview

![lazyrunner running a shop project: two .NET APIs and three npm apps in columns, each with its port, response time,
CPU and memory; below them the panel with the branch, the database, the keys, the settings and the log](docs/screenshots/overview.svg)

<table>
  <tr>
    <td><img src="docs/screenshots/palette.svg" alt="The command palette, filtered by typing 'start'"></td>
    <td><img src="docs/screenshots/durations.svg" alt="The durations box: builds, starts and npm ci over their last runs, with the usual time and the trend"></td>
  </tr>
  <tr>
    <td align="center"><code>:</code> the command palette</td>
    <td align="center"><code>D</code> how long builds and starts took</td>
  </tr>
  <tr>
    <td><img src="docs/screenshots/themes.svg" alt="The theme picker beside the columns, previewing Catppuccin Mocha"></td>
    <td><img src="docs/screenshots/stacked.svg" alt="The same five apps stacked top to bottom in a narrow terminal, the focused one taking the spare rows"></td>
  </tr>
  <tr>
    <td align="center"><code>t</code> themes, previewed live</td>
    <td align="center"><code>|</code> a narrow terminal stacks the apps</td>
  </tr>
</table>

- **.NET apps** are built with `dotnet build` (one at a time, skipped when nothing changed) and run with their launch
  profile; the port comes from `launchSettings.json`.
- **npm apps** run their dev server or watch build; which script runs can differ per branch.
- Every app runs as a process tree lazyrunner owns (a job object on Windows, a process group on Linux and macOS): `q`,
  or even closing the terminal, leaves nothing running.
- What a project runs is described in a small JSON **profile** - in the repo (`.lazyrunner.json`) or in your own
  folder. `lazyrunner profile init` writes a first one for you.

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
what's new; `lazyrunner update` does the same on demand. With apps running, the new version takes them over when it
opens (Windows): they keep running, on the same ports, with their output. Downloads come over HTTPS only, the release's SHA-256
catches a broken download, and the new binary has to run before it replaces the old one. `,` then `v` (or `"autoUpdate": false`) turns it
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

## Example stacks

- **.NET API + React/Vite frontend**: one `dotnet` app and one `npm` app variant (`dev`, `port`).
- **Multi-service backend**: multiple `dotnet` apps in one profile, each with its own launch profile and port.
- **Frontend watch build served by API**: an `npm` watch variant with `servedBy` and `ready.file`.

See [projects/README.md](projects/README.md) for full examples and recipes.

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
| `b` / `B` | Build the focused app / every app that builds (but the ones `S` leaves running): a full build, never skipped, that leaves them **stopped** (a running one is stopped first - it locks its build output). `u` / `U` then start them without building again. npm apps have no separate build step |
| `!` | Kill whatever holds a port the apps need (offered when a port is taken) |
| `q` / `Ctrl+C` | Quit: stop everything, back to the prompt |

**Tools**

| Key | Action |
|-----|--------|
| `d` | **Database picker**: switch the active connection string in the .NET apps' user secrets (below) |
| `i` | Install an npm app's packages in the background: `c` `npm ci`, `n` `npm install` (it stops the app first and starts it again after) |
| `w` / `W` | Open the focused app in the browser (Swagger for an API) / open everything |
| `o` | Open the focused app's full log file (or the failed build's log) |
| `y` | Copy from the focused app's output - it asks what: `y` the last error with its stack trace (a failed build's errors with full paths), `e` every error, `w` every warning, `a` both in order (repeats once), `s` the search match, `o` all of it |
| `D` | **Durations**: how long each build, app start, `npm ci` and test suite took over its last runs (below) |
| `M` | **Migrations**: the EF Core migrations, applied and pending, per DbContext; apply or roll back (below) |
| `H` | **Requests**: the HTTP requests the .NET apps served, per endpoint too (below) |
| `T` | **Tests**: the project's test suites - run them in the background, see what failed, run one test (below) |
| `L` | **lazygit** (or your `gitTool`) without leaving lazyrunner (below) |
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
| `+` / `-` / `=` | Widen / narrow the focused column (taller / shorter when stacked) / make all equal (saved) |
| `<` / `>` | The first / last: the oldest output and following new output again in a column, the first and last item in a list or box (as in lazygit) |
| `\|` | Layout: auto (stacked below 120 cells) → columns side by side → stacked top to bottom (saved) |
| `Ctrl+L` | Clear all columns and the panel log |
| `Ctrl+R` | **Reload window**, like VS Code's: reads the settings and themes again, sets the terminal up again (screen, mouse, tab, colours) and repaints - with a little CRT show (none with reduced motion). The apps keep running. Handy after editing a setting or a theme by hand, or when the terminal got garbled |
| `t` | Theme picker: preview each theme live, `Enter` keeps one (saved), `Esc` undoes; see [Themes](#themes) |
| `?` | Key reference |
| `:` / `Ctrl+P` | Command palette: fuzzy-find any action or setting |

**Settings (`,` then a letter)**, saved in `lazyrunner.config.json` and always shown in the panel:

| Key | Setting |
|-----|---------|
| `a` | Auto-restart when the git branch changes |
| `u` | Auto-start when lazyrunner opens |
| `s` | Stop/Restart all include the npm apps (off: `S` / `R` leave them running) |
| `e` | Restart the .NET apps when code changes: once saving in the watched folder has been quiet for 2 s, rebuild and restart the running ones (npm apps have their own watchers) |
| `w` | Startup checklist when lazyrunner opens |
| `r` | Run the startup checklist now (and check for an update) |
| `p` | Reload the project profile (reopens lazyrunner; on Windows the running apps carry on, unless their entry changed) |
| `v` | Auto-update lazyrunner when it opens |
| `m` | The mouse (below); off, the terminal selects text again |
| `f` | Reduced motion: nothing blinks, spins, glides or fades |
| `g` | `L` opens the git tool beside lazyrunner, in a Windows Terminal pane |
| `i` | Nerd Font icons in lazyrunner's chrome (title bars, the status line, the views' headers) |
| `c` | What's new: the changelog |

### Mouse

A click focuses a column; a double click on its title zooms it (again: back). The wheel scrolls the column under the
pointer, whichever has the focus. In the menus, the command palette and the database and theme pickers a click picks
an item and a double click runs it; a click outside a box closes it. A setting in the panel toggles on a click. Moving
the pointer does nothing (and costs nothing). While the mouse is on, the terminal leaves text selection to
lazyrunner, which has none: `,` then `m` turns it off (in Windows Terminal, Shift + drag selects either way).

### During a build

.NET builds run one at a time; `R`, `B`, `r`, `b`, `U` and `!` wait for the current one (`S` cancels it). Everything
else keeps working: the database picker, `npm ci` next to the build, starting an npm app, `s` to cancel just the
focused app's build or take it out of the queue, and `q`.

## What it shows

- **Each column's title:** a spinner and progress bar while building or starting, with an estimate from previous runs
  (`23s / ~41s`); a pulsing dot until the app is up; then its URL, response time (`12 ms`, `slow 1.4s`, `not
  responding`) and how long it took to start. The response time comes from requesting the app's Swagger page (`/`
  otherwise) every 5 s.
- **The layout:** the apps side by side, or top to bottom when the terminal is narrower than 120 cells (a Windows
  Terminal pane, a laptop screen); `|` picks one for good.
- **Each column's info line:** CPU and memory of the app's whole process tree with sparklines, requests per second for
  .NET apps, and error / warning counters.
- **Builds** show real progress: projects done out of those in the dependency graph (`23/41`), warnings counted
  instead of flooding the column.
- **Log lines** are coloured by level, and every app's full output is saved to `<project>/logs/<app>.log` in the
  state folder (below), so `o` lets you search all of it.
- **Links** in output are underlined: URLs, and places in files - a build error's `Orders.cs(12,5)`, a .NET stack
  frame's `in …\Checkout.cs:line 42`, a node or jest frame's `src/cart.ts:12:5`. A click opens a URL in the browser
  and a file at its line in your editor (`"editor"`); a bare file name is looked for in the project, outside build
  output and `node_modules`.
- **A scrollbar** on the right of a column with more output than fits shows where the lines on screen are; it is
  bright while the column is scrolled back (`G` follows new output again).
- **The terminal tab** title and progress show the overall state; **desktop notifications** cover a failed build, a
  crash, and a slow start finishing.
- **After "Ready"**, the log shows a startup timeline per app.

## Startup checklist

When lazyrunner opens, a short splash checks for a lazyrunner update, git, the .NET SDK and Node.js / npm, whether the
apps' ports are free, whether the active database's server answers, each npm app's packages against its
`package-lock.json`, and which project and branch it runs. The checks run side by side. The apps don't wait long for
the update check: once everything else is done it gets 2 s more, then goes on in the background (an update it installs
is switched to when nothing is in the way). Problems also go to the panel log. Turn it off with `,` then `w`.

## Database picker (`d`)

.NET apps that use user secrets read their connection strings from `secrets.json` in the user secrets folder
(`%APPDATA%\Microsoft\UserSecrets\<id>\` on Windows, `~/.microsoft/usersecrets/<id>/` elsewhere). lazyrunner lists
every SQL connection string there as `database @ server`, never showing passwords. Choosing one makes that line active
and comments out (`//`) the others with the same key, checks the file is still valid JSON, keeps the previous version
as `secrets.json.lazyrunner-backup`, and restarts the running .NET apps (no build needed). `n` adds an entry: a copy
of the selected one with another database name; `y` copies the highlighted line as it is in the file,
password included.

## lazygit (`L`)

`L` opens [lazygit](https://github.com/jesseduffield/lazygit) in the project's folder without leaving lazyrunner. It
takes over this terminal; `q` in lazygit brings lazyrunner back. Meanwhile the apps keep running and lazyrunner keeps
watching them (a branch you switch to in lazygit restarts them as usual, if that setting is on), and quitting or an
update waits until you're back. With `,` then `g` in Windows Terminal, `L` opens lazygit in a pane beside lazyrunner
instead, so you see both. Another git tool: `"gitTool"` (for example `"gitui"`). lazygit has to be installed (`winget
install JesseDuffield.lazygit`, `scoop install lazygit`).

## Migrations (`M`)

Give a .NET app `"migrations"` in the profile and `M` shows each of its DbContexts on the left and the selected one's
migrations on the right, newest first: applied (✓), pending (◌) and the current one, as the database the app uses
has them. `dotnet ef` (`dotnet tool install --global dotnet-ef`) runs with the app's project as the startup project,
its Development settings - so the database you picked with `d` - and its last build (`b` rebuilds it); in the
background, one job at a time, waiting while an app builds.

```json
{ "key": "api", "type": "dotnet", "project": "src/Api/Api.csproj", "launchProfile": "Api",
  "migrations": { "project": "src/Data/Data.csproj", "contexts": ["ShopDbContext"] } }
```

`project` is the one with the migrations (the app's own when left out); without `contexts` lazyrunner finds them with
`dotnet ef dbcontext list`. `l` chooses a migration, `m` moves the context to it - forward applies up to it, back
reverts the newer ones through their `Down()` - and `u` applies what's pending (`U`: in every context); each asks
first, naming the database. `r` checks again; switching the database or the branch does too.

## Requests (`H`)

lazyrunner starts the .NET apps with ASP.NET's request log turned on (`--Logging:LogLevel:Microsoft.AspNetCore.
Hosting.Diagnostics=Information`) and keeps those lines out of the columns: `H` lists the requests instead - time,
app, method, status, milliseconds and path, newest first (`s`: slowest first). Serilog's request logging (`HTTP GET /x
responded 200 in 12 ms`) shows too. `Tab` sums them up per endpoint (ids in paths become `{id}`): count, errors,
average, 95th percentile and slowest, sorted by total time (`s` for the others).

`e` shows errors only, `a` one app, `/` filters by path; `y` copies the selected row, `w` (or a double click) opens a
GET request in the browser, `c` clears the list, `Esc` / `q` / `H` close. The top row follows new requests; any other
row stays put as new ones arrive. The last 5,000 are kept while lazyrunner runs (lazyrunner's own response-time probe
isn't among them); the full lines are still in each app's log file.

## Tests (`T`)

`T` finds the project's test suites and lists them on the left - the backend, then the frontend, and in each the
profile's apps with their suites - the selected one on the right:

- **.NET**: every project referencing `Microsoft.NET.Test.Sdk` (NUnit, xUnit and MSTest alike), run with `dotnet
  test`. The queued ones build together first, in one MSBuild run, so the projects they share build once; then they
  run side by side without building again. An app's build pauses that test build, which runs again after it.
- **npm**: an npm app whose `test` script runs jest or vitest; in an Nx workspace, each project with tests (`nx test`).
  `npm ci` pauses the suites that run from its folder.

A .NET test project belongs to the app whose project it references, an npm or Nx suite to the npm app whose folder
it runs in; an app's `"tests"` in the profile (test project paths or names, Nx project names) says otherwise. Choose
Backend, Frontend or an app and `r` (or a click on it - the pointer on one says so) runs all of its suites; `1`-`9`
jump to the apps' sections, numbered in the list. A box opened from the tests or requests view (`,` settings, `?`,
`d`…) goes back to the view when it closes.

`r` runs the selected suite, `R` all of them, `f` / `F` only their failed tests again (the results merge into the last
full run's). Up to `testParallelism` suites run at once (half the cores by default), in the background: close the view
and the apps keep running, the panel says `testing 2 +3 queued`, the log says how each suite did, and a notification
comes when something failed. While a suite runs the right side shows its output; afterwards its failures with their
message and their own stack frames (a place in a file opens in your editor on a click), or its slowest tests when all
passed. `y` copies the failures, `o` opens the suite's whole log, `s` stops the tests, `Esc` / `q` / `T` close.

`l` opens the selected suite's tests (the failed first, then the skipped and the passed) to pick one: `j` `k` choose,
the right side shows its message and stack, `r` runs just that test (its result joins the suite's), `y` copies it,
`h` goes back to the suites. A click picks a test, a double click runs it. The
last results are kept, so they're still there after a restart, and each suite's time shows in `D`. The panel's
status line sums them up - `⚗ ✓438 ✗13` (red when something failed), `⚗ testing 6 +10 queued` while they run - and a
click on it opens `T`.

## Durations (`D`)

Every build, app start, `npm ci` and test suite is timed and kept (the last 40 of each) in `history.json` in the project's state
folder. `D` shows them in a box, grouped:

| Column | Meaning |
|---|---|
| bars | One bar per run, scaled to the item's slowest; red = it failed |
| last | The latest run (red when it failed) |
| usual | Median of the up to 10 successful runs before it (once there are 3) |
| ▲ / ▼ | The latest run 15% (and a second) or more slower (red) / faster (green) than usual; `≈` closer; nothing after a failed run |

When a run finishes notably off its usual, its log line says so, e.g.
`✓ Orders built in 52s (41 projects) · 37% slower than usual (38s)`. A cancelled build or `npm ci` isn't recorded; an
unreadable `history.json` is kept as `history.json.bad` and a new one starts.
`j` `k` scroll, `Esc` / `q` / `D` close.

## npm apps

Which script an npm app runs is decided on every start from its `package.json` (the profile's variants), so a branch
can use a dev server and another a watch build. npm apps start right away and compile while the .NET apps build.
Automatic restarts leave them alone, since `npm run` watches its own files - unless a branch needs another variant.
`i` installs packages in the background and asks how: `c` runs `npm ci` (a clean install of exactly what
`package-lock.json` says), `n` runs `npm install` (adds and updates packages to match `package.json`; the lock file may
change). Both stop the app first and start it again after, and pause the test suites that run from its folder. Once
you've installed from lazyrunner, it warns when `package-lock.json` changes.

## Nerd Font icons

With `, i` (`nerdIcons`) lazyrunner draws Nerd Font icons in its chrome: .NET and Node.js for apps without an icon of
their own, a git branch, a database, a flask for the tests, and so on. They need a terminal font that is a Nerd Font,
a "Mono" one so each icon is one cell wide - [CaskaydiaCove Nerd Font Mono](https://www.nerdfonts.com/font-downloads)
goes well with Windows Terminal. Boxes instead of icons mean the font isn't one; `, i` again turns them off. An icon an
app's profile gives it and the text in the columns stay as they are.

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
| `mouse` | `true` | Clicks and the wheel (off: the terminal selects text) |
| `reducedMotion` | `false` | No blinking, spinners, glides or fades |
| `gitTool` | `"lazygit"` | What `L` runs: a command (with arguments) on PATH or a path |
| `gitBeside` | `false` | `L` opens it beside lazyrunner (in Windows Terminal) instead of in place |
| `nerdIcons` | `false` | Nerd Font icons in the chrome; for a terminal whose font is a Nerd Font (a "Mono" one) |
| `layout` | `"auto"` | `columns` side by side, `stacked` top to bottom, or `auto`: stacked when the terminal is narrower than 120 cells (`\|`) |
| `testParallelism` | half the cores | How many test suites `T` runs at once |
| `editor` | `"auto"` | Opens a clicked file link: `auto` (VS Code, else Rider, else the default app), `code`, `rider`, `default`, or a command with `{file}`, `{line}`, `{column}` |
| `updateRepoUrl` | | Where updates come from on this machine (a GitHub repo URL; `none`: no updates) |
| `projects.<id>.columnWidths` | equal | Relative width per app key, or height when stacked (`>` / `<` / `=`) |
| `projects.<id>.apps.<key>.enabled` | `true` | `false` leaves that app out on this machine |
| `projects.<id>.apps.<key>.includeInAll` | | `false` keeps that app running on Stop all / Restart all |
| `projects.<id>.frontend` | `true` | `false` leaves out every npm app |
| `projects.<id>.stopFrontendOnStop` | `true` | Stop/Restart all include the npm apps (`,` then `s`) |

**Where things live:**

| | Settings (`lazyrunner.config.json`, `projects/`, `themes/`) | State (`lazyrunner.log`, trusted profiles, per-project logs, timings and durations) |
|---|---|---|
| Windows | `%APPDATA%\lazyrunner` | `%LOCALAPPDATA%\lazyrunner` |
| macOS | `~/Library/Application Support/lazyrunner` | `~/Library/Application Support/lazyrunner/state` |
| Linux | `~/.config/lazyrunner` | `~/.local/state/lazyrunner` |

`LAZYRUNNER_CONFIG_PATH`, `LAZYRUNNER_PROFILES_DIR` and `LAZYRUNNER_STATE_DIR` replace them.

## Troubleshooting

- **Build failed:** the column shows the errors and `o` opens the full log.
- **A build was skipped but you want a clean one:** `b` (focused app) or `B` (everything) builds in full; `u` / `U`
  then start without building again.
- **Port already in use:** something else runs the app (an IDE, say). The panel names the process; `!` kills it and
  starts.
- **An app is quiet:** some apps log to other sinks than the console; the `● … ms` in the title shows it's up and
  answering.
- **The terminal is too small:** lazyrunner needs at least 60×15; `z` zooms one column to full width, and `|` stacks
  the apps top to bottom.
- **The profile is wrong:** `lazyrunner profile check` names each problem and its field.

## Roadmap

- Publish Linux and macOS release binaries.
- Expand profile recipes for common multi-service setups.
- Keep improving startup diagnostics and recovery feedback in the UI.

## Development

lazyrunner is Rust ([Ratatui](https://ratatui.rs)), in `crates/`: `lazyrunner-core` (profiles, settings, apps,
processes, output, the self-update), `lazyrunner-ui` (the terminal UI) and `lazyrunner` (the binary).
[docs/parity.md](docs/parity.md) lists everything it does against the PowerShell version (0.1), which it replaces.

```
cargo run --release                                   # run the project in this folder
cargo run -- profile check                            # check the profile that applies here
cargo test --workspace                                # unit tests, snapshots of every screen, end-to-end tests
cargo insta review                                    # look at changed screen snapshots
LAZYRUNNER_WRITE_REFERENCE=1 cargo test -p lazyrunner-core reference   # rewrite the guide's field reference
cargo run -p lazyrunner-ui --example screenshots      # redraw the screenshots above (docs/screenshots) after visible changes
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

lazyrunner is source-available, not open source: free to use as a tool for any purpose - personally, for a
non-profit, or at work, including for a company - under the [lazyrunner License](LICENSE). Copying, redistributing,
selling it or offering it as a product or service, and maintaining a separate fork are not allowed without written
permission from the author; ask through this repository.

Pull requests are welcome. Forking is allowed only to prepare them, and contributions come in under the Contributor
Agreement in [CONTRIBUTING.md](CONTRIBUTING.md).
