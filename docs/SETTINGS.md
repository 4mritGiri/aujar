# Settings and launcher features

Open the settings window with any of:

- type `aujar settings` in the launcher,
- `aujar launcher settings`.

Changes save immediately to `~/.config/aujar/settings.toml`. The launcher re-reads the file
each time it opens, and the daemon reloads its plugins when the file changes (no restart).

## What you can type in the launcher

| You type | You get |
| --- | --- |
| `fire` | Applications (with icons), then files and folders |
| `2*(3+4)`, `2^10` | Calculator answer card. Enter copies it |
| `10 km to mi`, `100 f to c`, `2 gb to mb` | Unit conversion (length, mass, volume, time, data, speed, area, temperature) |
| `kind:pdf`, `kind:pdf invoice`, `kind:image`, `kind:folder`, `ext:rs` | Filter files by type (newest first when you type no name). Also `document`, `audio`, `video`, `archive`, `code` |
| `kind:recent` | Your file history, from the desktop's recently-used list (the empty launcher shows it as "Recent Files") |
| `report` | Files and folders by name (Ctrl+Enter shows the file in its folder, Ctrl+C copies the path) |
| `lock`, `sleep`, `restart`, `shut down`, `log out` | System commands (restart, shut down and log out ask for a second Enter) |
| `> ls -la` | Run a shell command (off by default, enable in Settings -> Search plugins) |
| `github.com/rust-lang` | Open a URL |
| `g rust lifetimes`, `yt lofi`, `gh aujar`, `w linux`, `maps pokhara` | Web shortcuts |
| `? anything` | Search with your default engine |
| `100 usd to eur`, `$50 in npr` | Currency conversion (opt-in: downloads rates with `curl`; rates by [Exchange Rate API](https://www.exchangerate-api.com)) |
| `wifi off`, `bluetooth on`, `mute`, `dark mode` | More system commands (shown only when `nmcli`, `bluetoothctl`, `wpctl`/`pactl`, `gsettings` exist) |
| `rdp`, `rdp office`, `nexdesk office` | Your saved NexDesk connections; Enter opens NexDesk's password prompt for it (see [NEXDESK](NEXDESK.md)) |
| `clip`, `cb foo` | Clipboard history (opt-in, kept in memory only) |
| *(empty box)* | Recent and frequently used items |
| anything | A "Search the web" row at the bottom |

The right-hand preview pane shows details of the selected result: size and modified time
for files, the first lines of text files, image thumbnails, and clipboard text.

Clipboard history notes: on X11 (`xclip`) and wlroots/KDE Wayland (`wl-clipboard`) a background
watcher records every copy. GNOME's Wayland session forbids that, so there the history only
gains what is on the clipboard each time you open the launcher.

PDFs get a first-page thumbnail in the preview pane (needs `pdftoppm` from poppler-utils; cached in
`~/.cache/aujar/thumbs`).

Editing the query: Shift+Left/Right/Home/End select; Ctrl+Left/Right jump words and
Ctrl+Shift+Left/Right select words; Ctrl+A selects all; Ctrl+C/X/V copy, cut, paste (Ctrl+C with
nothing selected copies the selected result); Ctrl+Z / Ctrl+Shift+Z or Ctrl+Y undo and redo.
With the mouse: click to place the caret, drag to select, double-click a word, triple-click
everything. Ctrl+B searches the web for the query, Ctrl+, opens settings, Esc clears then closes.

Keys: Left/Right/Home/End/Delete edit the query (Ctrl+E end);
Up/Down/Tab/Shift+Tab or Ctrl+N/P/J/K move; Enter runs; Alt+1..9 runs that row;
Ctrl+U clears; Ctrl+W deletes a word; Ctrl+V pastes; Esc clears, then closes.

## Config file reference

```toml
[appearance]
theme = "dark"        # "dark" | "light"
accent = "blue"       # blue | purple | pink | orange | green | teal
width = 860           # 480..1000
max_results = 6       # 3..12
show_icons = true
show_footer = true
show_preview = true
show_recents = true
close_on_blur = true

[plugins]
applications = true
calculator = true
units = true
currency = false      # downloads exchange rates
clipboard = false     # records what you copy (memory only)
files = true
system = true
shell = false         # runs arbitrary commands
nexdesk = true        # `rdp name` lists saved NexDesk connections

[files]
roots = ["~"]         # directories to index
max_depth = 4

[web]
engine = "duckduckgo" # duckduckgo | google | brave | bing | startpage

[[web.shortcuts]]
keyword = "g"
name = "Google"
url = "https://www.google.com/search?q="
```
