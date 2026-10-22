# Changelog

What changed in each lazyrunner version, newest first. lazyrunner shows the new entries in its startup checklist after
it updates itself; `,` then `c` shows the whole file. How to release a version: README, "Releasing".

## [1.1.0] - 2026-10-22

### Changed
- Nerd Font icons are on by default (`, i` turns them off), and with them every app's title bar has one: the .NET logo, or for an npm app its framework's (Angular, React, Vue, by its package.json) else Node.js's - an app's own `"icon"` is now its plain one

### Added
- `"nerdIcon"` in a profile's app: its title-bar icon with Nerd Font icons on (`"\uf0ac"` a globe, `"\uf085"` cogs)
- Nerd Font marks in the tests view (passed, failed, errors, skipped, queued, cancelled), the migrations view, the startup checklist, a column's error count and the reload window

## [1.0.0] - 2026-10-22

lazyrunner 1.0: everything the PowerShell lazybms did, in Rust, for any project a profile describes - builds, runs and watches its .NET and npm apps, with tests, requests, migrations, durations, lazygit and the database picker beside them.

### Added
- Nerd Font icons (`, i`, `"nerdIcons"`): .NET and Node.js for apps without an icon of their own, a git branch and a database on the status line, a flask, an exchange and a database for the tests, requests and migrations views, and pause and chart marks in a column's numbers - for a terminal whose font is a Nerd Font (a "Mono" one). Off by default

## [0.20.0] - 2026-10-22

### Added
- Reload window (`Ctrl+R`), like VS Code's: reads the settings and themes again, sets the terminal up again (screen, mouse, tab, colours) and repaints, while the apps keep running. It plays a little CRT show - the screen powers off into a glowing line and a sparkle, the logo shimmers in rainbow colours over twinkling stars with a progress bar, then the fresh UI powers on from the middle - and with reduced motion just reloads

## [0.19.0] - 2026-10-22

### Added
- Migrations (`M`): each DbContext of the .NET apps with `"migrations"` in the profile, and its EF Core migrations - applied, pending, the current one - on the database the app uses. `u` applies what's pending (`U` everywhere), `m` moves a context to the chosen migration, forward or back; each asks first, naming the database. dotnet ef runs with the app's project as the startup project, its Development settings and its last build, one job at a time, waiting while an app builds; without `contexts`, `dotnet ef dbcontext list` finds them

### Fixed
- Self-update takes the highest released version, not the one GitHub marks "Latest": that is only the release published last, and when several tags were pushed at once their builds could finish out of order (0.15.0 became "Latest" after 0.18.0 was out). The version comes from the release feed, with GitHub's "Latest" as the fallback

## [0.18.0] - 2026-10-22

### Added
- `y` asks what to copy from the focused app's output, with how much there is of each: `y` the last error with its stack trace (so `y y` is what `y` was), `e` every error, `w` every warning, `a` errors and warnings in order, `s` the search match, `o` all of it. An entry the output repeats (a dev server rebuilding) is copied once
- The tests on the panel's status line: `⚗ ✓438 ✗13 !1` after a run (red when something failed or didn't run, green when all passed), `⚗ testing 6 +10 queued` while they run; a click on it opens the tests view

## [0.17.0] - 2026-10-22

### Added
- npm install next to npm ci: `i` asks which - `c` npm ci (a clean install of exactly what package-lock.json says) or `n` npm install (adds and updates packages to match package.json) - and the `x` menu and the command palette offer both. npm install runs like npm ci: in the background, the dev server stopped meanwhile and started again after, its folder's test suites paused, its time in `D` on its own
- `<` and `>` go to the first and last, as in lazygit: the oldest output and following new output in a column, the first and last item in the tests and requests views, menus, the database picker, durations and the changelog. Widening and narrowing a column moves to `+` and `-`
- In the tests view `1`-`9` jump to the apps' sections, numbered in the list

### Changed
- A box opened from the tests or requests view - `,` settings, `?` the key reference, `d` the database picker… - goes back to the view when it closes, not to the columns
- A test suite that leaves no results says why when its output does ("Test environment jest-environment-jsdom cannot be found") instead of only its exit code

