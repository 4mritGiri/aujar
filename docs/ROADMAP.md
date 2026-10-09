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
- [ ] Protocol versioning
- [ ] Persistent daemon state

## v0.2 — Launcher
- GPUI spike across X11/KDE/GNOME/Sway/Hyprland (ADR 0002)
- [ ] IPC event stream and execute request
- [x] IPC search request
- [x] Application discovery (`.desktop`, XDG + Flatpak dirs)
- [ ] File provider
- [x] Calculator provider
- [ ] Command provider
- [ ] Keyboard shortcut integration
- [x] Search ranking (tiered match scoring)
- [ ] Native desktop UI

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
