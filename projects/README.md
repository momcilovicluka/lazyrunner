# Project profiles

A **profile** tells lazyrunner what a project is: the file that marks its repo, the folder whose changes need a rebuild,
and the apps it runs - .NET APIs and npm frontends, side by side as columns. Everything about *how* lazyrunner runs them
(building, ports, readiness, logs) is in its code; the profile only says *what*. One JSON file per project.

This guide is for people and for LLMs alike: "write a lazyrunner profile for this repo" is a task an agent can do with it,
using `lazyrunner profile init` and `lazyrunner profile check` (below).

## Where lazyrunner looks

`lazyrunner` walks up from the current folder. In each folder it checks, in this order:

1. **The repo's own `.lazyrunner.json`** - versioned with the code, so it can differ per branch. It needs no `detect`
   (the file marks the repo). It can be a whole profile, or just changes to a profile lazyrunner knows:

   ```json
   { "schemaVersion": 1, "extends": "shop", "name": "Shop (team)", "apps": [{ "key": "web", "name": "Storefront" }] }
   ```

   With `extends`, each top-level field replaces the base profile's, and `apps` merge by `key`: an app's fields
   replace that app's, a new key adds an app at the end.
2. **Your profiles**, `projects/<id>.json` in lazyrunner's settings folder (`%APPDATA%\lazyrunner` on Windows,
   `~/.config/lazyrunner` on Linux, `~/Library/Application Support/lazyrunner` on macOS): for projects whose repo has
   no profile, without committing anything there. One with the id of a bundled profile replaces it.
3. **The bundled ones**, `projects/<id>.json` next to the lazyrunner binary, matched by their `detect.anyFile`.

The first folder where one of these matches is the repo root. Outside any project, `repoRoot` from
`lazyrunner.config.json` is used.

**Trust.** A repo's `.lazyrunner.json` says what lazyrunner runs, and a clone or a branch can bring one you haven't seen.
The first time, and whenever the file changes, `lazyrunner` lists what it would run and asks before using it; the answer is
kept per file (by its hash) in `%LOCALAPPDATA%\lazyrunner\trusted-profiles.json`. Your own and the bundled profiles need no
trust.

**On a branch change** lazyrunner reads the profile again. If it changed, the log says how (`+ Wiki, ~ Store`) and
lazyrunner keeps running with the one it has; `,` then `p` reopens it with the new one (asking for trust first if it
needs it). The same happens when `lazyrunner start` runs in another project's checkout while lazyrunner is open: it reopens
there.

