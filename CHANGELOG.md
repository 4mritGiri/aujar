# Changelog

Format: [Keep a Changelog](https://keepachangelog.com/en/1.1.0/). Versioning: [SemVer](https://semver.org/).

## [Unreleased]

### Fixed
- **Mouse Borders: "Scan local network" froze the settings window ("aujar-settings is not responding") on every machine.** The scan used a blocking UDP socket with no read timeout and ran on the UI thread, so it waited forever when nobody answered. Nobody could answer anyway: no machine ever listened for the query. The scan now runs on a background thread, ends after its timeout whatever the network does, and finds machines that have a pairing window open.

### Added
- **Mouse Borders: pair over the network.** *Start pairing* on one machine and *Find devices* → *Pair* on the other; both screens show the same 8-digit code (Noise `NN` handshake, numeric comparison) and only after both people confirm does the key travel, encrypted. No more copying a code between machines. The pairing code stays as the fallback. New module `lan_pair`, new discovery packets with name and port.
- **Mouse Borders: drag-and-drop layout.** The layout is a 4 × 3 board you arrange by dragging machines (drop on another machine to swap), like the display arrangement in an operating system, so machines can sit left, right, above or below. New `matrix = "free"` (slot = row * 4 + column); `row` and `grid` layouts keep working and convert in place on the first drag. Pairing codes carry `free` layouts (older Aujar versions reject them).
- **Files grid and filter tokens.** The Files button now opens a grid of your recent files, in two sections like Spotlight's: **Suggestions** (the recent files you open most from Aujar, up to 5) and **Recents**. Pictures show their own thumbnail; other files show their icon and a two-line name; arrows move across and down, Enter opens, right-click for the actions menu. Typing or choosing a file type chip switches to the detailed list (kind, size, date, folder, with the preview pane). The chip you pick also shows inside the search bar as a blue token ("PDFs", or a group such as "Internet" in Applications); Backspace on an empty field removes the token first, then the scope. "View as Grid / List" in the "..." menu now covers Files too.
- **Easier to discover and use.** The round buttons, "..." and the new clear button show a tooltip on hover ("Applications", "Back to all results" when on, "Clipboard history", "More", "Clear"). A clear (x) button appears in the search bar once you have typed; it clears the text and keeps the scope. **Alt+Left / Alt+Right** switch the chips under the bar (All, Applications, Files, ...; the group chips in the Applications grid; the file types in Files), wrapping around. The footer now says what applies right now: arrows to move in the grid, Alt+arrows to filter when chips show, Backspace to go back out of a scope.
- **File rows with details.** Files and folders now read "PDF document · 57.0 KB · yesterday 9:05 PM · ~/Downloads" (kind, size, when it changed, where it is) wherever they are listed. The daemon sends each result's size and modification time (`meta:<size>:<modified>` in its detail) and the UI words them. `kind:file` (also `kind:files`, `kind:any`) matches every file and folder, and any `kind:` search may list up to 40 results (a plain name search still shows 8); the Files view uses it for typed text.
- **Nothing to remember.** A "..." button at the right end of the search bar opens a menu: **View as Grid / List** (in Applications), **Tips & shortcuts**, **Settings**. The tips panel (also F1 or Ctrl+/) lists what you can type, each with an example that fills the search field when clicked (open an app, find a file, only PDFs, math, unit conversion, lock/sleep, switch window, define, weather, web search, remote desktop, clipboard), and the keys worth knowing. An empty launcher with nothing recent shows a few of those examples as chips ("Try one of these, or just start typing"). Esc closes the menu, then the tips, then clears and closes as before.
- **File-type chips in Files.** The Files view shows All, Documents, PDFs, Images, Videos, Audio, Archives, Code and Folders; a chip searches `kind:pdf` etc. (with whatever you typed) instead of making you type it.
- **Spotlight's completion hint and top-hit icon.** While you type, the search bar shows the title of the result Enter would open in grey after your text ("fire — Firefox"), and that result's icon at the right end of the bar. It follows the selection, so arrowing down changes it; the web-search row and clipboard entries are left out.
- **Applications grid.** The Applications button now shows every installed application as an icon grid (5 per row, arrow keys move across and down, Enter opens, right-click for the actions menu) with group chips (Internet, Productivity, Media, Graphics, Development, Games, Utilities, System, ...) taken from each `.desktop` file's `Categories=`. While a round button is on, the bar shows its icon and, in the empty field, its title ("Applications", "Recent Files", "Actions", "Clipboard") like a placeholder. The scope stays on while you type, so the text searches inside it (typing `fire` in Applications filters the grid; `lock` in Actions, `url` in Clipboard, `invoice` in Files); Backspace on an empty field takes the scope off, and so does pressing the button again. `kind:app text` and `kind:action text` are searchable queries too. `kind:app` now returns up to 150 applications, in A to Z order, with the group in the result's detail; the daemon accepts search limits up to 200.
- **Clipboard view like Spotlight's.** Clipboard history now remembers when each entry was copied. The Clipboard button (or `clip`) shows a header with **Clear history**, and each row reads "Text · Copied 4:19 PM" or "URL · en.wikipedia.org · Copied 4:17 PM" (today's time, "yesterday 9:05 PM", "Mon 8:00 AM", then "N days ago") with a copy button on the right. Copying the same text again moves it to the top with the new time. Uses the system clock through `libc::localtime_r` (new direct dependency `libc`).
- **Spotlight-style launcher layout.** The search field is now a pill of its own, with four round buttons beside it (Applications, Files, Actions, Clipboard); the results sit in a separate card below, and with nothing to show only the pill and buttons remain. The buttons type `kind:app`, `kind:recent`, `kind:action` and `clip`; pressing the active one clears the query. When results hold more than one kind, chips ("All", "Applications", "Files", ...) narrow the list. New listing queries `kind:app` (all applications A to Z) and `kind:action` (all actions) return up to 48 results, with no web-search row.
- **NexDesk settings page**: install status, an Open NexDesk button and the launcher switch, in the settings window (Settings -> NexDesk).
- **`rdp <name>` in the launcher**: lists your saved NexDesk connections (`rdp`, `rdp office`, `nexdesk office`) and starts one with `nexdesk --connect "<name>"`, so NexDesk itself asks for the password. Reads only the names (and address and user) of NexDesk's `.rdp` files, never its vault. New plugin toggle `[plugins] nexdesk` (default on) and a switch under Settings -> Search plugins. Needs the NexDesk `--connect` option added alongside it.
- **NexDesk integration** (`modules/nexdesk`, `aujar nexdesk open|status`): starts the separate [NexDesk](https://github.com/4mritGiri/nexdesk) remote-access manager as a detached process and reports whether it is installed. NexDesk sources are a git submodule at `integrations/nexdesk`, excluded from the Cargo workspace (`scripts/add-nexdesk-submodule.sh`, `scripts/build-nexdesk.sh`). IPC `NexDesk` request/response (additive); `open` needs `execute_command`. See [NEXDESK](docs/NEXDESK.md).
- **Mouse Borders on macOS and Windows** (in addition to Linux): a platform-neutral input model (USB HID key codes, one master key table cross-checked against Linux, Windows and macOS codes, protocol v2), Windows backend (low-level hooks, `SendInput`, `LockWorkStation`), macOS backend (Quartz event tap, posted events, permission checks), cross-platform clipboard (`arboard`, with password-manager content never read) and paths. The Windows and macOS backends are type-checked and their logic unit-tested but **not yet run on real machines**.
- **`aujar-mwb`**, a standalone cross-platform agent and command line (Unix socket / Windows named pipe control channel, `run`, `autostart`, `doctor`, `audit`, plus every `mwb` command), and the shared `aujar-mwb-cli` crate used by it and `aujar mwb`.
- **Enterprise hardening**: a JSON-lines audit log (never contains clipboard contents or keys; rotated; private), a `doctor` preflight with per-OS fixes, start-at-login for systemd / launchd / Windows, pure-Rust crypto (no C toolchain needed on Windows/macOS), and a CI job building and testing on Linux, macOS and Windows.
- **Settings window restyled** like a macOS window: its own rounded, translucent frame with traffic-light buttons, sidebar toggle, back/forward, a breadcrumb pill, settings search (Ctrl+F) and a `...` menu; drag the title bar to move, double-click to zoom, drag the edges to resize. New Files and History pages in a collapsible "Data" group.
- **Appearance**: opacity, corner radius, blur behind windows, window buttons on/off; launcher "Move by dragging" and "Opens at" (top / center / last place; placement works on X11, dragging works everywhere).
- **Launcher**: drag the search bar or footer to move it; Spotlight-style preview pane with a large preview, kind, information table, folder contents and Open / Show in Folder buttons.
- **Redesigned settings window**: a sidebar with pages (Appearance, Search plugins, Web search, Mouse Borders, General), page titles and descriptions, plugins grouped by what they touch with an `Online` badge on those that use the network, one file per page.
- **Mouse Borders page** in the settings window: live status, pairing, a visual layout editor, switching and sharing options. Pairing codes (`aujar mwb pair`, `aujar mwb join`, or Copy / Join in the window) carry the security key and layout so nothing has to be typed.
- **Mouse Borders** (`modules/mouse-borders`, `aujar mwb ...`): share one keyboard and mouse between up to four Linux machines, with edge switching (X11), `Ctrl+Alt+1..4` jumping (X11 and Wayland), a shared clipboard (text and images), file transfer (100 MB), multi-machine input, wrap, Easy Mouse modes, corner blocking, relative movement, keep-awake and remote lock. Encrypted with a Noise `XXpsk3` channel keyed by a generated security key; off by default. Optional least-privilege system service for the login screen. See [MOUSE_BORDERS](docs/MOUSE_BORDERS.md).
- `Capability::ControlInput` and `Capability::Network`; IPC `MouseBorders` request/response (additive); `[mouse_borders]` settings.
- `Rect::new/right/bottom/contains/clamp` in `aujar-core`.
- Dictionary lookup (`define word`, `meaning of word`, `what does word mean`, and Ctrl+L on any word, like Spotlight's Cmd+L). Powered by dictionaryapi.dev; only the word you typed is sent.
- Weather (`weather paris`, `kathmandu weather`), via Open-Meteo: current conditions plus a five-day forecast; Fahrenheit/mph option.
- Stock quotes (`stock aapl`, `$tsla`, `msft share price`), via Yahoo Finance's public endpoint (unofficial, delayed).
- Sports scores (`nba`, `premier league`, `nfl scores`, `nba lakers`), via ESPN's public scoreboard (unofficial). Live games first.
- Right-click menu on results, also Ctrl+K / Shift+F10: Open, Show in Folder, Copy, Look Up, Search the Web.
- Drag a file or folder result out of the launcher into other apps (Wayland; GPUI's outbound file drag). The window stays open for 30 s after a drag leaves it, then closes.
- Window switching (`< firefox`, `win term`, `switch to mail`): GNOME via the new Aujar Windows Shell extension (`aujar gnome-extension install|remove|status`), plus Sway (`swaymsg`), Hyprland (`hyprctl`) and X11 (`wmctrl`) backends.
- Settings toggles for each new lookup; `[weather] imperial`.
- `SearchResult::detail` / IPC `SearchHit::detail` (additive) carry preview text; new result kinds `definition`, `weather`, `stock`, `sports`, `window`, `loading`, `notice`.
- Terminal applications (`Terminal=true`, e.g. htop) now launch inside a terminal emulator (`$TERMINAL`, else the first of a known list).

### Changed
- Launcher UI redesigned in the style of Spotlight: vector icon set, a search bar with a vector magnifier, rounded rows, answer cards for calculations, weather, quotes, scores and definitions, a context-aware preview pane, a quieter footer. Default width 720.

### Fixed
- **Every file type showed the same icon** on the blue token in the search bar (the Files button's folder). PDFs, pictures, videos, audio, archives, code, documents and folders now each have their own icon, on the token and on the chips; the Applications group chips and token have icons too (globe, briefcase, video, picture, code, gamepad, wrench, sliders, book). Nine new built-in icons.
- **Applications started from Aujar could not use setuid helpers.** `aujar.service` set `NoNewPrivileges=true` (and `LockPersonality=true`), and every program the launcher starts inherits them, so VirtualBox failed with "Effective UID is not root (rc=-10)" while the same app opened from the app menu worked (ping, Wireshark's capture helper and other setuid programs were affected too; `LockPersonality` also hurts some 32-bit and Wine programs). Both directives are gone, with a comment explaining why, and a test keeps sandboxing directives out of the unit. Re-run `scripts/install-local.sh` (or reinstall the package) to pick it up.
- **The launcher window fits what it shows.** It used to stay 640 px tall, leaving an empty strip of window under the results card, and the compositor drew its shadow and background around that whole rectangle (a shadow below the card and mismatched corners). The window now resizes to the search row alone, or the row plus the card (a fixed height; tall for results and tips, short for a notice or the "Try" examples), so nothing is left outside what is drawn.
- **The preview's Location row** showed the file row's new subtitle ("PDF document · 22.5 KB ...") instead of the folder. It shows the folder again, with `~` for your home.
- **Restart loops that can flicker the desktop.** A second `aujar-ui` exited with status 0 while `aujar-ui.service` had `Restart=always`, so systemd respawned it every two seconds for as long as another UI (for example a `cargo run` build) was alive. Duplicates now exit 75, which both units treat as "do not restart"; both units also stop after 5 starts in 60 s instead of looping forever.
- A second daemon deleted the socket of a running one, leaving the first running but unreachable (and both bound to the global shortcut). It now refuses with exit status 75 and a clear message; only a stale socket file left by a crash is replaced.
- `scripts/install-local.sh` rewrote the desktop file on every run, which makes GNOME Shell rescan applications and redraw the dash; it now leaves an unchanged file alone, restarts the daemon and UI in order, and warns about an `aujar-ui` started by hand. The no-window desktop entry no longer asks for a startup notification.
- Search ran on an async worker thread while holding a global mutex; searches now run on the blocking pool, concurrently, outside the lock.
- Changing any setting (even the theme) rebuilt every provider and emptied the file index until the next scan; providers now rebuild only when a provider-affecting option changed, and the settings file is checked at most once per second.
- Icons were resolved on disk for every matching application, not just the ones shown. A one-letter query probed hundreds of icon paths; only the top results are resolved now.
- Application ranking allocated two strings per candidate per keystroke; names are lower-cased once at scan time.
- File search fully sorted every hit; it now selects the top results first. Tie-breaking is deterministic.
- Newly installed applications did not appear until the daemon restarted; the application directories are re-checked every 10 s (a handful of `stat` calls unless something changed).
- The UI sent one request per keystroke and handled them one at a time, so a slow search delayed later keystrokes. Searches are now debounced (30 ms), concurrent, and only the newest reports back.
- `~` display of a path under a different user whose name starts with yours (`/home/ann2` shown as `~2`).
- Currency: the rates cache is written atomically, and after an offline start rates are retried after two minutes instead of an hour.
- Unit conversion accepts thousands separators (`1,000 m to km`).
- Preview of a huge folder counted every entry on the UI thread; it is now capped at 5,000.
- Every frame cloned every result; rows are now borrowed.
- Clipboard watcher re-parsed the settings file every 6 s and accepted clipboard text of any size; it now checks the file's modification time and ignores copies over 64 KiB.

### Added (earlier)
- `aujar shortcut install|remove`: adds/removes a GNOME custom shortcut (Ctrl+Alt+Space) running `aujar launcher toggle` via `gsettings`; works where the portal route fails (GNOME 50: "app id required"). Docs record the portal findings.
- Color picker gives up after 120 s instead of waiting forever; `aujar doctor` explains the "app id required" portal failure; troubleshooting steps for it.
- Clearer version-skew handling: the daemon answers unparseable requests with an error, and the client explains an empty reply; `install-local.sh` now restarts the daemon.
- `aujar doctor` and IPC `Status`: reports daemon reachability, session, subscribed UIs and global-shortcut state (bound trigger or the reason it is unavailable); daemon logs each shortcut press; troubleshooting guide.
- UI: Enter on a calculation result copies the answer to the clipboard.
- Color picker: `aujar color pick` (desktop portal `PickColor`, via the daemon, gated by `screen_capture`) and `aujar color convert <hex>`; hex/RGB/HSL conversion in `aujar-color` with tests.
- Packaging: `.deb` (cargo-deb), `.rpm` (cargo-generate-rpm), tarball for x86_64 and aarch64, Arch PKGBUILD, per-user `install-local.sh`; CI builds and installs the deb on every PR; tag-triggered release workflow.
- Native global hotkey (`aujar-hotkeys`): registers Ctrl+Alt+Space through the `GlobalShortcuts` portal and toggles the launcher; falls back with a log message when the portal is missing. `Options::hotkeys` (tests disable it); policy `global_hotkeys` disables it.
- GPUI spike: keyboard-driven launcher (type, Up/Down, Enter to launch, Esc to close).
- GPUI spike (`apps/aujar-ui`, pinned `gpui-pre =0.3.3` snapshot, non-default workspace member: `cargo run -p aujar-ui --release`): toggles a window from daemon events and shows search results. See docs/UI_SPIKE.md.
- IPC event stream: `Subscribe` connections, `Launcher` show/hide/toggle broadcast, `Shutdown` event; `aujar launcher` and `aujar events`.
- `aujar_ipc::{encode_line, decode_line, Envelope, subscribe, EventStream}`; daemon and client share one framing implementation.
- Daemon integration tests for the event stream. Docs: IPC protocol and keyboard-shortcut recipes.
- `Execute` IPC request and `aujar run <id>`: launches indexed applications (no shell, field codes dropped, own process group).
- Capability enforcement: each request maps to a capability; `/etc/aujar/policy.toml` can deny capabilities (fail-closed).
- `Capability::ALL` / `Capability::parse`; `tokio` `process` feature.
- Launcher engine: provider trait, application (`.desktop`) and calculator providers, tiered ranking.
- IPC `Search` request/response (protocol v1, additive) and CLI `aujar search` / `aujar modules`.
- ADR 0002 (GPUI UI proposal) and UI/UX design doc.
- Feature matrix, threat model, packaging notes, ADR process.
- Governance, security policy, code of conduct, contributing guide, issue/PR templates.
- CI matrix (Ubuntu/Fedora/Arch), MSRV, `cargo-deny`, Dependabot, release workflow.
- Daemon handles `Request::Modules`.

### Changed
- systemd unit now targets `/usr/bin/aujar` and `graphical-session.target`; removed mount-namespace hardening that fails in unprivileged user units.
- `Capability` now has a single definition in `aujar-core` (re-exported by `aujar-runtime`).
- Daemon rejects IPC clients whose uid is neither the daemon's owner nor root; 5 s request read timeout.
- Default socket path uses `XDG_RUNTIME_DIR` instead of a hard-coded uid.
- License metadata set to Apache-2.0 to match `LICENSE`.
- IPC socket is `0600`; requests capped at 64 KB; daemon handles SIGTERM.
- systemd unit and desktop entry now call `aujar daemon`.

### Fixed
- Non-exhaustive match on `Request` in the daemon.
- Rename rejects targets containing path separators, NUL, `.` or `..`.

### Removed
- Unreferenced legacy `powertoys-*` crates and apps.

## [0.1.1]
- Rename engine with plan/preview, collision detection and two-phase execution.
