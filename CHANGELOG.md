# Changelog

What changed in each lazyrunner version, newest first. lazyrunner shows the new entries in its startup checklist after
it updates itself; `,` then `c` shows the whole file. How to release a version: README, "Releasing".

## [1.82.0] - 2026-11-12

### Added
- Where a request's time went: its details in `H` draw its timeline from its log lines and when each arrived - the request as a bar (`✗` where it logged an exception), each EF Core command and outbound HttpClient call where it ran with its SQL or URL, a query run over and over as one comb, the time no line accounts for hatched as `in the app`, the longest bar amber, the endpoint's p95 dotted across, an axis of milliseconds. `m` marks a request as the baseline (`★`): another's details compare the two row against row - what differs amber, a digest like `4 rows differ · +180 ms · DB commands 6 → 8` - and draw the baseline's bars as ghosts behind its own. `get_request` gains the waterfall and a `baseline` to compare with (closes #38)

## [1.81.0] - 2026-11-12

### Added
- A Health tab on the Stack screen (`K`, beside Map and Code; `K` opens on the tab used last): a card per app with its p95 latency and 5xx rate over the last minute, its memory and CPU against its targets - braille sparklines with the target dotted across - and the slowest endpoints of the last five minutes as bars, `Enter` opening one in `H`. A reading over its target for a minute is an alert: logged, notified, the app's title flashed and its border red, `! K 1 alert` in the panel, the tab's progress red; `a` acknowledges it (amber) until the app recovers, which the log says with how long it lasted. Memory that keeps growing is an alert too. Targets per app in the profile - `"health": { "p95Ms", "errorRate", "memoryMb", "cpuPercent" }`, else 1000 ms, 5%, 2048 MB, 90% - and `"path"` / `"expectedStatus"` make the response-time probe ask the app's own health endpoint instead of the Swagger page. An agent reads it with `get_health`, and the alerts are in `get_status` (closes #35)

## [1.80.0] - 2026-11-12

### Added
- A Java preset: `"preset": "java"` builds, installs and runs a Maven or Gradle project by its wrapper (`./mvnw`, `./gradlew` - their `.cmd` and `.bat` on Windows) or else `mvn` / `gradle`, its run Spring Boot's (`spring-boot:run`, `bootRun`) or Quarkus' dev mode (`quarkus:dev`, `quarkusDev`) by the plugin its build file names. It's up at Spring's or Quarkus' started line or a server's `started on port`, else its port; `src/**/*.java` and `*.kt` are its code (Quarkus reloads by itself); an exception's line, `ERROR` and Maven's `[ERROR]` lines are its errors; a frame (`at com.shop.Pricing.quote(Pricing.java:42)`) links to its file by its package, the JDK's and the frameworks' frames dimmed. Kotlin services too. The checklist shows `java`, `mvn` or `gradle` and how to get them; a Java app has Java's icon (`jv` without Nerd Font icons); `profile init` adds one for a Spring Boot or Quarkus project, and `profile check` suggests it. Presets' options can now hold by what a file says (`anyFileContains`), and a tool's version is found past a banner or on stderr (closes #92)
- A bar of screens over every screen - Stack, Requests, Migrations, Audit, Tests, Ports, Durations, Timeline - naming them all with their keys, the open one lit: another screen's key, or a click on its name, goes there on the app the open one is looking at (the Map's picked box, the requests' app, the migrations' context, the audited app, the picked suite or port), its column focused; `M` on the Map shows the picked app's migrations, `H` its requests alone. The Map's boxes say what waits on each app in the other screens: migrations to apply `(M)`, failing tests `(T)`, its worst vulnerability `(A)`, 5xx `(H)` (closes #96)
- The build map follows the build: `w` keeps the picked box - and the view - on a project compiling now, moving on to the next one compiling as each is done, and back to the box picked before once the build is over; a running build opens followed, and moving by hand stops it. `B` shows the build's projects alone - built, building and still to build, with their links - without what it doesn't build.

### Changed
- A map's edge that has to go around boxes now runs through the nearest gap between them, not under the whole map, so on a big map you can see where it goes.

## [1.79.0] - 2026-11-10

### Added
- The build as a map (`Alt+b`; `b` on the code map's Code tab; `Enter` on a build in `D`): a .NET app's build over the code map's projects - the running one, else its last. Each project the build compiles is queued, building (spinning), done with its own time (from when what it references was done), failed, or not built; the critical path - the chain of references that decided how long the build took - is in the accent colour, and a row under the keys says `Orders built in 52s · critical path 38s of 52s` or how far a running build is. The picked project says its time against its usual and whether it's on the critical path. It reads the project references and MSBuild's `Project -> output.dll` lines as they arrive, nothing new to install. Each project's own time goes into the durations (`D`, Projects built), and an app's last build is kept for the next session. Agents read it with `get_build`, and following one shows it (closes #82)

## [1.78.1] - 2026-11-10

### Fixed
- A view's keys - the stack and code maps', and every other full-screen view's - no longer scroll sideways when they don't fit: they wrap onto rows of their own between keys, so each key stays in one place and all of them show at a glance. The code map's notes and the lines under it (what the picked part uses, what uses it, its rules) wrap the same way instead of scrolling (#90, thanks @ognjen004028)

## [1.78.0] - 2026-11-10

### Added
- An agent can read every screen: new MCP tools `get_durations` (each build's, start's, install's, suite's and task's last runs and usual time), `get_timeline` (every app's output in the order it came, narrowed by app, words or errors), `get_start_sheet` (an app's command line and environment, secrets masked), `get_checks` (the startup checks, or `rerun` them), `get_marks` (the lines you marked for it) and `get_changelog`. Following an agent, each screen it reads opens - the stack map, the code map, the ports, the audit, the durations, the timeline, an app's start sheet, your marks and the changelog besides the columns, requests, tests and migrations - and `F` after taking the keys back follows again on the screen the agent is at
- `followAgent` is on by default: an agent's call starts follow mode (`, F` turns it off). While the terminal is in the background - minimized, or behind another window - a desktop notification says an agent is at work and what it did, once until you've been back

## [1.77.0] - 2026-11-10

### Added
- A command app (`"type": "command"`): any long-running command - a Python, Go or Rust service, a queue emulator, a script - as a column, with what .NET and npm apps get, each optional. `run` is text or a list of arguments (no shell; its program found on PATH or by its path from `cwd`, a `.ps1` run by PowerShell), and `{port}` in it and in `env` is its `port`, which the ports view and port conflicts see. `build` runs before a start and on `b`, skipped when nothing in its folder changed since its last good build; `install` is what `i` runs. `ready` says when it's up: a port, a line of its output, or a 2xx from a path. `watch` globs name its code, and with restart on code change saving one restarts it. `errors`, `warnings` and `frames` colour and count its lines and make its stack frames links; Go's and Rust's `file.go:42` and `src/main.rs:42:5` are links anyway. Stopping takes its whole process tree down, and the startup checklist says whether its programs are found. The profile check and schema know the new fields, and agents start, build and install command apps through the same tools (#56)
- Presets for command apps, and the first: Python. `"preset": "python"` fills in each field a command app leaves out by what's in its folder - `uv sync` with a `uv.lock`, else `pip install -r requirements.txt` for `i`; a run for Django, `main.py` or `app.py`; its `.venv` first on PATH with `VIRTUAL_ENV` set (no activation script) and `PYTHONUNBUFFERED=1`; `*.py` as its code; tracebacks and `ERROR` lines as errors and their frames as links; its Python's version (and uv's) in the startup checklist. Anything set by hand wins. Presets are data (`projects/presets`): a `presets` folder beside your profiles or a profile's own `"presets"` adds or replaces them, each with `options` for what depends on the folder. `profile init` adds a command app with its preset for a Python folder, and `profile check` suggests it for one without. Command apps also get `path` (folders first on PATH), `tools` (versions in the checklist) and `{cwd}` (#61)
- A Go preset for command apps: `"preset": "go"` installs with `go mod download` (`i`), builds with `go build ./...` (before a start, skipped when nothing changed) and runs `go run .` where there's a `main.go`. `*.go`, `go.mod` and `go.sum` are its code; `panic:`, `fatal error:` and `level=error` lines (slog's text or JSON) are errors, and its frames (`handler.go:42`) links. The startup checklist shows `go version` - a tool's version falls back to `<tool> version` when `--version` isn't one. `profile init` adds one for a folder with a `go.mod` and a `main.go` (#61)
- A Rust preset for command apps: `"preset": "rust"` builds with `cargo build` (before a start, skipped when nothing changed) and runs `cargo run` where there's a `src/main.rs`, with `RUST_BACKTRACE=1` so a panic says where. `*.rs` and its `Cargo.toml`s are its code - not `Cargo.lock`, which a build may rewrite. `error[E…]:`, `panicked at` and `ERROR` lines are errors, `warning:` lines warnings, and its frames (`src/main.rs:42:5`) links; `cargo --version` is in the startup checklist. `profile init` adds one for a crate with a `src/main.rs` (#61)
- Tasks: one-off commands beside the apps - seed the database, regenerate a client - in the profile's `"tasks"` (`key`, `name`, `run`, and `cwd` and `env` when they need them). `:` then its name runs one in the background, the palette saying beside it how its last run went (`✓ 12s`, `✗ 3s · its error`) or how long it has run. Its output goes to the panel's log between a rule with its name and its result - how long it took against its usual time, or its first error line - and the header shows it while it runs. One that took over 10 seconds says when it's done with a notification, and the durations (`D`) keep its times. Agents run one with `run_task` and wait for `task_done`; `get_status` lists them; hooks get `task.passed` and `task.failed` (#56)
- A compose app (`"type": "compose"`): Docker Compose services - the database, a cache, a queue - as one app. Starting it runs `docker compose up` for its `services` (all without them) in its `cwd`, with Docker's own compose file or the one `file` names; its column shows their output, errors coloured and counted, and its title each service's state (`● db` up, `◌` starting, `✗` down, `✓` a one-off that did its job). It is up once each runs, healthy when it has a healthcheck, and a service that changes state says so in the column. Stopping it - or quitting - stops its services (`docker compose stop`), a start waiting for that. The startup checklist says whether Docker is running, `profile check` whether the compose file is there, `profile init` adds one for a folder with a compose file, and agents see the services' states and ports in `get_status`. Compose's `db-1 |` prefix becomes a thin gutter in the service's colour, its name where the service changes; a starting service's dot breathes. On the stack map a compose app's box lists its services, and a database service (postgres, SQL Server, MySQL...) is the database box, Enter on it going to the app it runs in (closes #56)
- A task running shows in the header with a spinner and, once it has passed before, a bar against its usual time (`▰▰▱▱ ⧗ 12s`) - the terminal tab's and taskbar's progress too. One that passes glints green and its output folds away (the log file keeps it); one that fails pulses red and stays in the header with its first error line until it's clicked away. The palette's tasks have an icon of their own
- A command app's line that made it up is outlined green for a few seconds, and its icon is its language's by the program it runs (Python, Go, Rust, Node.js) - Docker's for a compose app
- A command app's install: `setup` runs before it while none of its `path` folders is there (Python's pip options make the project's `.venv` first, so packages never land in the global Python), and `installFrom` names the files it installs from (`uv.lock`, `requirements.txt`, `go.mod`) - when they change since lazyrunner's last install, the startup checklist's packages row (its `.venv` in it) and the panel say it's due. `ready` takes `"orPort": true` with a line: up at the line or once the port listens, whichever comes first - Python's preset is up at uvicorn's, Flask's or Django's line, else its port. `path` takes `~/` folders, and Rust's looks for `cargo` in `~/.cargo/bin` (#61)
- Command apps by their language: a library's stack frames (site-packages, Python's and Go's own sources, Go's module cache, the cargo registry) are dimmed so the app's own stand out, and a Python traceback counts once, at its exception line. A `cargo build` moves its bar with its `Compiling` lines. The startup checklist shows tools as capsules (`python 3.12.1 · uv 0.5.1`), says how to get a missing one (`winget install astral-sh.uv`), and gives a command app's row its language's icon - without Nerd Font icons a `py`/`go`/`rs`/`js` chip, in its title too. A preset `profile check` would suggest shows in the start sheet, dimmed, until it's set (closes #61)
- A compose app's published ports (from `docker compose config`) are in the ports view and port conflicts - a port Docker itself holds is its own services, never killed - and the start sheet's env overrides reach `docker compose ps` and `stop` as they do `up`. Desktop notifications come only while the terminal doesn't have the focus, when it says (#56)

## [1.76.0] - 2026-11-09

### Added
- A code map, the stack view's Code tab (`K` then `Tab`; on the Map, `c` opens it on the picked app's projects): what depends on what in the code, both sides of the stack. It opens on the apps, linked when one's code uses another's and dashed with how many projects two share; `Enter` shows an app's projects - the .NET projects linked by their project references, each with what it is, its framework, files and packages; the TypeScript projects - an Nx workspace's, or the app - linked by their imports through the `tsconfig` paths, a lazy route dashed - and `a` all of them. `Enter` goes inside a project (a .NET project's namespaces linked by their usings, a TypeScript project's folders by their imports) and inside a folder (its NgModules and components, linked by their `imports: [...]`), what a level uses from elsewhere dashed with where it is; `Backspace` back out. In an Nx workspace, `nx graph` adds the dependencies only Nx knows of, dashed in blue. Cycles are red and `c` steps through them; a link a rule doesn't allow is yellow - the profile's new `layers` (`mustNotReference`) or Nx's module boundaries (`depConstraints`). For a big map: `/` searches every level and the source files by name, and `Enter` (or `g` on the picked box) shows its graph - all it depends on and all that depends on it; an edge another way already implies is hidden (`r` shows them); `f` keeps the picked box's neighbourhood with its own lines (on by itself for a big level); `z` zooms out to compact boxes, then just the names with the picked box's lines, `Z` back in; `t` hides the test projects; crossing lines hop each other and the picked box's are drawn on top; a minimap shows where you are. Under the map, what the picked box depends on and what uses it, by name, what a change to it affects, its tags and the rules it breaks. With motion on, a level zooms in or out, its edges draw in outwards from the picked box, and focus dims what it hides before it goes. Each file is read again only when it changed, the rest kept in the state folder. The MCP's `get_dependencies` gives an agent the same: the apps (and what they share), projects, cycles and rule breaks, or what one part - or a source file's - depends on and what uses it, with the way to each (#89)

## [1.75.0] - 2026-11-09

### Added
- A stack map (`K`): which app calls which, and how those calls go. The apps are boxes in columns by who calls whom, with the database after the apps that use it (their projects reference EF Core or a SQL client, or their requests ran commands). The edges come from the calls the .NET apps really make - lazyrunner turns on HttpClient's log for the apps it starts and maps each call's port to the app listening there - and the npm apps lazyrunner starts, through a small Node hook (`NODE_OPTIONS=--require`) that writes a line for each call their dev server's proxy makes. Until an npm app's calls are seen, its own config naming another app's port (`proxy.conf.json`, `environment.ts`, `.env`) or the profile's new `dependsOn` declares them. Each box says as much as fits - how long the app has run, its CPU and memory, its requests or proxied calls a minute with their p95 and failures, its errors and warnings, its slowest endpoint and last build or its npm script; the database its commands, the slowest and possible N+1s - and with many apps just the name and state, so nothing overlaps. A box says what its app is doing, building included. Each edge is cyan with calls, red with a failure in the last minute, and says how many it carries; under the map, every edge's calls, p95 and failures. `h` `j` `k` `l` choose an app, `Enter` goes to its column. With motion on, the edges draw in, a dot travels along each one that carries anything (yellow on a failing one, cyan to the database) and a new failure pulses its edge twice. The MCP's `get_stack` gives an agent the same map, with each edge's last minute and the newest failed calls (#51)

## [1.74.0] - 2026-11-09

### Added
- A failure's likely cause. Each test run keeps what it ran on, so a failed test that passed in an earlier run says what changed in between - git's diff between the two runs' commits, and the files that differed on top of them. Under the failure (and the picked test, `l`), `LIKELY CAUSE · changed since it last passed (09:41)` lists the changed sources as links: those its stack goes through first, then those of its project and the ones it references (`Orders.Tests ← Orders.Core`, the look of the affected suites' why), then the rest. It's left out rather than guessed when the test never passed in the runs kept or a commit is gone. `y` copies it with the failure, and the MCP's `get_test_results` gives each failure its `likely_cause` (#34)

## [1.73.1] - 2026-11-06

### Fixed
- Picking a test in the tests view (`l`): a test name too long for the list scrolls sideways instead of losing its start to `…`, and so does the line above it (the suite's name and how many tests it has) when the pane is narrow

## [1.73.0] - 2026-11-06

### Added
- Run what a change affects. Every whole run of a suite keeps what it ran on, so the tests view can tell which suites what changed since affects: a .NET suite through its own project and the ones it references, directly or not (a build file no project owns, such as `Directory.Build.props`, affects all of them), an npm or Nx suite through its own folder. `v` has a fourth list, affected, that keeps every suite but dims the others, ends each affected row with a `●` (an amber `?` where it can't be told, which runs with them), and says `affected 2 of 6` or `nothing changed since the last runs`; `Alt+r` runs the affected suites. A suite's details say why under its run cells - `AFFECTED BECAUSE`, `Orders.Tests ← Orders.Api ← Orders.Core`, then the changed file, a link - and the MCP's `list_tests` marks `affected` with its why, `run_tests` takes `affected: true`. With motion on, the why assembles hop by hop, the left-out rows fade as the list switches, and a suite that becomes affected while the view is open gets a sweep before its `●` lands (#78)

## [1.72.1] - 2026-11-05

### Fixed
- The clock that marks what's queued (tests, migrations) showed as a solid disc in Cascadia Code NF and Cascadia Mono NF, whose Font Awesome clock is drawn filled: it's the Octicons clock now, which both Nerd Fonts draw as a clock

## [1.72.0] - 2026-11-05

### Fixed
- The audit view's scans no longer slow the whole machine down. Each `dotnet list package` starts an MSBuild worker per core, and with several apps and both commands at once they took every core and gigabytes of memory - their workers lingering for minutes after. Now the scans run below normal priority (what they start inherits it), MSBuild's workers end with them, a .NET app's two commands run one after the other, and two apps are scanned at once by default instead of three (`+` raises it) (#79)

### Changed
- A column's title moves as its app changes state (not with reduced motion): coming up fills its chip green from the left, stopping drains the colour it had from the right, a crash pulses it twice
- An empty column says what its app is - `not started yet`, `stopped · started 10:42`, `exited · code 1`, `couldn't start` - and the focused one what starts it, as before
- The panel's log keeps each line's mark (`✓` `×` `!` `▸`) in a column of its own, the newest line slides in from the right, and a line too long for the panel scrolls sideways instead of being cut
- The settings (`,`) are switches - a green `●` on, a dim `○` off - grouped under Startup, Running, Display and Integrations when the terminal has room
- The palette's Settings entry has the settings icon (it had Help's `?`)

## [1.71.0] - 2026-11-05

### Added
- The panel's header has an `A` chip for what an audit scan found since the audit view was last open (`A 3 new findings · 1 critical`), and while the audit or the migrations view is closed, a chip for what it does in the background (`A ⠋ scanning 2 of 5`, `M ⠋ checking · 2 left`); a click opens the view
- `e` in the audit view puts each vulnerable package's advisory on a line under it: `└ title · fixed in 13.0.4 · +2 more`

### Changed
- Durations (`D`) is full screen like the other views: `j` `k` pick an item, and its details show its runs as a taller chart that grows in as it's picked and each run listed - when, how long, failed or not; the header names the item that slowed down most
- Every full-screen view starts the same way: its name in a chip, its keys on the row under it - the timeline's keys moved up from the bottom - and keys that don't fit scroll sideways instead of being cut off at the edge
- A column's title that doesn't fit leaves out the app's port before its state is cut
- A long output line wraps at a space or punctuation when one is near the edge, so words and paths stay whole

### Fixed
- The requests view's header said `1 errors`

## [1.70.0] - 2026-11-05

### Changed
- The audit view (`A`) scans three apps at once, and an app's two commands side by side; the header names those scanning and counts them off (`2 of 5 done`). `+` and `-` change how many (1 to 8, saved as `auditParallelism`) (#79)
- The migrations view (`M`) checks three DbContexts at once - a migration that's applied or reverted still runs alone; `+` and `-` change how many (1 to 8, `migrationParallelism`)
- A scan or a check that fails beside others goes again on its own once the rest are done, before it counts - a feed or a database may refuse a burst it would answer one at a time; a scan that fails for good keeps the one before, and its time

## [1.69.2] - 2026-11-05

### Changed
- The port map (`P`) shows its keys under its header, as the other views do, and its status line under the occupancy strip; `!`'s question takes the keys' row while it asks (#81)

## [1.69.1] - 2026-11-05

### Fixed
- The audit view's `new` chip sits after a finding's severity now (`▕HIGH▏ new`), so a package's name keeps its whole column instead of losing it to the chip (#79)

## [1.69.0] - 2026-11-05

### Added
- `P` opens the port map, full screen: every port the apps want and who really holds it. Its header counts the conflicts, an occupancy strip gives every port at a glance, and the table - the conflicts first - has the port, its app, the process holding it (PID and command line), its state (`●` free, `◐` its app, `×` held by other, `∅` reserved by Windows), for how long, who held it over the last minute (on a wide terminal) and what to do (`! stop it · since 09:39:58`). The ranges Windows reserved are listed under it, and a status line says what's wrong (#81)
- In it `!` stops what holds the picked port (after a yes; its row dims and spins until the port is free), `Enter` goes to its app's column, `w` opens it, `r` reads the ports again and `y` copies what frees the reserved ranges (#81)
- While it's closed, a stopped app's port that something else has taken puts `! 5001 taken` in the panel's header, lighting up once; a click opens `P` on it (#81)
- It moves (not with reduced motion): the strip fills and the rows come in as it opens, a port that changes flashes, a new conflict slides in with a red edge that pulses twice (and while `!` asks about it), and a freed port wipes out what held it while `● freed` draws in, then flashes green (#81)
- The MCP's `get_ports` gives an agent the same table (#81)
- `I` in the audit view installs many fixes together, after asking which - `c` the critical ones, `h` high and up, `m` moderate and up, `a` all; the question says how many each takes and how many are a new major. The app stops once, an npm app's fixes go in one `npm install` (#79)

### Changed
- The audit view moved from `O` to `A`, for Audit (#81)
- With no port in the way, `!` opens the port map instead of doing nothing (#81)
- The key reference says what capitals are: all apps, or a tool (#81)

### Fixed
- In the audit view a long package name no longer runs under its `new` chip or into its version: it ends in `…` before them (#79)

## [1.68.1] - 2026-11-05

### Changed
- The audit view (`O`) follows its concept more closely: a vulnerable row has an edge in its severity's colour and its severity framed (`▕HIGH▏`), an outdated-only row is dimmed, the packages' title says `/ filter`, the apps' list notes how packages are counted, and a `R` that scanned several apps ends with one line in the log - `▸ Audit finished in 3.8s · 12 packages · 3 high · 1 critical` (#79)
- And it moves more (not with reduced motion): the selection glides between the packages, the header counts the apps off while `R` scans (and pulses a count a scan adds to), a resolved row fills green from the left, a `new` chip blinks once, a fix being installed spins in its row and one that failed pulses it red, `y` lights the command it copied, `/` lights what it matched, and the last scan's age turns amber after a week and red after a month (#79)

### Fixed
- The audit view's details no longer cut off a long row beside the table - the folder `i` runs in, the major-version warning, another advisory's title: they wrap at their words (#79)

## [1.68.0] - 2026-11-04

### Added
- `O` opens the audit view: each app's outdated and vulnerable packages, the worst first, with their advisories and the command that fixes each (`y` copies it, `i` installs it after a yes, `o` opens the advisory). `r` scans the selected app, `R` every app - `dotnet list package` for a .NET app, `npm audit` and `npm outdated` for an npm one - in the background, and only when asked; the last scan is kept and the view opens on it, says what it fixed since the one before and marks what's `new` (#79)
- A column's title says `⚠ 2 high` while its app's last audit found something high or critical, the startup checklist has a Vulnerabilities row from the kept scans (`f` opens `O`), and the MCP's `get_audit` gives an agent the scans with their fixes (#79)

## [1.67.2] - 2026-11-03

### Changed
- The migrations view (`M`) opens on the DbContext with the newest pending migration, else the one with the newest migration, instead of the first

### Fixed
- In the start sheet, the picked variable's value scrolls sideways when it's longer than its room (a revealed secret, a long connection string), as the other lists' rows do

## [1.67.1] - 2026-11-03

### Fixed
- In the tests view, a suite's counts read `✓12 ×13 ≈1` again without spaces, in marks narrower than a cell: with Nerd Font icons the check ran into the number, and `✗` and `↻` weren't in every Nerd Font

## [1.67.0] - 2026-11-03

### Added
- Environment overrides, from the start sheet (`e`): what an app gets on this machine on top of what it's started with, kept in lazyrunner's state and never in a shared file. `a` adds one, `Enter` overrides the picked variable (a user secret through its variable), `d` drops one, `b` keeps one to this branch, `s` moves one that looks like a secret into the user secrets; `v` shows only the overrides and `V` copies them as what they change. A .NET app gets them as `dotnet run -e`, which beats its launch profile; an npm app in its environment. The column's title says `env*`, and `get_status` lists them (#80)

### Changed
- The start sheet lists the app's environment as one list, each variable with where it comes from (`launch`, `profile`, `secrets`, `override`) and what it hides; a secret's value wipes in when `Space` shows it (#80)

### Fixed
- The start sheet showed the profile's `env` as if it applied where the launch profile sets the same variable; `dotnet run` gives the launch profile's precedence, and the sheet now says so (#80)
- In the tests view, a suite's counts put a space after each mark (`✓ 12  ✗ 13  ↻ 1`): with Nerd Font icons the check ran into the number

## [1.66.0] - 2026-11-03

### Changed
- The startup checklist is laid out as the concept in #37 has it: a header with the project, a bar of the checks done, the counts, how long they took and the slowest; the groups Environment (with lazyrunner's version), Project (the profile, the ports, the branch), Database, Packages and Update, each under a header line with its worst result and time, all open unless the terminal is too short; `FIX` tags; the picked check's details beside the list (under it when narrow); a legend and the keys (#77)

### Fixed
- The startup checklist's selection starts on the first row, and `j` / `k` follow the rows as they're drawn instead of the order the checks ran in (#77)

### Added
- In the startup checklist, `n` / `N` jump to the next and previous problem, `f` fixes the picked check or the first problem, and `i` ignores a problem for this run (#77)
- The startup checklist moves (not with reduced motion): the selection bar glides between the rows and flashes going round the end, the header's counts tick up a check at a time and pulse when they change, each check's mark lands bright as it finishes, and a bar with an hourglass drains with the countdown to the start (#77)
- In the startup checklist a click picks a check, and a click on its `FIX` tag runs the fix; a click inside it no longer continues (one outside it still does) (#77)

## [1.65.0] - 2026-11-02

### Added
- A suite's last runs as a row of cells with its times as a braille line two rows high under them, the usual dotted across, amber where a run was slower than the usual; `[` / `]` pick a run - when it finished, its counts, whether it ran on the same code, the tests that failed in it (#32)
- Each suite's row in the list shows its last time and a line of its last times; the failures start with `FAILURES 13` and a `NEW 3` badge (#32)
- Backend, Frontend or an app shows its suites' runs as a grid, the newest on the right, their lines on one scale; the project's run history (time, passed, failed, how long) is under the list (#32)
- `/` in the tests view narrows the list by name, `Alt+f` runs the flaky tests again, and a run just finished glints in (#32)

## [1.64.0] - 2026-11-02

### Added
- Test history: each suite's last 20 runs, test by test, with the code they ran on. A failure is new or known, a test that failed and passed on the same code is flaky, one passing again after a failure is fixed; the tests view says `✓214 ✗13 ↻1` and `3 new`, splits the failures `(3 new · 10 known)` with the new ones first, and lists the fixed and the flaky tests (#32)
- Every test shows its last runs as a strip, two runs a cell, the newest on the right - in the details and beside each test to pick (#32)
- `v` in the tests view lists all the suites, those with failures, or those with flaky tests; the panel's chip counts the new failures, and `get_test_results` marks each failure `new` and `flaky` (#32)

## [1.63.0] - 2026-11-02

### Added
- `c` in the startup checklist copies it as a Markdown report for an issue: lazyrunner's version, the project and its apps with their ports and folders, each check's result and time, and what fixes the problems, secrets masked as in the diagnostics zip (#37)

## [1.62.0] - 2026-11-02

### Added
- The startup checklist fixes what it finds: `j` / `k` pick a check, `f` runs its fix - `npm ci` for missing or stale packages, stopping what holds a port, the database picker, checking again, or copying `net stop winnat; net start winnat` for a port Windows reserved - and `a` runs every safe one in turn (#37)
- Its checks are grouped and timed, each with a bar of its time against the slowest; a group that passed folds while another has a problem, a failed check has a red edge and a warning an amber one, and a fix that worked flashes green while one that didn't pulses red (#37)
- At startup a failed check holds the start until it's fixed or `Enter` starts anyway; once nothing blocks it, it counts down to the start (`Esc` holds it) (#37)

## [1.61.0] - 2026-11-02

### Added
- With `Alt+d` on, `Enter` lists what was fixed: the last run's errors and warnings that didn't come back, with their links (#54)
- A run with new errors puts a chip in the panel's header, `1 Orders +2 new errors`: the column's number or a click shows them with `Alt+d`, on the first one (#54)
- A good build says how its warnings changed since the last good build - `12 warnings (+3)` - and lists the new ones in its column; each warning counts once (#54)

## [1.60.0] - 2026-11-02

### Added
- `Alt+d`: what's new since an app's last run, in the focused column. The lines the last run printed too dim - times, durations, ports and ids left out of the comparison - and new ones keep their colour, `▲` before a new error or warning, `●` before any other new line; the top row counts them (`+2 new errors · 3 fixed · 41 new lines`, `fixed?` until the app is up) and `n` / `N` step through them. The rail marks only what's new and sends its ember only for a new error; the last run is kept in the state folder, so the first run after lazyrunner opens again compares too (#54)

## [1.59.0] - 2026-11-02

### Changed
- lazyrunner's MCP answers are compact JSON, and `get_logs` and `get_request` give their log lines as one text, each line after its number, instead of an object per line - the same answers for far fewer tokens. Anything that read `lines[].seq` reads `text` now (#76, #75, thanks @ognjen004028)
- The requests view and `get_requests` sum up paths with a user's name in them (`/api/users/image/jo@contoso`) as one endpoint, `{id}` (#76, #75, thanks @ognjen004028)

## [1.58.0] - 2026-11-02

### Added
- The migrations rail starts each month with a `┼ Apr 2026` tick - after a gap, how many months on - and each migration shows its day beside it (#39)
- After a failed migration, the rest of the way to the target stays cracked (`⠅`, red) (#39)
- A history the database couldn't be read for is drawn dashed, with `◌` nodes and no bead, instead of a bead above the oldest migration (#39)
- The status bar shows `○ 3` beside the database while migrations are pending on it (#39)

## [1.57.0] - 2026-11-02

### Added
- Migrations as a timeline (`M`): a context's migrations on a rail, oldest at the top - `●` applied, `○` pending, dates from their ids with a gap where months pass, the older ones folded - and the database as a bead, **database is here**. Choosing a target lights the path to it: green flowing down to apply, amber flowing up to revert, `⚠` on what reverting likely loses (named in the confirmation too). While `dotnet ef` works the bead travels a node a hop as each migration is done - in turn, catching up when several finish at once - and breathes between them; nodes fill or empty with a glint, the list shows a `▰▰▱▱ 2/4` bar, a shimmer runs along the stretch once done, and a failure pulses red with its reason on the node (#39)

## [1.56.1] - 2026-11-02

### Fixed
- Holding ↓ or ↑ in the command palette moves the selection smoothly: the bar keeps a row behind the key instead of stalling and then jumping (#74, thanks @ognjen004028)

## [1.56.0] - 2026-11-02

### Added
- The timeline's activity strip: down its right edge, one lane per app over the whole stretch kept - oldest at the top - each cell shaded by how many lines the app logged then, red where one was an error, and an edge beside the time the rows on screen cover (dim while following, cyan when scrolled back). It follows the search and the level filter, so `level:error` shows where errors cluster (#53)

## [1.55.0] - 2026-11-02

### Added
- The timeline brackets a request's lines: a faint rule above its first line with its method, path, status and time, `│` down its lines and `╰` on its last, `┆` beside other apps' lines that came while it ran. Its own lines by RequestId, or the lines it logged while it ran when no other request ran with it (#53)

## [1.54.0] - 2026-11-02

### Added
- The timeline searches and filters: `/` narrows it to lines holding some words, `app:orders` to an app's lines, `level:warn` to warnings and up - together as you like; `f` cycles the levels as in the columns, and the header says how many lines of all are shown. `Esc` shows every line again (#53)

## [1.53.0] - 2026-11-02

### Added
- The timeline (`~`, or the palette): every app's output in one list, in the order it came - each line tagged with its app and coloured by its level, a quiet stretch of more than 3 s shown as a dim `· 4.2 s ·` row so bursts stand apart. `j` `k` scroll, `G` follows new output, `Enter` jumps to the picked line in its own column (#53)

## [1.52.0] - 2026-11-02

### Added
- A request's log lines are exactly its own: lazyrunner starts the .NET apps with their console log's scopes on (`--Logging:Console:IncludeScopes=true`) and matches each line to its request by `RequestId`, so overlapping requests no longer mix and the details panel drops "possibly mixed" for them. The scope lines stay out of the columns; an app that logs no scopes keeps the time-window match (#21)

### Fixed
- The self-update tests' local server closes each connection gracefully, so a download can't be cut short on a busy Windows runner

### Changed
- The demo workflow uses `actions/setup-dotnet@v6` and `actions/setup-node@v7` (#72, #73)

## [1.51.0] - 2026-11-02

### Added
- Test watch runs only the .NET test projects a save affects: the saved file's project is theirs or one they reference, directly or not; the log says why - `Orders.Tests (references Orders.Core, which changed)`. A file no project owns (`Directory.Build.props`) still runs them all (#40)

## [1.50.0] - 2026-10-30

### Added
- Test watch shows its countdown: while a save waits for the quiet, a capsule beside `◉ WATCH` (the tests view's header, the panel) drains over the real 400 ms and fills again on the next save; a suite the watch queues gets one sweep across its row. With reduced motion, `rerun in 0.3s` and no sweep (#40)

## [1.49.0] - 2026-10-30

### Added
- A watched suite (or its pinned test) that turns red, or green again, gets a desktop notification: `Cart.Tests fails now` / `adds_the_new_item in Cart.Tests passes again`. A run that stays the same colour notifies nothing, and a red suite whose failed tests pass is only green once all of it has run (#40)

## [1.48.0] - 2026-10-30

### Added
- Pin-watching one test: in the test picker (`l`), `a` watches only the picked test - a save runs just it, the tight loop for a new test. The suite's row and the header say `pinned: <test>`; `a` on it again stops, `a` on the suite (or `A`) watches all of it again, and a pinned test that's gone after a rename stops being watched (#40)

## [1.47.0] - 2026-10-30

### Added
- Test watch: `a` in the tests view watches the selected suites, `A` all of them. A watched suite (`◉`) runs again when its code is saved, once saving is quiet for 400 ms - a green one whole, a red one its failed tests, then, if they pass, whole. .NET suites follow the profile's watch folder, npm and Nx ones their own. A running suite is never cancelled (a save during its run runs it again after); one log line per batch of saves; `◉ WATCH 2` in the tests view and the panel. For this session only; an agent sees what's watched in `get_status` (#40)

## [1.46.0] - 2026-10-30

### Added
- `p` (the `x` menu, the palette): run the focused app on another launch profile - a .NET app's, with their URLs - or npm variant. `Enter` restarts it on the one picked, remembered for this branch in lazyrunner's state (not the shared profile); its column's title says so (`Orders · Orders-Https`), its port follows, and picking the profile's own forgets the choice (#50)

## [1.45.0] - 2026-10-30

### Added
- The rail shows how fast the app logged each stretch: its ordinary rows, dim ticks until now, are braille - a dotted line at the app's usual pace, fuller where it logged twice as fast or more, brighter where a burst came (eight times or more, a compiler's report, say). Errors, warnings, matches and marks keep their colours (#69)

## [1.44.1] - 2026-10-30

### Fixed
- `G`'s shimmer on the rail starts from where the view was as the view glides home, runs down the map to where the bracket starts, then down the bracket to its `▼`; it started only once the glide had ended, from next to the `▼`, so a long log showed just its last row (#69)

## [1.44.0] - 2026-10-30

### Added
- The rail's top end dims while an app's buffer drops its oldest lines, so the rescaling map reads as that end moving; it clears a moment after the last one (not with reduced motion)

### Fixed
- A rail showing for the first time no longer counts what the column had before as new: no ember for an old error, no dimming for lines dropped earlier (#69)

## [1.43.0] - 2026-10-30

### Added
- The rail moves: a new error that lands off-screen, while a column is scrolled back, sends an ember from the bracket to its mark - the mark waits, dim, until the ember eases into it, then settles in (a burst sends one, at most one every 750 ms, its tail longer for each error it carries); as `G` snaps the view back to live, a shimmer runs down the map from where the view was and hands over to the bracket's `▼`; a search's jump pulses its row once, bright then fading. In the theme's focus colour, so light themes show it too; none of it with reduced motion (#69)

## [1.42.0] - 2026-10-30

### Added
- The rail previews and drags: the pointer on a row of a column's rail shows the line a click there brings into view, over the column in the line's own colour, and a drag along the rail scrolls the column as it goes (#69)

## [1.41.2] - 2026-10-30

### Fixed
- A label too long for its place scrolls sideways to show the rest, as a selected row does, instead of being cut with … - the update check's line in the About box, the startup checklist's details, the panel's notice line, and a column's title and its info line. Several scroll at once; what names the line (a check's name, an app's chip) stays put (#71, thanks @ognjen004028)

## [1.41.1] - 2026-10-30

### Fixed
- A spinner of your own spins while an app starts too (`starting`, `compiling` in its title), where lazyrunner's blinking dot was (#70, thanks @ognjen004028)

## [1.41.0] - 2026-10-29

### Added
- The start sheet (`e`, in the `x` menu and the palette): what the focused app is started with - the command line and its folder, the launch profile (its URL and variables) or npm script, the profile's `args` and `env`, and the user secrets with their values masked (`Space` shows one for 5 s; a connection string reads `database @ server`). A part that changed since the app started says so, and `r` restarts it; `y` copies the sheet, secrets masked
- `"env"` in a profile's app: environment variables the app gets on top of lazyrunner's own (#55)

## [1.40.0] - 2026-10-29

### Added
- Hooks: your own commands, run when something happens - `"hooks": [{ "on": "tests.red", "run": "..." }]` in `lazyrunner.config.json`. The events: an app up or crashed, a build passed or failed, tests green or red, migrations applied, an update installed (`"app"` narrows one to an app). A hook runs in the background with what happened in `LAZYRUNNER_*` variables; one that fails or runs past 60 s says so in the log. `lazyrunner hook test <event>` tries them (#57)

## [1.39.0] - 2026-10-29

### Added
- A rail in place of the scrollbar: a map of all a column's lines - errors red, warnings yellow, search matches cyan, marks magenta, the rest dim ticks - with a bracket on the lines on screen (`▼` while it follows new output). A click on it jumps there. A narrow column gets the map alone (#31)

## [1.38.0] - 2026-10-29

### Added
- A spinner of your own, as lazygit's: `"spinner": { "frames": ["◐", "◓", "◑", "◒"], "rate": 120 }` in `lazyrunner.config.json`, for every project or in a project's section for that one. It spins wherever lazyrunner's would; one that can't be used says why in the log, and lazyrunner's own spins (#64, thanks @ognjen004028)

## [1.37.0] - 2026-10-29

### Added
- Your own themes (`themes/` in the settings folder) are listed apart in the theme picker, after the built-in ones: Custom dark, then Custom light. A theme of yours with a built-in one's id is listed as yours (#65, thanks @ognjen004028)

## [1.36.0] - 2026-10-29

### Added
- The diagnostics zip's path in the log is a link: Alt+click on it (a click, with `, l` off) shows the zip in its folder - selected in Explorer (#66, thanks @ognjen004028)

## [1.35.2] - 2026-10-29

### Fixed
- A held key acted over and over - a held `f` in the settings flicked reduced motion on and off. It acts once now; moving and scrolling (`j` `k`, the arrows, `PgUp` `PgDn`…) still repeat, and so does typing in a prompt (#67, thanks @ognjen004028)

## [1.35.1] - 2026-10-29

### Changed
- The bug report zip is now the diagnostics zip, and moved from the settings to the About box, under Report a bug (`,` `A` then `z`, or the palette), so the two bug reports aren't mistaken for each other. It goes into the state folder's `diagnostics` (#68, thanks @ognjen004028)

## [1.35.0] - 2026-10-29

### Added
- A bug report (`,` then `b`, or the palette): a zip with lazyrunner's and the tools' versions, the apps and their states, the settings and the profile with their secrets masked, each app's recent output and lazyrunner's log, and the screen as a picture - ready to attach to an issue (#22)

## [1.34.0] - 2026-10-29

### Added
- "New since you looked" chips in the panel's header: `H 3 new errors`, `T 2 suites done · 1 red`, `M 1 pending` - what happened in the requests, tests and migrations views since each was last open. The view's key or a click on the chip opens it on the first new thing; having it open clears the chip. A chip pops in once, then stays still (#49)

## [1.33.0] - 2026-10-29

### Added
- MCP for everything the keys do: `list_actions` names every action and setting, `run_action` runs any action by its key, `press_keys` presses keys for what no tool covers (answering with the screen; it waits while you type in a prompt), and `switch_database`, `install_packages`, `apply_migrations`, `kill_port_holder`, `set_setting` do those by name. What can't be undone or takes something from you needs `"confirm": true` (#62)

## [1.32.1] - 2026-10-29

### Fixed
- An empty column's hints (`u starts it`, `b builds it`, `U starts all`) start in one column, the block centred, instead of each line centred on its own

## [1.32.0] - 2026-10-29

### Added
- Follow mode (`F`): the screen follows an agent working over MCP - the column whose output it reads, scrolled to the lines it read; the requests view with its filters; the tests view on its suite; the migrations view - and outlines what it read for a few seconds. Your keys rest meanwhile (all but `F` and `Ctrl+C`), the header says whom you follow and the keys row tells its last calls. `F` again gives you the keys back and the screen as it was; where the agent took you stays in the jump list. `followAgent` (`, F`) starts it when an agent calls (#63)

## [1.31.0] - 2026-10-29

### Added
- A jump list: `Ctrl+O` or `Alt+←` goes back to where you were before a jump, `Alt+→` forward again. A view opened (`H` `T` `M`), `gg` / `G`, a search match, a mark or a scroll of more than half a page is a jump; where you land flashes, and the header shows `↶ 3/12` while there's somewhere to go back to. Also in the palette (#48)

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
- `L` with `"gitBeside"` on opens the git tool beside lazyrunner once: more presses while its pane starts are ignored, and while it runs in the project - its folder or its repo's top one, beside lazyrunner or in another window - the log says so instead of opening another (#13, #14, thanks @ognjen004028)

## [1.10.1] - 2026-10-26

### Changed
- `contributors.json` no longer counts commits: they changed with every push, so the list (and `main`) changed almost daily for nothing the About box shows. Code counts by merged pull requests

## [1.10.0] - 2026-10-26

### Added
- The About box's contributors come from the repo: who wrote code (merged pull requests, commits) and who reported bugs and ideas, with what each did ("1 PR · 2 bug reports · 7 ideas"). A GitHub workflow keeps `contributors.json` up to date; lazyrunner fetches it at most once a day as it checks for updates and keeps the one built in for offline
- `p` in the About box opens everyone who contributed, on GitHub

## [1.9.1] - 2026-10-26

### Fixed
- A link's letter in the About box highlights that link before opening it, like the `,` and `x` menus, so `Enter` and `j` / `k` go on from it (#7, #12, thanks @ognjen004028)

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
- The About box's "Report a bug" and "Suggest a feature" open the repository's bug and feature forms (the bug form's version and operating system filled in), not a blank issue (#5, thanks @ognjen004028)

### Changed
- A letter in the , menu (and the x menu) highlights the item it runs, so a toggled setting is the one in view (#4, thanks @ognjen004028)
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