## [0.16.0] - 2026-10-22

### Added
- The tests view groups its suites: Backend and Frontend, and in each the profile's apps with their suites. A .NET test project belongs to the app whose project it references, an npm or Nx suite to the npm app whose folder it runs in; an app's new `"tests"` in the profile names its suites when that guess is wrong. `r` on Backend, Frontend or an app runs all of its suites, and so does a click - the mouse pointer on one says so

### Fixed
- In the tests and requests views the keys they don't use work as they do on the columns again: `,` opens the settings, `?` the key reference, `:` the command palette, `d` the database picker, and so on
- Nx test suites run with Nx 16 and later (lazyrunner looked for Nx's script where older versions kept it, said "run npm ci first" though the packages were there, and didn't stop the Nx daemon before npm ci)
- Only real test files (`*.spec.ts`, `*.test.js`, ...) make an Nx project a suite - not its `tsconfig.spec.json`; projects with a test target but no test files show as one dim line, with nothing to run
- Suites whose packages are missing say "needs packages" and stay as they were, with one line in the log per folder instead of a red "didn't start" for each suite

## [0.15.0] - 2026-10-22

### Added
- Requests (`H`): the HTTP requests the .NET apps served - time, app, method, status, milliseconds and path, newest or slowest first - and, on `Tab`, summed up per endpoint (count, errors, average, 95th percentile, slowest). `e` errors only, `a` one app, `/` a path filter; `y` copies a row, `w` opens a GET request. lazyrunner turns on ASP.NET's request log for the apps it starts (Serilog's request lines count too) and keeps those lines out of the columns

## [0.14.0] - 2026-10-22

### Added
- One test at a time: in the tests view `l` opens the selected suite's tests (failed first) to pick one - its message and stack on the right - and `r` runs just that test; its result joins the suite's. `y` copies it, `h` goes back; a click picks a test, a double click runs it

## [0.13.0] - 2026-10-22

### Added
- Tests (`T`): the project's test suites - .NET test projects (`dotnet test`: NUnit, xUnit, MSTest), npm apps whose test script runs jest or vitest, and an Nx workspace's projects with tests. `r` runs the selected suite, `R` all of them, `f` / `F` their failed tests again; up to `"testParallelism"` at once (half the cores by default), in the background while the apps keep running. The .NET suites build together first, in one MSBuild run. The view shows a running suite's output, then its failures with their message and own stack frames (clickable), or its slowest tests; `y` copies the failures, `o` opens the log. The last results are kept across restarts, and each suite's time shows in `D`

## [0.12.0] - 2026-10-22

### Added
- Stacked layout: in a terminal narrower than 120 cells (a Windows Terminal pane, a laptop screen) the apps stack top to bottom, each the full width, sharing the height as columns share the width (`>` and `<` make the focused one taller or shorter). `|` switches between auto, columns side by side and stacked top to bottom, saved as `"layout"`. When not every app fits, the focused one stays on screen

## [0.11.0] - 2026-10-22

### Added
- Clickable links: URLs and places in files in the columns' output (a build error's `Orders.cs(12,5)`, a .NET stack frame's `in …\Checkout.cs:line 42`, a node or jest frame's `src/cart.ts:12:5`) are underlined, and a click opens a URL in the browser or the file at its line in your editor - VS Code, else Rider, else the default app, or what `"editor"` says. A bare file name is looked for in the project, outside build output and packages

## [0.10.0] - 2026-10-22

### Added
- `L`: lazygit without leaving lazyrunner. It takes over this terminal in the project's folder and `q` brings lazyrunner back; the apps keep running and being watched meanwhile, and quitting or an update waits until you're back. With `,` then `g` in Windows Terminal it opens in a pane beside lazyrunner instead. `"gitTool"` picks another tool

## [0.9.3] - 2026-10-22

