# Feature Matrix

Aujar is a Linux productivity toolkit inspired by the utilities Windows users get from
Microsoft PowerToys. It is an independent project and is not affiliated with Microsoft.

**Legend**

| Symbol | Meaning |
|---|---|
| ✅ | Implemented and tested |
| 🚧 | Partially implemented / stub exists |
| 📅 | Planned (see [ROADMAP](ROADMAP.md)) |
| ⚠️ | Possible only with a compositor-specific backend or extension |
| ❌ | Not possible with current Linux protocols |
| — | Not applicable |

Status reflects the repository at **v0.1.1**. Linux has no single API for windows, hotkeys
or screen capture, so each feature lists its backend per session type.

## 1. Platform

| Capability | Crate | Status | Notes |
|---|---|---|---|
| Module runtime (lifecycle, registry, API versioning) | `aujar-runtime` | ✅ | Only `core` module registered so far |
| Capability model (declared per module) | `aujar-runtime` | 🚧 | Declared, **not yet enforced** |
| Versioned Unix-socket IPC | `aujar-ipc` | ✅ | JSON line protocol v1 (additive variants: `Modules`, `Search`); 64 KB request cap |
| Daemon (graceful shutdown, 0600 socket) | `aujar-daemon` | ✅ | SIGTERM + Ctrl-C |
| IPC peer authentication (peer-uid check) | `aujar-daemon` | ✅ | Per-client capability grants 📅 |
| Typed configuration (TOML) | `aujar-config` | 🚧 | Load only; no hot-reload/validation |
| X11 / Wayland session detection | `aujar-platform` | ✅ | Env-based |
| Compositor detection (GNOME/KDE/Sway/Hyprland) | `aujar-platform` | 📅 | v0.3 |
| Persistent state | `aujar-daemon` | 📅 | |
| Event bus | `aujar-runtime` | 📅 | |
| Plugin manifests / extension SDK | `aujar-plugin` | 🚧 | Manifest type only |
| Enterprise policy (`/etc/aujar/policy.toml`) | `aujar-config` | 📅 | Admin-enforced module/capability lockdown |
| Telemetry | — | — | None, by design |

## 2. Modules

| Module | PowerToys analogue | Crate | Status | X11 | Wayland: Sway / Hyprland | Wayland: KDE | Wayland: GNOME | Backend |
|---|---|---|---|---|---|---|---|---|
| Launcher engine | PowerToys Run | `modules/launcher` | 🚧 engine done: apps + calculator providers, ranking, IPC `Search`; no execute/UI yet | ✅ | ✅ | ✅ | ✅ | Session-independent; files and commands providers 📅, launching apps 📅 |
| Batch Rename | PowerRename | `modules/rename` | ✅ engine + CLI | ✅ | ✅ | ✅ | ✅ | Pure filesystem; GUI/file-manager integration 📅 |
| Window management | — | `modules/window` | 🚧 trait only | 📅 (EWMH / `x11rb`) | 📅 compositor IPC; list/activate only on generic wlroots | 📅 (KWin scripting, DBus) | ⚠️ (GNOME Shell extension) | |
| Zones (FancyZones) | FancyZones | `modules/zones` | 🚧 grid math | 📅 | 📅 (Sway/Hyprland IPC) | 📅 (KWin script) | ⚠️ (extension) | Depends on Window management |
| Workspace manager | Workspaces | `modules/workspace` | 🚧 stub (not in workspace) | 📅 | 📅 | 📅 | ⚠️ | Save/restore app layouts |
| Always on top | Always On Top | `modules/window` | 📅 | 📅 (`_NET_WM_STATE_ABOVE`) | ⚠️ (Hyprland pin / Sway sticky) | 📅 (KWin) | ⚠️ (extension) | ❌ on generic Wayland |
| Clipboard history | Advanced Paste / Win+V | `modules/clipboard` | 🚧 in-memory store | 📅 | 📅 (`wlr-data-control`) | 📅 (`ext-data-control`) | ⚠️ (no data-control; needs extension) | Sensitive-app exclusion planned |
| Color picker | Color Picker | `modules/color` | 🚧 stub | 📅 | 📅 (portal `PickColor`) | 📅 (portal) | 📅 (portal) | `xdg-desktop-portal` Screenshot |
| Screen ruler | Screen Ruler | `modules/ruler` | 🚧 stub | 📅 | 📅 (portal screencast) | 📅 | 📅 | Requires screen-capture permission |
| Global hotkeys | (all modules) | `aujar-runtime` | 📅 | 📅 (XGrabKey) | 📅 (portal GlobalShortcuts) | 📅 (portal) | 📅 (portal) | |
| Quick preview | Peek | — | 📅 | 📅 | 📅 | 📅 | 📅 | |
| Image utilities | Image Resizer | — | 📅 | 📅 | 📅 | 📅 | 📅 | Pure Rust, no session dependency |
| Keyboard remapping | Keyboard Manager | — | ⚠️ | ⚠️ (evdev/uinput, needs privileges) | ⚠️ | ⚠️ | ⚠️ | Privileged helper; design under review |
| Mouse utilities (find my mouse, highlighter) | Mouse Utilities | — | 📅 | ❌ generic / ⚠️ per compositor | ⚠️ | ⚠️ | ⚠️ | |
| Text extractor (OCR) | Text Extractor | — | 📅 | 📅 | 📅 | 📅 | 📅 | Screen capture + Tesseract |
| Awake (inhibit sleep) | Awake | — | 📅 | 📅 | 📅 (logind / portal Inhibit) | 📅 | 📅 | |

