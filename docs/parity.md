# Parity with the PowerShell version (0.1)

The Rust screen is a port of the PowerShell 0.1 UI (`lib\view.ps1`, `lib\input.ps1`, `lib\actions.ps1`,
`lib\checklist.ps1`, `lib\themes.ps1`). Each item below was checked against the PowerShell source. The snapshot tests
(`crates/lazyrunner-ui/src/screen/snapshots`) pin each screen, and the end-to-end tests (`crates/lazyrunner/tests`)
press the keys against real apps.

✓ = same behaviour and the same text · ≈ = a deliberate difference (listed at the end)

## Keys

| Key | Does | |
|---|---|---|
| `Space` | start / stop the focused app (also cancels its build, queue place or npm ci) | ✓ |
| `u` `U` | start · focused / all (a start that builds shows "Checking for source changes…" first) | ✓ |
| `s` `S` | stop · focused / all; `S` while building cancels the build and this start's apps | ✓ |
| `r` `R` | restart · focused / all | ✓ |
| `b` `B` | rebuild · focused / all | ✓ |
| `!` | kill whatever holds a needed port (only when there is a conflict) | ✓ |
| `q` `Q` | quit (also while building) | ✓ |
| `d` | database picker | ✓ |
| `i` | npm ci: the app whose lock changed (focused first), else the focused npm app | ✓ |
| `w` `W` | open · focused (Swagger for an API) / everything | ✓ |
| `o` | open the focused app's log (the failed build's log after a failed build) | ✓ |
| `x` | menu for the focused app | ✓ |
| `/` then `n` `N` `Esc` | search the focused column, older / newer match, clear ("wrapped", "no match") | ✓ |
| `z` | zoom | ✓ |
| `>` `<` `=` | widen / narrow (5..90, saved per project) / equal widths | ✓ |
| `Ctrl+L` | clear the output and the log | ✓ |
| `t` | next theme (the folder is read again) | ≈ hint text |
| `,` | settings | ✓ |
| `?` | key reference | ✓ |
| `:` `Ctrl+P` | command palette | ✓ |
| `h` `l` `Tab` `Shift+Tab` `←` `→` `1`-`9` | focus another column | ✓ |
| `j` `k` `↑` `↓` `Ctrl+U` `Ctrl+D` `Ctrl+B` `Ctrl+F` `PgUp` `PgDn` `Home` `End` | scroll (a page = height − 16, at least 2) | ✓ |
| `gg` `G` | oldest line (the second `g` within 600 ms) / follow new output | ✓ |
| `Ctrl+C` | quit from anywhere, overlays included | ✓ |
| unsafe keys while building | "Building - press S to cancel it first, then: …" | ✓ |

## Overlays