### Changed
- The license (lazyrunner License 1.1) allows using lazyrunner at work, including for a company - not only personally and for non-profits - and covers earlier versions too. Redistributing it, selling it or offering it as a product or service still needs written permission

## [0.9.2] - 2026-10-21

### Added
- Screenshots in the README: lazyrunner's real screens for a made-up shop project, drawn by `cargo run -p lazyrunner-ui --example screenshots` (run it after visible changes)

### Fixed
- The settings entry in the command palette and the key reference no longer lists only some of the toggles' letters

## [0.9.1] - 2026-10-21

### Changed
- A build's percentage shows in the header and the terminal tab title whether reduced motion is on or not (`building WebAPI 56%`): they have no progress bar. The column title keeps its bar and project count in both modes, so reduced motion only takes the motion away

## [0.9.0] - 2026-10-21

### Added
- Reduced motion (`,` then `f`, off by default; issue #2): nothing blinks, spins, glides or fades - the busy header, spinners, pulsing dots, progress-bar shimmer, the error flash, the checklist's light sweep and sparkles, the blinking text cursor and new output's white flash all hold still, and the screen only redraws when something changes. Builds show how far they are as a percentage instead (`23/41 ( 56%)`, and in the header and the terminal tab title)

## [0.8.0] - 2026-10-21

### Added
- The mouse (on by default; `,` then `m` turns it off so the terminal selects text again): a click focuses a column and a double click on its title zooms it, the wheel scrolls the column under the pointer (the focus stays), a click picks an item in a list (the menus, the command palette, the database and theme pickers) and a double click runs it, a click outside a box closes it, and a setting in the panel toggles on a click. A moving pointer costs nothing: only clicks and the wheel redraw
- Scrollbars: a column with more output than fits shows where its visible lines are, bright while it is scrolled back

## [0.7.2] - 2026-10-21

### Changed
- Groundwork for the next features, with nothing to see in use: the look-and-feel prototype is gone (the hidden `spike` and `emit` commands with it, about 2,300 lines), the two largest source files are split up, the on/off settings are one typed list, and keys, the command line and the control channel now share one set of commands (the control channel also takes them directly - start, stop, restart, build, npm ci, kill a port's holder, a setting - e.g. `{"command": {"restart": "orders"}}`), ready for the mouse and an agent channel
- Builds, failed builds, build only, restarts and port waits are now tested with quick scripts in place of dotnet and npm, and a clock the tests move by hand

## [0.7.1] - 2026-10-21

### Changed
- The startup checklist runs its checks side by side and moves on sooner when everything is fine, so the apps start seconds earlier. When only the update check is left (offline it can take 20 s), the apps start anyway after 2 s and the check goes on in the background; a version it installs takes the running apps over as soon as nothing is in the way
- An update only replaces lazyrunner once the new binary has shown it runs (`--version`): a damaged or blocked download leaves the installed one as it was

### Fixed
- Restarting no longer freezes the screen while the old servers let go of their ports, and `!` no longer pauses before starting
- `!` only kills what still holds the port, never a process that has since taken the old one's PID
- A panic in a background task no longer tears down the screen while lazyrunner carries on; it is reported in the log instead
- If the running apps can't be handed over (for example the disk is full), the next lazyrunner starts them again and says why, instead of leaving them stopped
- A column's log is only moved to `.prev.log` when it has something in it, so opening lazyrunner twice in a row (after an update) keeps the last real run's log; one that can't be moved is added to rather than wiped. Column logs start over past 32 MB, and `lazyrunner.log` past 8 MB
- A cancelled or failed `npm ci` no longer counts as installed, and a port snapshot that never comes back no longer stops readiness for good
- Output no longer redraws the screen more often than 30 times a second

## [0.7.0] - 2026-10-20

### Changed
- `b` / `B` build the focused app / every app that builds (a full build, never skipped) and leave them **stopped**, instead of rebuilding and restarting them. A running app is stopped first, since it locks its own build output. `u` / `U` then start them without building again. For the old one-key rebuild and restart, press `b` then `u` (or `r` / `R`, which rebuild what changed). npm apps have no separate build step

### Fixed
- The bars in `D` no longer run into the next row's: the tallest is ▇ instead of a full block, so each row stands on its own

## [0.6.0] - 2026-10-20

### Added
- Durations (`D`): every build, app start and `npm ci` is timed (the last 40 of each, in `history.json` in the project's state folder). `D` shows them grouped, with a bar per run, the last run, the usual one (median of the earlier successful runs) and whether the last was notably slower or faster. A run that finishes 15% or more off its usual says so in its log line

### Fixed
- The key reference (`?`) paired `d` with any uppercase `D` as if it were "all"; only keys for the focused app pair up with their uppercase twin

## [0.5.0] - 2026-10-20

### Added
- The running apps carry on when lazyrunner reopens on the same project (Windows): after an update and with `,` then `p` (reload the profile), the new lazyrunner takes them over - same processes, same ports, their output and state - instead of stopping and starting them. Only an app whose entry in the profile changed starts again. While a build or `npm ci` runs, lazyrunner waits for it

### Changed
- An update installed while apps run switches to the new version on its own as soon as nothing is building, a menu is open or a search is being typed (it used to wait for `,` then `p`); the apps keep running

## [0.4.0] - 2026-10-20

### Changed
- `t` opens a theme picker instead of switching to the next theme: a narrow panel on the right, so the columns show each theme as you move through the list (the terminal's background too). Typing filters by name, Enter keeps and saves the theme, Esc puts back the one you had. Each theme has a swatch of its own colours, and the saved one is marked

## [0.3.0] - 2026-10-20

### Added
- `y` copies the focused app's last error with its stack trace, ready to paste into a ticket or a chat; after a failed build, its errors with their full paths. With a search match (`/`), it copies that line and the indented lines that belong to it
- `y` in the database picker copies the highlighted line as it is in secrets.json, password included

## [0.2.0] - 2026-10-20

lazyrunner is now one native binary for Windows, Linux and macOS, with the same profiles, keys and screen.

### Added
- Linux and macOS: apps run in process groups, and nothing lazyrunner started outlives it, even when it crashes
- Install with one command in PowerShell (`irm …lazyrunner-installer.ps1 | iex`); on Linux and macOS, `cargo install` builds it
- Smoother and lighter: 60 frames a second under heavy output at a fraction of the CPU and memory, and nothing redrawn while nothing changes
- The startup checklist shows the update's download progress; any key skips the checks still running

### Changed
- Updates come from GitHub Releases: the binary for your platform, checked against its SHA-256, replaces the running one, and lazyrunner reopens on it
- Settings and your profiles live in the OS's settings folder (`%APPDATA%\lazyrunner`, `~/.config/lazyrunner`, `~/Library/Application Support/lazyrunner`), with `themes` for your own themes; the built-in themes and the changelog are part of the binary
- `updateRepoUrl` takes a GitHub repo; `updateBranch` is gone

## [0.1.0] - 2026-10-15

The first release.

### Added
- Runs a project's .NET APIs and npm frontends side by side in one terminal: builds (one at a time, skipped when nothing changed), ports from launch profiles, readiness, response times, CPU and memory, logs in columns you can scroll, search and zoom
- Project profiles: `.lazyrunner.json` in a repo (trusted once, asked again when it changes), your own in `%APPDATA%\lazyrunner\projects`, or bundled; `extends` for a team's changes; settings and state per project
- npm apps with variants: the first whose script `package.json` has runs, so branches can differ (a dev server, or a watch build a .NET app serves)
- `lazyrunner profile init` proposes a profile for a repo; `lazyrunner profile check` checks one against the repo; `projects/README.md` is a guide for people and LLMs
- Database picker for the .NET apps' user secrets, `npm ci` in the background, startup checklist, themes, command palette, key reference
- Self-update: a higher `VERSION` on the release branch is installed when lazyrunner opens, then it reopens and shows what's new; `lazyrunner update`, `lazyrunner version`, `,` then `v` turns it off, `,` then `c` shows this changelog
