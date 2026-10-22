# Changelog

What changed in each lazyrunner version, newest first. lazyrunner shows the new entries in its startup checklist after
it updates itself; `,` then `c` shows the whole file. How to release a version: README, "Releasing".

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