**Per project:** the settings that depend on the project (`columnWidths`, `apps`, `frontend`, `stopFrontendOnStop`)
live under `projects.<id>` in `lazyrunner.config.json`, and its state (build stamps, logs, timings) under
`%LOCALAPPDATA%\lazyrunner\<id>\`.

## A minimal profile

One ASP.NET API and one Vite frontend:

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

- `schemaVersion`, `id`, `name`: the format version, a short id (it names the project's settings and state), and the
  name shown in messages.
- `watch`: the folder with the .NET code. A change there means the APIs need a rebuild; nothing changed, and the
  build is skipped.
- The **API** is built with `dotnet build` and run with `dotnet run --no-build --launch-profile Api`. The port comes
  from that launch profile's `applicationUrl` in `services/Api/Properties/launchSettings.json`. lazyrunner probes
  `/swagger/index.html` for the response time, and `w` opens Swagger.
- The **frontend** runs `npm run dev` in `web/` and is up once port 5173 listens.

Saved as `.lazyrunner.json` at the repo root, that's all lazyrunner needs. (Saved in your profiles folder instead, it also
needs `"detect": { "anyFile": ["Shop.sln"] }`: a file only this repo has.)

## Field reference

`profile.schema.json` is the source of these tables; editors that read `$schema` (VS Code does) check a profile and
complete it as you type. Paths are relative to the repo root; `/` and `\` both work.

<!-- reference: made from profile.schema.json (LAZYRUNNER_WRITE_REFERENCE=1 cargo test -p lazyrunner-core reference) - change the schema, not this -->

#### The profile

What a project is for lazyrunner: the file that marks its repo, the folder whose changes need a rebuild, and the apps it runs. Schema version 1 (projects/README.md). In a repo's .lazyrunner.json, detect can be left out, and with extends only the changes are needed.

| Field | Type | Required | What it does |
|---|---|---|---|
| `$schema` | string |  | Lets editors check the file; lazyrunner ignores it. |
| `schemaVersion` | `1` | yes | The profile format version. lazyrunner refuses a version it doesn't know. |
| `extends` | string |  | Only in a repo's .lazyrunner.json: the id of a known profile this file changes. Its top-level fields replace that profile's; apps merge by key. Example: `"shop"`. |
| `id` | string | yes | Short lowercase name. Names the project's settings in lazyrunner.config.json and its state folder. Example: `"shop"`. |
| `name` | string | yes | Shown in messages and notifications. Example: `"Shop"`. |
| `detect` | object |  | How lazyrunner recognises the repo. Required, except in a repo's .lazyrunner.json (that file marks the repo). |
| `detect.anyFile` | array of paths | yes | A file only this project's repo has, relative to its root. lazyrunner looks for it upward from the current folder. One file for now. Example: `["Shop.sln"]`. |
| `watch` | array of paths | yes | The folder whose changes need a rebuild of the .NET apps (unchanged builds are skipped; restart on code change). One folder for now. Example: `["services"]`. |
| `apps` | array | yes | The apps lazyrunner runs, left to right as its columns: .NET apps, npm apps and command apps (the tables below). |
| `tasks` | array of objects |  | Optional. One-off commands run from the palette or by an agent (the task table below). Example: `[{"key":"seed","name":"Seed the database","run":"python seed.py"}]`. |
| `presets` | object |  | Optional. This profile's own presets, by name: a command app's "preset" finds them before yours and lazyrunner's (the preset table below). Example: `{"fastapi":{"run":"uvicorn main:app --port {port}","install":"uv sync"}}`. |
| `layers` | object |  | Optional. Rules for the code map (K, then Tab): each key a project's name (* for anything, e.g. *.Api), and the projects it must not reference - a link that does is marked as a rule break. Nx's module boundaries (.eslintrc.json's depConstraints) are read as well. Example: `{"Orders.Core":{"mustNotReference":["Orders.Api"]}}`. |

#### A .NET app ("type": "dotnet")

An ASP.NET (or any dotnet run) project. lazyrunner builds it with dotnet build, one build at a time and skipped when nothing in watch changed, then runs it with dotnet run --no-build --launch-profile.

| Field | Type | Required | What it does |
|---|---|---|---|
| `key` | app key | yes | Short name, unique in the profile. Used in settings (columnWidths, apps.<key>) and by servedBy. Example: `"orders"`. |
| `name` | string | yes | The column's title. Example: `"Orders"`. |
| `icon` | character |  | One character before the app's name in its title bar. Default: `■`. Example: `"◆"`. |
| `nerdIcon` | character |  | Optional. Its icon in the title bar when Nerd Font icons are on (nerdIcons): one Nerd Font character. Without it .NET's, or the framework's in package.json (Angular, React, Vue) else Node.js's, as a JSON escape (\uf0ac, a globe) or the character itself. |
| `type` | `"dotnet"` | yes | The app type: what lazyrunner does with the app follows from it. |
| `project` | path | yes | The .csproj to build and run. Example: `"services/Orders/Orders.csproj"`. |
| `launchProfile` | string | yes | A profile in the project's Properties/launchSettings.json. Its applicationUrl gives the port. Example: `"Orders"`. |
| `swagger` | string |  | Optional. The Swagger UI path: lazyrunner probes <swagger>/index.html for the response time, and w opens it. Without it lazyrunner probes /. Example: `"/swagger"`. |
| `args` | array of strings |  | Optional. What dotnet run passes the app besides lazyrunner's own, after --: configuration for local runs, say. An app that logs through Serilog to Seq only prints nothing for the requests view (H); an empty Seq key (--Seq:ApiKey=) brings its console log back, when its code reads it that way. Example: `["--Seq:ApiKey="]`. |
| `env` | object |  | Optional. Environment variables the app gets on top of lazyrunner's own (x then e shows them). Example: `{"ASPNETCORE_ENVIRONMENT":"Development"}`. |
| `tests` | array of strings |  | Optional. Test suites that belong to this app in the tests view (T), whatever lazyrunner works out: .NET test projects by path from the repo root or by name, Nx projects by name. Without it a .NET test project belongs to the app whose project it references, an npm or Nx suite to the npm app whose folder it runs in. Example: `["tests/Orders.Tests/Orders.Tests.csproj"]`. |
| `dependsOn` | array of app keys |  | Optional. The apps it calls, by key: the stack map (K) draws these edges dashed until a call is seen (a .NET app's own calls are seen in its log without this). Example: `["billing"]`. |
| `migrations` | object |  | Optional. Where its EF Core migrations are, for the migrations view (M), when lazyrunner shouldn't find them itself (the model snapshots in the app's project and the projects it builds). dotnet ef runs with this app's project as the startup project (its Development settings, the database picked with d) and its last build. |
| `migrations.project` | path |  | The project with the migrations, when it isn't the app's own. Example: `"src/Shop.Data/Shop.Data.csproj"`. |
| `migrations.contexts` | array of strings |  | Its DbContexts. Without them, those of the model snapshots in project, else what dotnet ef dbcontext list finds. Example: `["ShopDbContext"]`. |
| `health` | object |  | Optional, on a .NET, npm or command app. What the Stack screen's Health tab (K) holds the app to, and what its response-time probe asks for. A reading over its target for a minute is an alert - the log says so, the app's title flashes and a notification goes out - until it recovers; a on the Health tab acknowledges it. Without it: p95 1000 ms, 5% 5xx, 2048 MB, 90% CPU. |
| `health.p95Ms` | number |  | Optional. The p95 latency over the last minute, in milliseconds (a .NET app whose requests are logged). Default 1000. Example: `500`. |
| `health.errorRate` | number |  | Optional. The share of 5xx answers over the last minute, judged from 10 requests on (a .NET app whose requests are logged). Default 0.05. Example: `0.02`. |
| `health.memoryMb` | number |  | Optional. Memory of the app's whole process tree, in MB. Default 2048. Example: `1500`. |
| `health.cpuPercent` | number |  | Optional. CPU of the app's whole process tree, as a percent of the machine. Default 90. Example: `80`. |
| `health.path` | string |  | Optional. The app's own health endpoint: the response-time probe asks for it instead of the Swagger page (or /). Example: `"/healthz"`. |
| `health.expectedStatus` | integer |  | Optional. The status the probe expects; any other counts as not responding. Without it any answer counts. Example: `200`. |

#### An npm app ("type": "npm")

A folder with package.json, run with npm run <script>. It needs no build from lazyrunner; i runs npm ci for it.

| Field | Type | Required | What it does |
|---|---|---|---|
| `key` | app key | yes | Short name, unique in the profile. Example: `"shop"`. |
| `name` | string | yes | The column's title. Example: `"Shop"`. |
| `icon` | character |  | One character before the app's name in its title bar. Default: `■`. Example: `"◆"`. |
| `nerdIcon` | character |  | Optional. Its icon in the title bar when Nerd Font icons are on (nerdIcons): one Nerd Font character. Without it .NET's, or the framework's in package.json (Angular, React, Vue) else Node.js's, as a JSON escape (\uf0ac, a globe) or the character itself. |
| `type` | `"npm"` | yes | The app type: what lazyrunner does with the app follows from it. |
| `dir` | path | yes | The folder with package.json. Example: `"web/shop"`. |
| `variants` | array of objects | yes | How it runs, chosen again on every start: the first variant whose when.packageScript package.json has. One without when always applies, so only the last can leave it out. Example: `[{"script":"dev","port":5173}]`. |
| `env` | object |  | Optional. Environment variables the app gets on top of lazyrunner's own (x then e shows them). Example: `{"VITE_API_URL":"http://localhost:5001"}`. |
| `tests` | array of strings |  | Optional. Test suites that belong to this app in the tests view (T), whatever lazyrunner works out: .NET test projects by path from the repo root or by name, Nx projects by name. Without it a .NET test project belongs to the app whose project it references, an npm or Nx suite to the npm app whose folder it runs in. Example: `["admin","shared-ui"]`. |
| `dependsOn` | array of app keys |  | Optional. The apps it calls, by key: the stack map (K) draws these edges dashed until a call is seen (a .NET app's own calls are seen in its log without this). Example: `["orders"]`. |
| `health` | object |  | Optional, on a .NET, npm or command app. What the Stack screen's Health tab (K) holds the app to, and what its response-time probe asks for. A reading over its target for a minute is an alert - the log says so, the app's title flashes and a notification goes out - until it recovers; a on the Health tab acknowledges it. Without it: p95 1000 ms, 5% 5xx, 2048 MB, 90% CPU. |
| `health.p95Ms` | number |  | Optional. The p95 latency over the last minute, in milliseconds (a .NET app whose requests are logged). Default 1000. Example: `500`. |
| `health.errorRate` | number |  | Optional. The share of 5xx answers over the last minute, judged from 10 requests on (a .NET app whose requests are logged). Default 0.05. Example: `0.02`. |
| `health.memoryMb` | number |  | Optional. Memory of the app's whole process tree, in MB. Default 2048. Example: `1500`. |
| `health.cpuPercent` | number |  | Optional. CPU of the app's whole process tree, as a percent of the machine. Default 90. Example: `80`. |
| `health.path` | string |  | Optional. The app's own health endpoint: the response-time probe asks for it instead of the Swagger page (or /). Example: `"/healthz"`. |
| `health.expectedStatus` | integer |  | Optional. The status the probe expects; any other counts as not responding. Without it any answer counts. Example: `200`. |

#### A dev-server variant

The script starts a server on a port (vite, ng serve, next dev …). The app is up once the port listens.

| Field | Type | Required | What it does |
|---|---|---|---|
| `label` | string |  | Shown in the column next to the script. Default: `dev server`. Example: `"mock API"`. |
| `when` | object |  | When the variant applies; without it, always. |
| `when.packageScript` | string | yes | The variant applies on a checkout whose package.json has this script (so it can differ per branch). Example: `"dev:mock"`. |
| `script` | string | yes | The npm script to run. Example: `"dev"`. |
| `port` | integer | yes | The dev server's port, unless the script itself says --port <n>. Example: `5173`. |

#### A watch-build variant

The script builds and keeps watching, and a .NET app serves the output. The app is up once ready.file is written.

| Field | Type | Required | What it does |
|---|---|---|---|
| `label` | string |  | Shown in the column next to the script. Default: `watch build`. Example: `"served by Orders"`. |
| `when` | object |  | When the variant applies; without it, always. |
| `when.packageScript` | string | yes | The variant applies on a checkout whose package.json has this script (so it can differ per branch). Example: `"dev:mock"`. |
| `script` | string | yes | The npm script to run. Example: `"watch"`. |
| `servedBy` | app key | yes | The key of the .NET app that serves the build: w opens the app on its port. Example: `"orders"`. |
| `ready` | object | yes | When the build is done. |
| `ready.file` | path | yes | A file the build writes, relative to dir: the app is ready once it is newer than the start. Example: `"dist/index.html"`. |

#### A command app ("type": "command")

Any long-running command - a Python, Go or Rust service, a queue emulator, a script - with what .NET and npm apps get, each optional: a build before it starts, an install (i), when it counts as up, what counts as its code (restart on code change), and what its errors and stack frames look like. Stopping takes its whole process tree down.

| Field | Type | Required | What it does |
|---|---|---|---|
| `key` | app key | yes | Short name, unique in the profile. Example: `"pricing"`. |
| `name` | string | yes | The column's title. Example: `"Pricing"`. |
| `icon` | character |  | One character before the app's name in its title bar. Default: `■`. Example: `"◆"`. |
| `nerdIcon` | character |  | Optional. Its icon in the title bar when Nerd Font icons are on (nerdIcons): one Nerd Font character. Without it .NET's, or the framework's in package.json (Angular, React, Vue) else Node.js's, as a JSON escape (\uf0ac, a globe) or the character itself. |
| `type` | `"command"` | yes | The app type: what lazyrunner does with the app follows from it. |
| `run` | text or list |  | The long-running command; required unless its preset knows it for this folder. Example: `"uvicorn main:app --port {port}"`. |
| `preset` | string |  | Optional. A preset that fills in each field this app leaves out - by what's in its folder: python (its install by uv.lock or requirements.txt, its .venv on PATH, its errors and stack frames, *.py as its code), go (go mod download, go build, go run . by a main.go) or rust (cargo build, cargo run by a src/main.rs). Example: `"python"`. |
| `path` | array of strings |  | Optional. Folders from cwd (or from your home folder, starting ~/) put first on PATH when they exist, and where its programs are looked for first (a .venv's Scripts, ~/.cargo/bin). Example: `[".venv/Scripts",".venv/bin"]`. |
| `tools` | array of strings |  | Optional. Programs the startup checklist shows with their versions. Example: `["python","uv"]`. |
| `cwd` | path |  | Optional. Where it runs, from the repo root (the root without it). Example: `"services/pricing"`. |
| `port` | integer |  | Optional. Its port: {port} in run, build, install and env is replaced with it, and the ports view (P) and port conflicts see it. Without ready it is up once the port listens. Example: `8000`. |
| `build` | text or list |  | Optional. Runs before it starts and on b, one build at a time; skipped when nothing in cwd changed since its last good build (as git sees the folder). Example: `"cargo build"`. |
| `install` | text or list |  | Optional. Installs its packages, as npm ci does an npm app's: i runs it. Example: `"uv sync"`. |
| `setup` |  |  | Optional. Runs before i's install while none of its own path folders is there - it makes them ([] for none). Example: `"python -m venv .venv"`. |
| `installFrom` | array of strings |  | Optional. The files its install installs from, from cwd: when they change since lazyrunner's last install, the startup checklist and the panel say it's due. Example: `["uv.lock","pyproject.toml"]`. |
| `ready` | object |  | Optional. When it counts as up: one of port, line or http. Without it, once its port listens - or once it has started, when it has no port. |
| `ready.port` | integer |  | Up once this port listens. Example: `8000`. |
| `ready.line` | string |  | Up once a line of its output matches this regular expression. Example: `"Uvicorn running on"`. |
| `ready.orPort` | boolean |  | With line: up once the line shows or its port listens, whichever comes first. |
| `ready.http` | string |  | Up once this path on its port answers with a 2xx (it needs port). Example: `"/health"`. |
| `watch` | array of strings |  | Optional. Globs of its code, from cwd (** any folders, * and ? within a name): with restart on code change, saving one restarts it - built first when it builds. .git, .venv, venv, node_modules, target, __pycache__, bin, obj, dist and build are never watched. Example: `["**/*.py"]`. |
| `errors` | array of strings |  | Optional. Regular expressions of an error line: shown red and counted as an error (the title flashes). Example: `["^ERROR","Traceback"]`. |
| `warnings` | array of strings |  | Optional. Regular expressions of a warning line: shown yellow and counted as a warning. Example: `["^WARNING"]`. |
| `frames` | string |  | Optional. A regular expression of a stack frame with (?P<file>...) and (?P<line>...) groups, and (?P<col>...) when it says: the file becomes a link that opens the editor at its line. Example: `"File \"(?P<file>[^\"]+)\", line (?P<line>\\d+)"`. |
| `env` | object |  | Optional. Environment variables the app gets on top of lazyrunner's own (x then e shows them); {port} in a value is its port, {cwd} its folder. Example: `{"APP_ENV":"dev"}`. |
| `dependsOn` | array of app keys |  | Optional. The apps it calls, by key: the stack map (K) draws these edges dashed. Example: `["orders"]`. |
| `health` | object |  | Optional, on a .NET, npm or command app. What the Stack screen's Health tab (K) holds the app to, and what its response-time probe asks for. A reading over its target for a minute is an alert - the log says so, the app's title flashes and a notification goes out - until it recovers; a on the Health tab acknowledges it. Without it: p95 1000 ms, 5% 5xx, 2048 MB, 90% CPU. |
| `health.p95Ms` | number |  | Optional. The p95 latency over the last minute, in milliseconds (a .NET app whose requests are logged). Default 1000. Example: `500`. |
| `health.errorRate` | number |  | Optional. The share of 5xx answers over the last minute, judged from 10 requests on (a .NET app whose requests are logged). Default 0.05. Example: `0.02`. |
| `health.memoryMb` | number |  | Optional. Memory of the app's whole process tree, in MB. Default 2048. Example: `1500`. |
| `health.cpuPercent` | number |  | Optional. CPU of the app's whole process tree, as a percent of the machine. Default 90. Example: `80`. |
| `health.path` | string |  | Optional. The app's own health endpoint: the response-time probe asks for it instead of the Swagger page (or /). Example: `"/healthz"`. |
| `health.expectedStatus` | integer |  | Optional. The status the probe expects; any other counts as not responding. Without it any answer counts. Example: `200`. |

#### A compose app ("type": "compose")

Docker Compose services as one app: starting it runs docker compose up for them, its column shows their output and its title each one's state (● up, ◌ starting, ✗ down, ✓ done), and it is up once each runs - healthy, when it has a healthcheck. Stopping it stops them (docker compose stop); the startup checklist says whether Docker is running. The ports are Docker's: a compose app has none of its own.

| Field | Type | Required | What it does |
|---|---|---|---|
| `key` | app key | yes | Short name, unique in the profile. Example: `"services"`. |
| `name` | string | yes | The column's title. Example: `"Services"`. |
| `icon` | character |  | One character before the app's name in its title bar. Default: `■`. Example: `"◆"`. |
| `nerdIcon` | character |  | Optional. Its icon in the title bar when Nerd Font icons are on (nerdIcons): one Nerd Font character. Without it .NET's, or the framework's in package.json (Angular, React, Vue) else Node.js's, as a JSON escape (\uf0ac, a globe) or the character itself. |
| `type` | `"compose"` | yes | The app type: what lazyrunner does with the app follows from it. |
| `file` | path |  | Optional. The compose file, from cwd; without it Docker's own (compose.yaml, docker-compose.yml, with its override file). Example: `"docker-compose.dev.yml"`. |
| `cwd` | path |  | Optional. The folder of the compose file, from the repo root (the root without it). Example: `"ops"`. |
| `services` | array of strings |  | Optional. The services it runs; all of the file's without it. Example: `["db","redis"]`. |
| `env` | object |  | Optional. Environment variables docker compose gets on top of lazyrunner's own: the compose file's ${VARIABLES}. Example: `{"DB_PORT":"5433"}`. |
| `dependsOn` | array of app keys |  | Optional. The apps it calls, by key: the stack map (K) draws these edges dashed. Example: `["orders"]`. |

#### An app's health ("health")

Optional, on a .NET, npm or command app. What the Stack screen's Health tab (K) holds the app to, and what its response-time probe asks for. A reading over its target for a minute is an alert - the log says so, the app's title flashes and a notification goes out - until it recovers; a on the Health tab acknowledges it. Without it: p95 1000 ms, 5% 5xx, 2048 MB, 90% CPU.

| Field | Type | Required | What it does |
|---|---|---|---|
| `p95Ms` | number |  | Optional. The p95 latency over the last minute, in milliseconds (a .NET app whose requests are logged). Default 1000. Example: `500`. |
| `errorRate` | number |  | Optional. The share of 5xx answers over the last minute, judged from 10 requests on (a .NET app whose requests are logged). Default 0.05. Example: `0.02`. |
| `memoryMb` | number |  | Optional. Memory of the app's whole process tree, in MB. Default 2048. Example: `1500`. |
| `cpuPercent` | number |  | Optional. CPU of the app's whole process tree, as a percent of the machine. Default 90. Example: `80`. |
| `path` | string |  | Optional. The app's own health endpoint: the response-time probe asks for it instead of the Swagger page (or /). Example: `"/healthz"`. |
| `expectedStatus` | integer |  | Optional. The status the probe expects; any other counts as not responding. Without it any answer counts. Example: `200`. |

#### A preset ("presets")

What a language needs to run as a command app ("preset" names it): each field the app leaves out comes from the first of the preset's options that holds in the app's folder and has it, else from the preset (env merged, the app's own on top). lazyrunner's own are in projects/presets (python, go, rust); a presets folder beside your profiles and a profile's "presets" add to them or replace them.

| Field | Type | Required | What it does |
|---|---|---|---|
| `id` | string |  | Its name, as "preset" says it (a profile's own presets go by their key). Example: `"python"`. |
| `name` | string |  | The language, in messages. Example: `"Python"`. |
| `nerdIcon` | character |  | Optional. Its icon in the title bar when Nerd Font icons are on (nerdIcons): one Nerd Font character. Without it .NET's, or the framework's in package.json (Angular, React, Vue) else Node.js's, as a JSON escape (\uf0ac, a globe) or the character itself. |
| `detect` | object |  | Optional. The files that mark a folder as this language's: profile check suggests the preset for a command app there, and profile init adds one. |
| `detect.anyFile` | array of strings |  | Example: `["pyproject.toml"]`. |
| `tools` | array of strings |  | Optional. Programs the startup checklist shows with their versions. Example: `["python","uv"]`. |
| `path` | array of strings |  | Optional. Folders from the app's cwd put first on PATH when they exist, and where its programs are looked for first. Example: `[".venv/Scripts",".venv/bin"]`. |
| `env` | object |  | Optional. Variables for the app, under its own; {cwd} is its folder, {port} its port. Example: `{"PYTHONUNBUFFERED":"1"}`. |
| `run` | text or list |  | Optional. What runs it. |
| `build` | text or list |  | Optional. Its build. |
| `install` | text or list |  | Optional. Its install. |
| `setup` |  |  | Optional. What makes its path folder before its first install. |
| `installFrom` | array of strings |  | Optional. The files its install installs from. |
| `ready` | object |  | Optional. When it counts as up, as a command app's ready. |
| `ready.port` | integer |  | Up once this port listens. |
| `ready.line` | string |  | Up once a line of its output matches this regular expression. |
| `ready.orPort` | boolean |  | With line: or once its port listens. |
| `ready.http` | string |  | Up once this path on its port answers with a 2xx. |
| `watch` | array of strings |  | Optional. Globs of its code. Example: `["**/*.py"]`. |
| `errors` | array of strings |  | Optional. Regular expressions of an error line. |
| `warnings` | array of strings |  | Optional. Regular expressions of a warning line. |
| `frames` | string |  | Optional. A regular expression of a stack frame, with file and line groups. |
| `options` | array of objects |  | Optional. What depends on the folder: each option's fields apply when its when holds - one of its anyFile in the app's folder, its program on PATH - the first option with a field winning over the later ones and the preset's own. Example: `[{"when":{"anyFile":["uv.lock"]},"install":"uv sync"}]`. |

#### A task ("tasks")

A one-off command run from the palette (: then its name) or by an agent (run_task) - seed the database, regenerate a client - in the background, beside the apps. Its output goes to the panel's log between a rule with its name and its result: how long it took against its usual time, and its first error line when it failed.

| Field | Type | Required | What it does |
|---|---|---|---|
| `key` | app key | yes | Short name, unique among the tasks. Example: `"seed"`. |
| `name` | string | yes | What the palette calls it. Example: `"Seed the database"`. |
| `run` | text or list | yes | The command, as a command app's run. Example: `"python seed.py"`. |
| `cwd` | path |  | Optional. Where it runs, from the repo root (the root without it). Example: `"tools"`. |
| `env` | object |  | Optional. Variables it gets on top of lazyrunner's own; {cwd} is its folder. Example: `{"SEED":"demo"}`. |

<!-- end of reference -->

## Recipes

**A .NET API.** Point `project` at the `.csproj` and `launchProfile` at a profile in its
`Properties/launchSettings.json` that has an `applicationUrl` (and `"commandName": "Project"`). Add `swagger` if it
has Swagger UI. Without a launch profile lazyrunner can't tell the port, so it can't run it.

**Several APIs.** One `dotnet` app each. They build one at a time, in the order of `apps`, each skipped when nothing
under `watch` changed since its last build; `watch` is the folder they all share.

**An npm dev server.** A variant with `script` and `port`. When the script itself says `--port 4300`, that wins over
`port`, so the profile can keep the usual one.

**A watch build that a .NET app serves.** For a frontend built into an API's `wwwroot` and served from there:

```json
{ "key": "admin", "name": "Admin UI", "type": "npm", "dir": "web/admin",
  "variants": [{ "script": "watch", "servedBy": "api", "ready": { "file": "dist/index.html" } }] }
