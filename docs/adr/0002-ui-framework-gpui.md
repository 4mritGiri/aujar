# ADR 0002: Use GPUI for the desktop UI

- Status: proposed (pending a spike; see "Validation")

## Context

Aujar needs a fast, GPU-rendered UI for the launcher, zones editor, overlays and settings.
GPUI (the framework behind the Zed editor) is GPU-accelerated on Linux and has native Wayland and
X11 backends. Current Zed `main` renders through wgpu (Vulkan/GL); the older 0.2.x release used a
different renderer.

Known constraints:

- The official `gpui` 0.2.2 on crates.io did not build on a current toolchain (`xattr 0.2.3` vs
  `libc`), and a git dependency on Zed `main` failed to resolve. The spike therefore uses the
  community-published `gpui-pre*` snapshots of Zed `main` (zed@5b055fa), whose API differs from
  0.2.x (separate `gpui_platform`, `application()` entry point).
- **Supply chain:** these crates are published by a third party (a GPUI contributor), not by
  Zed. Before shipping: review the publisher and diff the snapshot against the upstream commit
  recorded in its metadata, and switch to an official Zed release as soon as one builds.
- GPUI has no global-hotkey support on Wayland (protocol limitation) and, in at least some
  builds, no `wlr-layer-shell`, which a launcher/overlay wants on Sway, Hyprland and KDE.
- GPU rendering needs a working Vulkan/GL driver. Headless, VM and very old hardware need a
  software fallback.

## Decision

1. Build the UI with GPUI, in a **separate process** (`aujar-ui`) that talks to the daemon
   over the existing IPC. The daemon stays headless and toolkit-free.
2. Pin GPUI to exact versions (`gpui-pre =0.3.3`, `gpui-pre-platform =0.3.3`) in
   `apps/aujar-ui/Cargo.toml` and commit its `Cargo.lock`; upgrade deliberately via PR.
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
