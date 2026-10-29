# Changelog

What changed in each lazyrunner version, newest first. lazyrunner shows the new entries in its startup checklist after
it updates itself; `,` then `c` shows the whole file. How to release a version: README, "Releasing".

## [1.30.0] - 2026-10-29

### Added
- Notes on marks: `a` in the marks box (`'`) writes one for the selected mark, shown in the box and in the log when `[` / `]` jump to it; `y` there copies every mark, oldest first, as a timeline (time, app, note, line) for a ticket or a chat. The box lists the marks oldest first across the apps; notes go with their marks (#58)

## [1.29.0] - 2026-10-29

### Added
- Empty places say what fills them: a stopped app's empty column its keys (`u` starts it, `b` builds it, `U` starts all), a column whose filter or search hides every line what hides them and how to show them again, and the marks box (`'`) opens with no marks to say how to make one (#60)

## [1.28.0] - 2026-10-29

### Added
- More Nerd Font icons: a column's state (up, watching, stopped, exited) and its warnings, level filter and search chips; the header's "all up" and "stopped"; an icon in every box's title; one for each startup check; sun and moon on the theme picker's groups and a pin on the project's own theme; one for each way to copy a request; the durations' trend arrows. Plain symbols (`, i`) and the terminal tab's title stay as they were

## [1.27.0] - 2026-10-29

### Added
- The command palette remembers what you ran: with nothing typed, the last five come first under Recent, and of two equally good matches the one run lately wins. Kept per project (#59)

## [1.26.0] - 2026-10-29

### Added
- `Alt+j` / `Alt+k` move the selection in the command palette and the theme picker, where a plain `j` or `k` types; `Ctrl+J` / `Ctrl+K` too, in terminals that send them as such (#47)

## [1.25.1] - 2026-10-28

### Fixed
- `r` in the startup checklist stays the checklist's: pressed again while a retry runs it does nothing, and with nothing to retry it only closes the checklist - it no longer goes on to restart the focused app (#46)

## [1.25.0] - 2026-10-28

### Added
- Marks: `m` marks the line at the bottom of the focused column (or the search match it is on; again unmarks), `[` / `]` jump to the older / newer mark, and `'` lists every column's marks - Enter jumps to one, `d` unmarks it. Marked lines show a `▌` in the margin; an app's marks go when it starts again (#30)

## [1.24.0] - 2026-10-28

### Added
- Search filters, and takes a regex: while typing, `Tab` shows only the matching lines (a dimmed line either side as context; the column's top row says what it hides), `Alt+r` takes the query as a regular expression (one that doesn't compile is searched as plain text, and the prompt says why), `Alt+c` minds case. The prompt counts the matches as you type, and `/` again brings the column's last search back (#29)

### Fixed
- `n` / `N` step only through the lines a column shows, so with a level filter (`f`) they no longer jump to a hidden line

## [1.23.0] - 2026-10-28

### Added
- `f` filters the focused column by level: errors only, warnings and errors, info and up, then all lines again. An entry's level counts for its indented lines too (an exception's stack stays with it), the column's top row says how many lines it hides, its title shows the filter, and the app keeps it between runs (#28)

## [1.22.1] - 2026-10-28

### Fixed
- A selected row too long for the terminal no longer scrolls whole: its key, icon and label stay put, and only its hint scrolls, in the room they leave (all after the key when the label doesn't fit either) - in the menus, the palette, the About box and the other boxes (#45, thanks @ognjen004028)

## [1.22.0] - 2026-10-28

### Added
- `y` in the requests view asks what to copy the selected request as: the row (`y` again, as before), curl for bash, PowerShell's `Invoke-WebRequest`, `curl.exe` for cmd, an `.http` request (VS Code REST Client, Rider, Visual Studio), JavaScript's `fetch`, or just the URL. ASP.NET's log keeps no bodies, so a POST comes with its content type and an empty body, and the log says so (#19)
- `p` sends the selected GET, HEAD or OPTIONS request again, right from lazyrunner: it shows as the newest request, and the log says how it went

## [1.21.0] - 2026-10-28

### Changed
- Long rows aren't cut off any more: the `,` and `x` menus, the command palette, the About box and the install box grow to fit their longest row (up to the terminal's width), and a selected row that still doesn't fit scrolls sideways, resting at both ends, to show the rest - cut with … with reduced motion (#41, thanks @ognjen004028)

