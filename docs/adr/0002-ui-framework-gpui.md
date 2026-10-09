# ADR 0002: Use GPUI for the desktop UI

- Status: proposed (pending a spike; see "Validation")

## Context

Aujar needs a fast, GPU-rendered UI for the launcher, zones editor, overlays and settings.
GPUI (the framework behind the Zed editor) renders through wgpu on Linux (Vulkan, with a GL
fallback) and has native Wayland and X11 backends.

Known constraints:

- GPUI is not a conventional, stable, semver-managed library. Upstream is consumed as a git
  dependency pinned to a commit; community snapshots and forks exist on crates.io. APIs change.
- GPUI has no global-hotkey support on Wayland (protocol limitation) and, in at least some
  builds, no `wlr-layer-shell`, which a launcher/overlay wants on Sway, Hyprland and KDE.
- GPU rendering needs a working Vulkan/GL driver. Headless, VM and very old hardware need a
  software fallback.

## Decision

1. Build the UI with GPUI, in a **separate process** (`aujar-ui`) that talks to the daemon
   over the existing IPC. The daemon stays headless and toolkit-free.
2. Pin GPUI to an exact git revision in `apps/aujar-ui/Cargo.toml`; upgrade deliberately via PR.
3. Keep `aujar-ui` **outside the default workspace members** so `cargo check` for the platform
   crates does not require GPU/windowing system libraries. UI has its own CI job.
4. Global hotkeys, tray and window control stay in the daemon (portals, X11 grabs, compositor
   IPC), not in the UI toolkit. The daemon tells the UI to show/hide via IPC events.
5. No Aujar logic in the UI: it renders state and sends requests.

## Consequences

- Needs **event subscription** (server push). Implemented: see [IPC](../IPC.md).
- Launcher on GNOME runs as a normal window; on wlroots/KDE we want layer-shell (may need an
  upstream patch or a fork).
- Fallback plan if the spike fails (API churn, missing Wayland features): GTK4 (`gtk4-rs`) or
  `iced` (wgpu). The IPC boundary makes this a contained swap.

## Validation (spike, before accepting)

- Hello-window and launcher-style popup on: X11, KDE Wayland, GNOME Wayland, Sway, Hyprland.
- Measure cold start, frame time, idle CPU/GPU, and memory vs. a GTK4 baseline.
- Verify behaviour with no GPU (llvmpipe/lavapipe) and with Vulkan unavailable (GL fallback).
- Confirm IME input, HiDPI/fractional scaling, and screen-reader support (AT-SPI).
