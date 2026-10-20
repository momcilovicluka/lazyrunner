# Changelog

What changed in each lazyrunner version, newest first. lazyrunner shows the new entries in its startup checklist after
it updates itself; `,` then `c` shows the whole file. How to release a version: README, "Releasing".

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