```

It is up once `web/admin/dist/index.html` is newer than its start, and `w` opens the API's URL.

**Different on some branches.** Variants are tried in order on every start; the first whose `when.packageScript` is
in `package.json` runs. A branch that adds a `dev:mock` script gets the mock setup, every other branch the plain one:

```json
"variants": [
  { "label": "mock API", "when": { "packageScript": "dev:mock" }, "script": "dev:mock", "port": 4300 },
  { "script": "dev", "port": 4200 }
]
```

When a branch switch needs another variant, lazyrunner restarts the app; otherwise npm's own watcher picks up the change.

**Any other command.** A Python, Go or Rust service, a queue emulator, a script: a `command` app with its `run`, and
as much as it needs of the rest - a `port` (`{port}` in its commands and env), a `build` before it starts, an
`install` for `i`, when it's `ready`, the `watch` globs of its code, and patterns for its `errors`, `warnings` and
stack `frames`:

```json
{ "key": "pricing", "name": "Pricing", "type": "command", "cwd": "services/pricing",
  "run": "uvicorn main:app --port {port}", "port": 8000, "install": "uv sync",
  "ready": { "line": "Uvicorn running on" }, "watch": ["**/*.py"] }
```

**A Python service.** A command app with `"preset": "python"`: the preset fills in what it leaves out by what's in
its folder - `uv sync` or `pip install -r requirements.txt` for `i`, its `.venv` first on PATH, `*.py` as its code,
its tracebacks as errors and their frames as links, and a run for Django (`manage.py`), `main.py` or `app.py`:

```json
{ "key": "pricing", "name": "Pricing", "type": "command", "preset": "python", "cwd": "services/pricing",
  "run": "uvicorn main:app --port {port}", "port": 8000 }
