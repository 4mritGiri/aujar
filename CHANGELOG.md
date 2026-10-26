# Changelog

Format: [Keep a Changelog](https://keepachangelog.com/en/1.1.0/). Versioning: [SemVer](https://semver.org/).

## [Unreleased]

### Added
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
