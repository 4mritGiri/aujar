# Smoke Test

1. Start the daemon: `nexora daemon`.
2. `nexora ping` → expect `Pong`.
3. `nexora health` → confirm session detection (X11/Wayland).
4. `nexora rename --pattern old --replacement new <files>` → prints a plan only; no files change.
5. Repeat with `--apply` → files renamed; re-run to confirm collisions are reported.
6. `nexora rename --pattern old --replacement ../x <file>` → must be rejected (`InvalidTarget`).
