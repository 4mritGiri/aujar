# Aujar Roadmap

## v0.1 — Foundation
- [x] Cargo workspace
- [x] Core types
- [x] Typed configuration
- [x] Unix-socket IPC
- [x] Central daemon
- [x] Capability model
- [x] X11/Wayland session detection
- [x] Module boundaries
- [x] Automated CI on Linux (configured; verify first run)
- [x] Protocol versioning (supported range, answers in the client's version, readable mismatch errors, `Version` request; see [IPC](IPC.md#versioning))
- [x] Persistent daemon state (`state.toml`, crash-safe; used by Awake so far, see `aujar_config::State`)
- [x] Mouse without borders (Mouse without borders is a quick and easy way to move your cursor across multiple devices): `modules/mouse-borders`, see [MOUSE_BORDERS](MOUSE_BORDERS.md). Still open: Wayland edge switching (InputCapture portal), drag-and-drop in the layout editor and for file transfer

## v0.2 — Launcher
- [ ] GPUI spike across X11/KDE/GNOME/Sway/Hyprland (ADR 0002): needs a run on each desktop, checklist in [UI_SPIKE](UI_SPIKE.md)
- [x] IPC event stream (subscribe, launcher commands, shutdown)
- [x] IPC execute request (indexed apps only, policy-gated)
- [x] IPC search request
- [x] Application discovery (`.desktop`, XDG + Flatpak dirs)
- [x] File provider (name index, `kind:` / `ext:` filters, recent files)
- [x] Calculator provider
- [x] Command provider (programs on `PATH` run in a terminal; `>` shell commands, opt-in; system actions)
- [x] Keyboard shortcut integration (native via `GlobalShortcuts` portal; manual fallback `aujar launcher toggle`)
- [x] Search ranking (tiered match scoring)
- [x] Native desktop UI (`aujar-ui`, GPUI: search field with selection, undo and IME, results, previews, settings window)

## v0.3 — Window & Workspace
- X11 window backend
- KDE integration
- Hyprland integration
- Sway integration
- Workspace manager
- Zones UI
- Multi-monitor support

## v0.4 — Productivity tools
- Clipboard service
- Color picker
- Screen ruler
- Power Rename UI
- Quick preview
- Image utilities

## v0.5 — Extensions
- Plugin discovery
- Manifest validation
- Capability approval UI
- Extension SDK
- Example third-party extension

## v1.0
- Stable API/protocol
- Polished desktop UI
- Packaging: deb/rpm/Flatpak/AppImage/AUR
- Upgrade/migration support
- Crash-safe configuration
- Security review


See [FEATURE_MATRIX](FEATURE_MATRIX.md) for per-feature, per-compositor status.