## [1.20.0] - 2026-10-28

### Changed
- The theme picker lists the dark themes, then the light ones under their own heading, and a light theme recolours the terminal's background only once you stay on it a moment - passing by (or wrapping from the top of the list) never flashes the whole terminal white. A theme of your own can say `"light": true`; without it lazyrunner goes by its terminal background (#42, thanks @ognjen004028)
- Keeping a theme (`Enter` or `Tab` in the picker) ripples it out over the screen from where you picked it: a ring in the new theme's own colours, sparkles twinkling on it, the screen ahead dimmed until the wave gets there and the new theme glowing behind it. Previews stay instant; none with reduced motion

## [1.19.0] - 2026-10-28

### Changed
- Menus and lists wrap around: `↑` (or `k`) on the first item goes to the last, `↓` (or `j`) on the last to the first - the `,` and `x` menus, the palette, the About box, the theme, database and branch pickers, the tests, migrations and requests views. A page (`PgUp` / `PgDn`), `Home` and `End` still stop at the ends; the logs and the changelog scroll as before (#44, thanks @ognjen004028)

## [1.18.1] - 2026-10-28

### Fixed
- `Tab` in the theme picker undoes itself: on this project's own theme it gives the project back the shared one. The log says where a project's own theme is kept - your `lazyrunner.config.json`, not the project's `.lazyrunner.json` (#43, thanks @ognjen004028)
- The theme picker listed One Dark twice: the built-in `dark` theme (One Dark's colours, the terminal background left as it is) is now called Dark. Two themes with the same name - one of your own beside a built-in one - show their ids

## [1.18.0] - 2026-10-28

### Changed
- `u` in the About box checks for an update right there instead of opening the startup checklist: what it is doing, then how it ended, shows under its link, and an update it finds is installed as the checklist would (#9, thanks @ognjen004028)

## [1.17.1] - 2026-10-28

### Changed
- `S` cancels a running build from inside the `,` and `x` menus too, and the menu stays open: an action a build blocks (`,` then `r`, the startup checklist) runs right after, without closing the menu and opening it again (#8, thanks @ognjen004028)

## [1.17.0] - 2026-10-28

### Added
- A project can have its own theme: in the theme picker (`t`) `Tab` keeps the selected theme for this project alone (`Enter` still keeps it for every project, and gives this one back the shared theme). It's `"theme"` in the project's section of `lazyrunner.config.json` (#26, thanks @ognjen004028)
- Twelve more themes, the favourites of VS Code and Neovim: One Dark, GitHub Dark and Light, Monokai Pro, Kanagawa, Rosé Pine, Everforest, Ayu Mirage, Night Owl, Shades of Purple, Catppuccin Latte and Solarized Dark
- And four anime ones: Evangelion (Unit-01's purple and green), Zero Two, Rem and Hatsune Miku

## [1.16.1] - 2026-10-28

### Fixed
- The startup checklist no longer closes under you while `r` can retry something: opened by hand (`,` then `r`), or once `r` was pressed, it waits for a key. When it does move on by itself, the time left drains along its bottom edge (#25, thanks @ognjen004028)

## [1.16.0] - 2026-10-27

### Changed
- Links in the output - URLs, and places in files that open in your editor - open on Alt+click, so a click that focuses a column never opens one by accident; a plain click on a link says so once. Ctrl+click works too where the terminal leaves it alone (Windows Terminal takes it for the URLs it finds, cut where they wrap). `,` then `l` (`linksOnAltClick`) brings back the plain click (#11, thanks @ognjen004028)

### Fixed
- A file link in an app's output is looked for in that app's folder first: an Angular app's `libs/...` paths open, also in a repo too big to search through

## [1.15.1] - 2026-10-27

### Added
- `,` then `P` (or the palette) opens the project profile - the `.lazyrunner.json` lazyrunner runs, or your own - in your editor (`"editor"`), as log links open files (#10, thanks @ognjen004028)

## [1.15.0] - 2026-10-27

### Added
- `l` (or →) on an endpoint in the requests view lists its requests, the filter in the header; `h` (or ←, Tab, Esc) goes back to its row. `get_requests` takes an `endpoint` (and `method`) for the same (#20)

## [1.14.0] - 2026-10-27

### Added
- N+1 warnings: a request that runs the same query 10 times or more (its parameters aside) says `N+1` in the requests view; its details show the query, how often and how long it ran, and the usual fix, and an endpoint's details how many of its requests do it. `get_request` has the query (`repeated`), `get_requests` marks them (#18)

### Changed
- A request's database commands are summed up as it finishes, so its details (and get_request) keep them after the app's log lines scroll away

## [1.13.0] - 2026-10-27

### Added
- Requests over MCP: `get_requests` lists what the .NET apps served - newest or slowest first, only the new ones (`since`), one app, errors only, a path - or sums it up per endpoint (count, errors, avg, p50, p95, max); `get_request` has one in full: what it sent and returned, its endpoint's times, the log lines its app wrote while it ran and its EF Core database commands. An agent can now find a slow or failing endpoint and see why on its own (#17)

## [1.12.0] - 2026-10-27

### Added
- An app whose port Windows reserved (Hyper-V, WSL and Docker take blocks of 49152-65535 at boot) doesn't start - it couldn't listen there - and the log says so and how to free the port; the other apps start. The startup checklist's ports check fails for it, next to the ports another process holds (#16)

## [1.11.1] - 2026-10-27

### Fixed
- An npm app whose package.json lists no packages - a script that needs only Node - starts: npm ci makes no node_modules for it, and lazyrunner no longer waits for one
- The startup checklist's database check doesn't warn when the APIs keep no user secrets: there is nothing to check

## [1.11.0] - 2026-10-27

### Added
- `r` in the startup checklist runs the checks that failed or warned again - a failed update, a busy port - without closing it; the footer says which (#6, thanks @ognjen004028)

## [1.10.3] - 2026-10-27

### Security
- HTTPS to GitHub - the self-update and the contributors list - no longer accepts certificates that chain through the distrusted emSign intermediates, whose private keys are public (rustls-webpki 0.103.16, RUSTSEC-2026-0382)

### Fixed
- `y` copies with clipboard-win 5.4.3: 5.4.1 was yanked

## [1.10.2] - 2026-10-26

### Fixed
- `L` with `"gitBeside"` on opens the git tool beside lazyrunner once: more presses while its pane starts are ignored, and while it runs in the project - its folder or its repo's top one, beside lazyrunner or in another window - the log says so instead of opening another (#13, thanks @ognjen004028)

## [1.10.1] - 2026-10-26

### Changed
- `contributors.json` no longer counts commits: they changed with every push, so the list (and `main`) changed almost daily for nothing the About box shows. Code counts by merged pull requests

## [1.10.0] - 2026-10-26

### Added
- The About box's contributors come from the repo: who wrote code (merged pull requests, commits) and who reported bugs and ideas, with what each did ("1 PR · 2 bug reports · 7 ideas"). A GitHub workflow keeps `contributors.json` up to date; lazyrunner fetches it at most once a day as it checks for updates and keeps the one built in for offline
- `p` in the About box opens everyone who contributed, on GitHub

## [1.9.1] - 2026-10-26

### Fixed
- A link's letter in the About box highlights that link before opening it, like the `,` and `x` menus, so `Enter` and `j` / `k` go on from it (#7, thanks @ognjen004028)

## [1.9.0] - 2026-10-26

### Added
- Details beside the requests view (`H`): `Enter` or `Space` (or a double click) shows and hides them, `Esc` hides them. A request's: status, time, protocol, what it sent and returned, how it compares with its endpoint (p50, p95), and what the app logged while it ran - errors, EF Core database commands (count, total, share of the request, slowest) and the lines themselves, marked possibly mixed when other requests ran at the same time
- An endpoint's details: average, p50, p95 and slowest, a sparkline of its latest times, its statuses, its slowest and latest failing requests
- `o` in the requests view opens the selected row's app log

### Changed
- In the requests view `Enter` and a double click show a row's details; `w` opens a GET request in the browser, as before

## [1.8.0] - 2026-10-26

### Added
- `"args"` in a .NET app's profile: arguments `dotnet run` passes the app besides lazyrunner's own (configuration for local runs, say); the column's start line shows them
- The requests view, while it's empty, names the .NET apps that are up but print no ASP.NET log to the console - they log somewhere else (Serilog to Seq, say), so their requests can't show - and how `"args"` can switch that off

## [1.7.0] - 2026-10-23

### Changed
- The command palette as keycaps: every command and setting with its keys as keycap pills, grouped under headers (Run, Tools, View, Help, Settings) until something is typed, then ranked with the typed letters lit; what acts on the focused app names it and shows its icon ("Start Orders"); a coloured selection bar slides to the chosen row a row at a time (40 ms a row, a jump of more than 3 rows cut), and Enter runs the command at once, shows its keycap pressed for a frame and wipes the palette away in 60 ms
- The panel's key line shows the keys that matter now, as keycaps: what the focused app can do in its state (start / build, restart / stop / open, cancel, copy / log after a crash), search, start or stop all and the alerts, with commands, the x menu, keys, settings and quit on the right - those give way first in a narrow terminal
- The settings are no longer listed in the panel (they're in the , menu): the columns get the rows back

## [1.6.1] - 2026-10-23

### Fixed
- The About box's "Report a bug" and "Suggest a feature" open the repository's bug and feature forms (the bug form's version and operating system filled in), not a blank issue (#5)

### Changed
- A letter in the , menu (and the x menu) highlights the item it runs, so a toggled setting is the one in view (#4)
- The About box thanks the people whose bug reports and ideas shaped lazyrunner

## [1.6.0] - 2026-10-23

### Added
- Motion that answers input, never waited for: the hint of the key pressed and the column it acts on flash (every column for an action on all), the column that gets the focus pulses, boxes and views open from their middle, a setting flashes as it flips (in the , menu and the panel), copied lines light up in their column, the search's match flashes as it jumps there, column widths and scrolling glide, and what the pointer is over lights up everywhere (a click shows pressed)
- With restartOnCodeChange, a saved change counts down to the rebuild on the running APIs' columns
- Reloading the profile or switching to an update keeps lazyrunner's screen: the logo shows while the next lazyrunner starts, which opens from the middle - the shell's screen no longer flashes in between

### Changed
- A key during the startup checklist ends it and does what it does at once (Esc, Enter and Space only end it); a key or a click during the Ctrl+R show ends it and acts. The checklist's spinners show for 80 ms at least (150 before)

## [1.5.1] - 2026-10-23

### Changed
- Smoother and snappier: 60 frames a second while something moves (30 before); the light across the startup checklist's logo, the About box's rainbow and a progress bar's glint now flow every frame instead of jumping every 100 ms, and progress bars glide to their value faster

## [1.5.0] - 2026-10-23

### Added
- `lazyrunner mcp`: an MCP server for an agent such as Claude Code (`claude mcp add --scope user lazyrunner -- lazyrunner mcp`). It reads and drives the open lazyrunner - status, numbered logs, the last error with its stack, the screen, start / stop / restart / build, wait for ready / a build / the tests / the migrations check, the tests and the migrations - and each of its actions shows in the log with the agent's name. It acts only in the checkout lazyrunner runs

### Fixed
- The migrations view's snapshot test depended on the day it ran

## [1.4.0] - 2026-10-22

### Added
- The migrations view keeps its checks between runs, per database: after lazyrunner opens again, or the database is switched back, each context shows its last check there and when it was made (the day too once it isn't today)

## [1.3.0] - 2026-10-22

### Added
- The migrations view (M) finds the migrations by itself: the EF Core model snapshots in each .NET app's project and the projects it builds, and their DbContexts - a profile's `"migrations"` is only needed when they're elsewhere (and without `contexts`, its model snapshots name them before dotnet ef is asked)
- `R` in the migrations view finds the contexts again and checks them all; `r` checks the selected one
- `checkMigrationsOnOpen` (`, o`, off by default): M checks every context against the database when it opens

### Changed
- M no longer checks against the database each time it opens, nor after a database or branch switch (the contexts then say "not checked") - unless `checkMigrationsOnOpen` is on

## [1.2.0] - 2026-10-22

### Added
- About lazyrunner (`,` then `A`, or `:` about): the version, its release date and the OS, the core contributors, and links to the README, the changelog, the releases, the license, a bug report (the version and OS filled in), a feature request, the source and an update check - with a rainbow logo, a sparkling title and a beating heart

### Fixed
- Ctrl+R in Windows Terminal: lazyrunner drew in part of the window afterwards, over what was there - it now takes a fresh alternate screen of the window's size

### Changed
- The reload window's show is snappier: under a second instead of about a second and a half

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