| Overlay | Keys inside, title, footer, size | |
|---|---|---|
| `:` palette | type, `↑` `↓` `Ctrl+N` `Ctrl+P`, `Enter`, `Esc`; fuzzy score, settings shown as `, a`; "▸ label" logged | ✓ |
| `?` key reference | whole screen, columns fitted to the height, any key closes | ✓ |
| `x` menu | the item's key, `j` `k`, `Enter` / `Space` run the highlighted one, `Esc` `q` `x` close | ✓ |
| `,` settings | a u s e w v toggles ("… : on (saved)"), r checklist, c changelog, p reload profile; stays open | ✓ |
| `d` picker | `j` `k`, pages, `Home` `G` `End`, `h` `l` between keys, `/` filter, `n` new database, `Enter` switch + restart the APIs | ✓ |
| changelog | `j` `k`, pages, `g` `G`, `Esc` `q` `c`; "x-y of n · j k scroll · Esc closes" | ✓ (PowerShell's has a bug, see below) |
| startup checklist | the checks, what's new, the logo sweep; holds 60 / 20 / 8 / 1.2 s; any key continues | ✓ |

## States

| Where | What | |
|---|---|---|
| column title | chip, port, building with its bar and n/total, npm ci (and "npm ci → start"), starting dot + bar + "/ ~eta", "● … ms", "slow x.xs", "HTTP nnn", "not responding", "checking…", "● built, watching", "up in", "✗ exited (code n)", "✗ error", "○ stopped" | ✓ |
| info line | "⏸ ↑n · G follows", cpu / mem / ↗ / resp / req sparklines, ✗ and △ counts, the npm variant | ✓ |
| panel header | lazyrunner chip, summary, ⎇ branch, DB, folder; the theme's name on the rule | ✓ |
| notice line | the search prompt or match, port conflicts, "Checking for source changes…", a changed package-lock.json | ✓ |
| summary (header and tab) | "building X…" + "npm ci…", "something failed - see the log", "starting (a/b up)…", "up, but slow / not responding", "● all up (n)", "○ stopped" | ✓ |
| too small | "lazyrunner: make the terminal a bit bigger (60x15 at least)" | ✓ |

## Animations

All of these step on PowerShell's 100 ms frame count, whatever the frame rate.

| Animation | |
|---|---|
| braille spinner, orbit spinner (npm ci), pulsing dot, blinking cursors | ✓ |
| progress bars glide forward (never back) with a ▓ glint running along them | ✓ |
| new lines fade in (white, then bright, then their colour over a second) | ✓ |
| border colours per state; the focused column's borders heavy | ✓ |
| the error flash on the title chip (an error line 1.5 s, an exit 3 s) | ✓ |
| the chip shimmers while building; "✦ ready in … ✦" for 4 s | ✓ |
| the logo's light sweep and ✦✧ in the checklist | ✓ |
| the startup timeline in the log after "Ready in …" (█ building, ▒ starting, · up) | ✓ |

## Outside the frame

| What | |
|---|---|
| themes: `t` cycles, default first then by name, `extends`, focus colours, terminal colours (OSC 10/11, put back with 110/111) | ✓ |
| `NO_COLOR`: no colours, backgrounds as reverse video, " no-color " on the rule | ✓ |
| the tab's title "lazyrunner · db · summary" and its progress (OSC 9;4); the window title put back on exit | ✓ |
| desktop notifications: build failed, npm ci failed, "X stopped unexpectedly", "P is ready" (after 20 s or more); `notifications` setting | ✓ |
| `lazyrunner start` from another worktree of the same project switches in place ("Switching to …"); another project reopens | ✓ |
| `, p` reloads the project profile (reopens, starting what ran) | ✓ |

## Deliberate differences

- **Themes are built in.** Your own go in `%APPDATA%\lazyrunner\themes` (next to the settings), not the install's
  `themes\` folder, so the `t` hint says "n themes - add your own .json".
- **"No changelog in this lazyrunner."** The changelog is built into the binary, so there is no file path to name.
- **"✗ exited (code ?)"** when the exit code is unknown. PowerShell printed "(code )".
- **New messages:** "✗ Couldn't open …" when the browser or editor won't start; "Control channel … unavailable";
  "✗ Couldn't reopen …: … - kept …" when `, p` or another project fails to load (PowerShell exited).
- **The screen stays live during the checklist.** PowerShell's checklist blocked the loop. Here the output keeps
  arriving, and any key skips the checks still running. A tool that hangs times out after 10 s.
- **A search match is centred by rows.** PowerShell counted lines, so a match among long, wrapped lines could land
  off-screen.
- **Lines that rewrite themselves with `\r`** (progress bars) keep what a terminal would show. Other control
  characters are dropped.

## Not in P3

- The update check and self-update (the checklist's update row, `v`'s effect, "lazyrunner updated to …" on reopen)
  come in P4. The `v` setting already toggles and is saved.

## PowerShell bugs not copied

- **The changelog view ignores keys.** In `actions.ps1`, `Invoke-ChangelogKey $overlay $name` runs before `$name` is
  set, so the view can't be scrolled or closed from the keyboard. The port does what `update.ps1` documents.
