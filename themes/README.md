# lazyrunner themes

Each `<id>.json` here is one theme, built into lazyrunner. Your own go in `themes/<id>.json` in lazyrunner's settings folder (`%APPDATA%\lazyrunner` on Windows, `~/.config/lazyrunner` on Linux, `~/Library/Application Support/lazyrunner` on macOS); one with the id of a built-in theme replaces it. `t` in lazyrunner cycles through them: Default comes first, then the rest by display name. The folder is re-read on every press, so a new or edited file shows up without restarting. A file that can't be used is skipped, and the panel log says why.

## Format

```json
{
  "name": "My Theme",
  "description": "Shown in the panel log when you switch to it",
  "extends": "dracula",
  "colors": { "Red": "#ff5555", "Green": "#50fa7b" },
  "foregrounds": { "Gray": "#e0e0e0" },
  "backgrounds": { "DarkGray": "#44475a" },
  "focus": { "background": "#bd93f9", "text": "#21222c", "border": "#ff79c6" },
  "terminal": { "background": "#282a36", "foreground": "#f8f8f2" }
}
```

Every field is optional.

| Field | Meaning |
|---|---|
| `name` | Display name. Defaults to the file name. |
| `description` | One line, shown in the panel log when you switch to it. |
| `extends` | Theme id to start from. Defaults to `default`; `null` starts from the terminal's own colours. Only the colours you list are changed. |
| `colors` | Sets the text and the background colour of a palette name. |
| `foregrounds` / `backgrounds` | Sets only the text, or only the background, colour. |
| `focus.background` / `focus.text` | The focused column's title bar. |
| `focus.border` | The heavy borders on both sides of the focused column. |
| `terminal.background` / `terminal.foreground` | Recolours the whole terminal while lazyrunner runs; restored on quit. Must be `#rrggbb`. Windows Terminal supports it; other terminals ignore it. |

**Palette names** (what lazyrunner draws with): `Black`, `DarkRed`, `DarkGreen`, `DarkYellow`, `DarkBlue`, `DarkMagenta`, `DarkCyan`, `Gray`, `DarkGray`, `Red`, `Green`, `Yellow`, `Blue`, `Magenta`, `Cyan`, `White`.
- `Gray` is normal text, `White` is emphasised text and `DarkGray` is dim text.
- `Black` is the text on coloured chips.
- The `Dark*` colours are the state colours (green running, yellow starting, red failed).

**Colour values:**
- `"#rrggbb"`: true colour.
- An SGR code such as `"91"`, `"1;93"` (bold) or `"38;5;208"` (256-colour), which uses your terminal's palette. As a background, `30`–`37` and `90`–`97` become `40`–`47` and `100`–`107`.
- A palette name such as `"Blue"`, which takes that colour from this theme.
