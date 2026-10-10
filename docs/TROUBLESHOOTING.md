# Troubleshooting

Start with the built-in diagnosis:

```bash
aujar doctor
```

It reports whether the daemon is reachable, the session type, how many UIs are subscribed, and
the state of the native global shortcut.

## The keyboard shortcut does nothing

Work through these in order.

1. **Is the UI running?** A shortcut only *toggles* a running UI; the daemon alone shows no
   window. `aujar doctor` should show `UI clients: 1` or more. While the UI is still a spike,
   start it yourself: `cd apps/aujar-ui && cargo run`.
2. **Do the events arrive?** In one terminal run `aujar events`, press the shortcut, and watch for
   `Launcher(Toggle)`. If it appears, the shortcut and daemon work and the problem is the UI.
   If not, continue.
3. **Which mechanism are you using?** There are two, and they can fight over the same key:
   - *Native* (portal): the daemon asks the desktop for **Ctrl+Alt+Space**. `aujar doctor` shows
     `hotkey: bound: ...` when the desktop accepted it.
   - *Manual*: you add a custom shortcut that runs `aujar launcher toggle`.

   If you created a manual shortcut on the same keys, the desktop may refuse the native one
   (`hotkey: unavailable: ...`) or the keys may do something else. Use **one** of the two, or give
   them different keys.
4. **Manual shortcut: use the absolute path.** Desktop shortcuts often run with a minimal `PATH`
   that does not include `~/.local/bin`. Use the full path, e.g.
   `/home/you/.local/bin/aujar launcher toggle` (`aujar doctor` prints it).
5. **Look at the daemon log** while pressing the key:
   `journalctl --user -u aujar -f`. You should see `global shortcut pressed` for the native
   shortcut. `global shortcuts unavailable: <reason>` explains why it is off.
6. **Install the desktop file** so GNOME can identify the app for the portal:
   `install -Dm644 packaging/desktop/dev.aujar.Aujar.desktop ~/.local/share/applications/`
   and restart the daemon: `systemctl --user restart aujar`.
7. **The window opens but typing does nothing (Wayland).** Some compositors refuse to give a newly
   opened window keyboard focus (focus-stealing prevention). Click the window once and type; report
   your compositor and version if this happens every time.

## `aujar doctor` says the daemon is not reachable

```bash
systemctl --user status aujar
journalctl --user -u aujar -n 50 --no-pager
```

Check that the CLI and daemon agree on the socket (`AUJAR_SOCKET`, otherwise
`$XDG_RUNTIME_DIR/aujar.sock`).

## Changed the code but nothing changed

The installed daemon is a copy. Rebuild and restart it: `scripts/install-local.sh`.
