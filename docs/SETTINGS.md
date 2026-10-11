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
| `report` | Files and folders by name (Ctrl+Enter shows the file in its folder, Ctrl+C copies the path) |
| `lock`, `sleep`, `restart`, `shut down`, `log out` | System commands (restart, shut down and log out ask for a second Enter) |
| `> ls -la` | Run a shell command (off by default, enable in Settings -> Search plugins) |
| `github.com/rust-lang` | Open a URL |
| `g rust lifetimes`, `yt lofi`, `gh aujar`, `w linux`, `maps pokhara` | Web shortcuts |
| `? anything` | Search with your default engine |
| `100 usd to eur`, `$50 in npr` | Currency conversion (opt-in: downloads rates with `curl`; rates by [Exchange Rate API](https://www.exchangerate-api.com)) |
| `wifi off`, `bluetooth on`, `mute`, `dark mode` | More system commands (shown only when `nmcli`, `bluetoothctl`, `wpctl`/`pactl`, `gsettings` exist) |
| `clip`, `cb foo` | Clipboard history (opt-in, kept in memory only) |
| *(empty box)* | Recent and frequently used items |
| anything | A "Search the web" row at the bottom |

The right-hand preview pane shows details of the selected result: size and modified time
for files, the first lines of text files, image thumbnails, and clipboard text.

Clipboard history notes: on X11 (`xclip`) and wlroots/KDE Wayland (`wl-clipboard`) a background
watcher records every copy. GNOME's Wayland session forbids that, so there the history only
gains what is on the clipboard each time you open the launcher.

Keys: Left/Right/Home/End/Delete edit the query (Ctrl+Left/Right jump words, Ctrl+A/E home/end);
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
