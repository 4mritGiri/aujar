# UI / UX Design

Status: design only. Framework decision: [ADR 0002](adr/0002-ui-framework-gpui.md).

## Principles

1. **Instant**: launcher appears in < 100 ms from hotkey; the UI process stays resident (hidden)
   or is started by the daemon on demand.
2. **Keyboard-first**, mouse-friendly, fully navigable without a pointer.
3. **Honest about the platform**: if a feature is unavailable on this compositor, say so and
   show why, instead of a dead button (driven by the daemon's capability report).
4. **Native feel**: follow the system light/dark preference, accent colour and fractional
   scaling; no custom window chrome on GNOME.
5. **Accessible**: AT-SPI labels, focus order, contrast AA, reduced-motion respected.
6. **Safe by default**: file operations always show a preview first (as the rename engine does).

## Surfaces

| Surface | Window type | Notes |
|---|---|---|
| Launcher | Centered popup (layer-shell where available, else normal undecorated window) | Query box, ranked results, action keys |
| Zones editor / overlay | Fullscreen overlay per monitor | Needs window-manager backend |
| Rename | Dialog (also launched from the CLI / file manager) | Live preview table with per-row status, errors highlighted |
| Clipboard history | Popup | Search, pin, per-item delete, sensitive-item masking |
| Color picker | Small overlay + history panel | Portal-based pick |
| Settings | Normal window | Module toggles, hotkeys, capability approvals, backend status |
| Tray | StatusNotifierItem menu | Pause, settings, quit |

## Hardware acceleration

- Rendering path: GPUI → wgpu → Vulkan (preferred) → OpenGL fallback → software (llvmpipe/lavapipe).
- Settings show the active renderer ("Vulkan – <GPU name>" / "Software") so users can diagnose.
- A **"Software rendering"** setting/flag must exist for broken drivers and VMs.
- Budget: idle UI process uses ~0% GPU (no continuous redraw); redraw only on input/animation.
- Test matrix: Intel, AMD (Mesa), NVIDIA (proprietary), VM (virtio-gpu), no-GPU (llvmpipe).
- wgpu generally honors the `WGPU_BACKEND` environment variable; confirm in the spike whether
  the pinned GPUI revision respects it before documenting it to users.

## Daemon ↔ UI contract (new)

| Need | Mechanism |
|---|---|
| Show/hide launcher on hotkey | IPC **event stream** (`Subscribe` → `Event::ShowLauncher`) |
| Search results | `Request::Search { query }` → ranked results |
| Run an action | `Request::Execute { result_id }` (capability-checked) |
| Rename preview/apply | existing `Request::Rename` |
| Backend/capability report | `Request::Capabilities` (per module: available / unavailable + reason) |
| Settings changes | `Request::SetConfig`, validated by the daemon |

All of these require protocol v2 or additive variants; document in CHANGELOG when added.

## Visual direction (to be refined with mockups)

Dark-first, high-contrast, 8 px grid, one accent colour from the system, a single icon set
(freedesktop symbolic icons), 150 ms ease-out transitions that honour reduced motion.
