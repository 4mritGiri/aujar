# Smoke Test

1. Start the daemon: `aujar daemon`.
2. `aujar ping` → expect `Pong`.
3. `aujar health` → confirm session detection (X11/Wayland).
4. `aujar rename --pattern old --replacement new <files>` → prints a plan only; no files change.
5. Repeat with `--apply` → files renamed; re-run to confirm collisions are reported.
6. `aujar rename --pattern old --replacement ../x <file>` → must be rejected (`InvalidTarget`).