## 3. Front-ends

| Front-end | Status | Notes |
|---|---|---|
| CLI (`aujar`) | ✅ | `ping`, `health`, `windows`, `modules`, `search`, `rename`, `daemon` |
| Settings app | 📅 | v0.4+; GPUI (proposed, [ADR 0002](adr/0002-ui-framework-gpui.md)) |
| Launcher UI | 📅 | v0.2; GPUI, GPU-accelerated via wgpu (Vulkan/GL) with software fallback; see [UI_UX](UI_UX.md) |
| Tray / status indicator | 📅 | StatusNotifierItem |
| IPC event stream (daemon → UI) | 📅 | Prerequisite for UI; see UI_UX |
| GPU renderer selection / software fallback | 📅 | Part of the UI spike |

## 4. Distribution & Quality

| Area | Status | Notes |
|---|---|---|
| CI: fmt, clippy, tests (Ubuntu / Fedora / Arch), MSRV 1.85, `cargo-deny` | ✅ configured | See `.github/workflows/ci.yml` |
| Release tarball + checksums on tag | ✅ configured | `.github/workflows/release.yml` |
| systemd user unit, `.desktop` entry | ✅ | `packaging/` |
| `.deb`, `.rpm`, Flatpak, AppImage, AUR | 📅 | See [PACKAGING](PACKAGING.md) |
| Signed releases / SBOM / provenance | 📅 | v1.0 |
| Integration tests (daemon + CLI) | 📅 | Only unit tests today (rename) |
| Fuzzing (IPC parser, rename planner) | 📅 | |
| Security review | 📅 | v1.0 |

## Supported environments (target)

| Environment | Tier | Notes |
|---|---|---|
| X11 (any DE) | 1 | Full feature set |
| KDE Plasma 6 (Wayland) | 1 | Via KWin scripting + portals |
| Sway, Hyprland (Wayland) | 2 | Via compositor IPC |
| GNOME (Wayland) | 2 | Portals + optional Shell extension; some features ❌ |
| Other wlroots compositors | 3 | Subset (list/activate windows, clipboard, portals) |

Tier 1 = release-blocking tests. Tier 2 = best-effort tests. Tier 3 = community maintained.

_Update this file in the same PR that changes a feature's status._
