# GPUI Spike

Goal: decide whether GPUI is viable for Aujar's UI ([ADR 0002](adr/0002-ui-framework-gpui.md)).
The spike lives in `apps/aujar-ui` and is built **separately** from the main workspace.

What it does: a GPUI window opens, renders on the GPU, is shown/hidden by daemon events
(`aujar launcher show|hide|toggle`), and lets you **type a query, pick a result and launch it**.

Keys: type to search, `Up`/`Down` to select, `Enter` to launch (calculations are not launchable yet),
`Esc` to close.

## Build and run

Ubuntu/Debian build prerequisites (install the matching `-dev` package for any missing `.pc` file):

```bash
sudo apt install build-essential pkg-config libxkbcommon-dev libxkbcommon-x11-dev \
  libwayland-dev libx11-xcb-dev libxcb1-dev libvulkan-dev mesa-vulkan-drivers libfontconfig-dev
```

```bash
cargo run -p aujar-launcher-app -- daemon                     # terminal 1
cd apps/aujar-ui && cargo run                                 # terminal 2 (first build is slow)
# then type, e.g. `fire`, press Enter to launch
cargo run -p aujar-launcher-app -- launcher toggle            # terminal 3: window opens
cargo run -p aujar-launcher-app -- launcher toggle            # window closes
```


## Checklist (record results in the ADR)

| Check | How | Result |
|---|---|---|
| Builds with pinned `gpui-pre =0.3.3` / `gpui-pre-platform =0.3.3` | `cargo build` | ✅ Ubuntu, rustc 1.97, debug build 2m39s |
| Window opens on your session | toggle | ✅ Wayland (GNOME-style session): window opens with rounded corners and live results |
| Toggle show/hide from the CLI | `launcher toggle` twice | ✅ "Delivered to 1 subscriber" |
| Typing, selection, Enter-to-launch, Esc | use the keyboard | |
| Window opens on X11 | log into an X11 session, repeat | |
| Daemon restart reconnects | stop/start the daemon, toggle again | |
| GPU in use | `vulkaninfo --summary`; watch GPU load (`intel_gpu_top`, `nvtop`, `nvidia-smi`) while resizing | |
| Idle cost | `top -p $(pgrep aujar-ui)` with the window open and untouched; target ~0% CPU | |
| Cold start | time from `toggle` to visible window | |
| Memory | RSS in `top` with window open | |
| Works without GPU | run with a software Vulkan driver (`mesa-vulkan-drivers` includes lavapipe); record behaviour | |
| HiDPI / fractional scaling | set 125-200% scale, check sharpness | |
| Overlay behaviour on your compositor | is it a normal window or can it be centered/undecorated? | |

## Known limits of this spike

- Closing the window removes it; reopening creates a new one (no true hide yet).
- GNOME on Wayland has no layer-shell, so the launcher is a normal, decorated window there.
- Keyboard input uses plain key events (`key_char`): no IME/composition, no cursor movement or
  selection inside the query. IME needs an `EntityInputHandler` implementation (a later step).
- If `launcher toggle` reports 0 subscribers right after starting the UI, the UI had not yet finished
  subscribing (or the daemon had just restarted and the UI was in its 1 s reconnect delay).
- The window looked very slightly translucent in a screenshot (a terminal border line showed through).
  Check whether that is intended; the background color is set opaque in code.
- The official crates.io `gpui 0.2.2` failed to build on a current toolchain (`xattr 0.2.3` vs
  `libc`) and a git dependency on Zed `main` failed to resolve, so the spike uses the community
  `gpui-pre*` snapshot crates (zed@5b055fa). Commit `apps/aujar-ui/Cargo.lock`.
- The snapshot's Linux platform crate depends on a wgpu-based renderer crate (`gpui-pre-wgpu`), so
  rendering goes through wgpu (Vulkan/GL). Confirm the active backend on your machine and record it.

## Decision rule

Accept GPUI if it builds reproducibly, runs on KDE/GNOME/Sway/X11 with acceptable idle cost, and
text input/IME is achievable. Otherwise fall back to GTK4 or iced behind the same IPC protocol.