```

**A one-off command.** Seeding the database, regenerating a client: a task, run from the palette (`:` then its name)
with its output in the panel's log and its result, or by an agent (`run_task`):

```json
"tasks": [{ "key": "seed", "name": "Seed the database", "run": "python seed.py", "cwd": "tools" }]
```

**A Go or Rust service.** `"preset": "go"` (`go mod download`, `go build ./...` before a start, `go run .` where
there's a `main.go`) or `"preset": "rust"` (`cargo build`, `cargo run` where there's a `src/main.rs`); either way
with its errors coloured, its frames as links and its code watched. Give it a `run` when it isn't the folder's main
(`["go", "run", "./cmd/api"]`, `"cargo run -p api"`).

**Your own preset.** A preset is the fields it fills in, plus `options` for what depends on the folder (the first
whose `when` holds - a file there, a program on PATH - wins). Put one in a `presets` folder beside your profiles
(`presets/go.json`), or in a profile's `"presets"` (the preset table below):

```json
"presets": {
  "go": { "build": "go build ./...", "watch": ["**/*.go"], "errors": ["^panic:", "level=error"],
          "options": [{ "when": { "anyFile": ["go.mod"] }, "install": "go mod download" }] }
}
```

**A team's changes to a profile.** A `.lazyrunner.json` with `extends` (see "Where lazyrunner looks") renames, adds or
changes apps without copying the whole profile.

**On one machine.** `lazyrunner.config.json` changes an app for you only:

```json
{ "projects": { "shop": { "apps": { "admin": { "enabled": false }, "web": { "includeInAll": false } } } } }
```

`enabled: false` leaves the app out (no column); `"frontend": false` does that for every npm app. `includeInAll:
false` keeps it running on Stop all / Restart all; npm apps follow `stopFrontendOnStop` (`,` then `s`) unless set.

**Not yet:** toggles, hooks before and after start, and migrations or tests for a command app.

## Writing a profile for a repo

The same steps for a person and for an LLM. Each says what to look for and which field it fills.

1. **Start from `lazyrunner profile init`** in the repo. It prints a first profile from what it finds (`--save` writes it to
   `.lazyrunner.json`), with notes on what it guessed and what it left out. The steps below check and complete it; if
   init finds nothing or fails, they are all you need.
2. **.NET apps** - `*.csproj` files with a `Properties/launchSettings.json`:
   - one `dotnet` app per project that has a launch profile with an `applicationUrl` and `"commandName": "Project"`;
     `launchProfile` is that profile's name
   - leave out test projects (they reference `Microsoft.NET.Test.Sdk`), and libraries and workers without launch
     settings (lazyrunner can't tell where they listen)
   - `swagger: "/swagger"` when the project references Swashbuckle or NSwag, or `Program.cs` calls `UseSwagger`
3. **npm apps** - `package.json` files outside `node_modules`, one `npm` app per folder (`dir`):
   - a `dev`, `start` or `serve` script: a dev-server variant with that `script` and its `port`
   - a build that keeps watching (`watch`, `build:watch` …) and whose output a .NET app serves - wherever the output
     goes, the API's `wwwroot` or the client's own `dist/`: a watch-build variant with that `script`, `servedBy` (the
     API's key) and `ready.file` (a file the build writes, relative to `dir`). The repo's README or the API's
     static-files setup says which app serves it.
   - `port`: from the script (`--port`, `-p`, `PORT=`), else the tool's config (`vite.config.*` `server.port`,
     `angular.json` `serve.options.port`, `.env` `PORT`), else the code the script runs (a `PORT` constant,
     `listen(4300)`), else the tool's default (Vite 5173, Angular 4200, webpack 8080)
   - scripts that only some branches have: a variant per script, with `when`
4. **`watch`** - the folder that holds the .NET code the apps build from: the common folder of the .NET projects and
   their libraries, without the tests (usually `src`); `.` if there is no .NET app.
5. **`detect`** - only for a profile kept outside the repo: a file only this repo has, usually its `.sln`.
6. **Names** - what the team calls each app; keys short and lowercase (`api`, `admin`; the folder's name is fine).
   Leave `$schema` out of a committed file: the path to the schema is different on every machine (map
   `.lazyrunner.json` to `projects/profile.schema.json` in your editor instead - VS Code: `json.schemas`).
7. **Check** - `lazyrunner profile check` until it says OK (the next section). Fix errors; read the warnings.
8. **Save** - `.lazyrunner.json` at the repo root (committed, shared with the team), or
   `%APPDATA%\lazyrunner\projects\<id>.json` (yours only; then `detect` is required).
9. **Run `lazyrunner`** in the repo. It shows what the profile runs and asks to trust it; the person answers.

**Rules for LLMs:**
- Use only the fields in the reference above; never invent one (`lazyrunner profile check` reports unknown fields).
- Relative paths with forward slashes; no drive letters, no machine-specific paths.
- No secrets, connection strings or tokens in a profile.
- Read the ports from the repo (step 3); don't guess when the repo says.
- Run `lazyrunner profile check` until it says OK, then show the profile to the person - before saving it, unless they
  asked you to save it.
- Never answer the trust question for the person, and never write to `trusted-profiles.json`: trusting a profile is
  how they approve what it runs.

## Checking a profile

```
lazyrunner profile check                    the profile lazyrunner uses here
lazyrunner profile check path\to\file.json  a given file (a draft, or one from your profiles folder)
lazyrunner profile check --json             the same, as JSON for tools: { file, source, root, ok, problems: [{ level, at, message }] }
```

Which checkout: a `.lazyrunner.json` is checked against its own folder; another file with `detect` against the checkout
that `detect` finds from where you run it; a draft without `detect` (or with `extends`) against the folder you run it
in.

It checks the fields (types, required ones, unknown ones, the rules between them), the schema, and the checkout:
that the projects, launch profiles, `package.json` files and scripts exist, that a variant applies on this branch,
and that no two apps want one port. Each problem names its field (`apps[1] (web).variants[0].script`). The exit code
is 1 when there are errors.

| Message | What to do |
|---|---|
| `… is not a profile field - did you mean …?` | A typo, or a field that doesn't exist: see the reference |
| `apps[0] (api).launchProfile: 'Api' is not in …launchSettings.json (it has: …)` | Use one of the names listed |
| `… has no applicationUrl, so lazyrunner can't tell its port` | Pick a launch profile with an `applicationUrl`, or add one |
| `… package.json has no script 'dev'` | Use a script that exists, or add `when` if only some branches have it |
| `no variant applies on this branch` | A warning: on this branch the app won't start. Expected if it only exists on some branches |
| `A and B both use port 4200 on this branch` | Give one of them another port (in the profile, or `--port` in its script) |
| `node_modules is missing` | Information: install the packages (`i` in lazyrunner runs `npm ci`) |
| `… is a test project` | A warning: lazyrunner runs apps; leave test projects out |
| `servedBy 'x' must be the key of a dotnet app` | `servedBy` names a .NET app's `key` |
| `extends 'x', which is not a known profile` | No profile with that id in your folder or `projects\` |

A broken profile also stops `lazyrunner` itself, with the same messages.

## Limits for now

- one marker file in `detect.anyFile`, one folder in `watch`
- app types are `dotnet`, `npm` and `command` (what each does is in `crates/lazyrunner-core/src/apptypes.rs`)
- the apps are columns side by side, focused with 1-9
- a watch build's page (`w`) opens at the root of the app that serves it; there is no field for a path under it

## Format versions

`schemaVersion` changes only when a profile written for the new version would be misread by an older lazyrunner; a
newer version stops an older lazyrunner with "needs a newer lazyrunner".

- **1** - the first: `detect`, `watch`, `extends`, `dotnet` and `npm` apps, dev-server and watch-build variants.

## Tests

`tests\profile.Tests.ps1` checks the profile loader, that no code names an app or compares an app type outside
`apptypes.ps1`, variants, `lazyrunner profile check` and `init`, and that the reference above matches the schema.
`tests\projects.Tests.ps1` covers where lazyrunner looks, `extends`, trust, and settings and state per project. The
`*.e2e.ps1` tests run lazyrunner itself. See `tests\README.md`.
