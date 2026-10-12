# Troubleshooting

Start with the built-in diagnosis:

```bash
aujar doctor
```

It reports whether the daemon is reachable, the session type, how many UIs are subscribed, and
the state of the native global shortcut.

## The keyboard shortcut does nothing

Work through these in order.

1. **Is the UI running?** A shortcut only _toggles_ a running UI; the daemon alone shows no
   window. `aujar doctor` should show `UI clients: 1` or more. While the UI is still a spike,
   start it yourself: `cargo run -p aujar-ui --release`.
2. **Do the events arrive?** In one terminal run `aujar events`, press the shortcut, and watch for
   `Launcher(Toggle)`. If it appears, the shortcut and daemon work and the problem is the UI.
   If not, continue.
3. **Which mechanism are you using?** There are two, and they can fight over the same key:
   - _Native_ (portal): the daemon asks the desktop for **Ctrl+Alt+Space**. `aujar doctor` shows
     `hotkey: bound: ...` when the desktop accepted it.
   - _Manual_: you add a custom shortcut that runs `aujar launcher toggle`.

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

## `hotkey: unavailable: ... An app id is required`

**Short answer (GNOME):** run `aujar shortcut install`. It uses GNOME's own shortcut settings and
needs no app id. The rest of this section explains the portal failure and what we tried.

GNOME (and some other desktops) only bind a global shortcut for a program they can _identify_.
A daemon started by systemd is anonymous unless it is identified in one of two ways.

**1. Make the desktop file visible to the portal.** The daemon registers the id `dev.aujar.Aujar`;
the log line `App info not found for 'dev.aujar.Aujar'` means the portal could not find
`dev.aujar.Aujar.desktop`. Check:

```bash
ls -l ~/.local/share/applications/dev.aujar.Aujar.desktop
gtk-launch dev.aujar.Aujar            # runs `aujar health`; "no such application" = not found
```

If the file exists but is not found, the portal service probably started before the folder or file
existed: log out and back in (or reboot), then `systemctl --user restart aujar`.

**2. Name the service after the app id** (tried on GNOME 50.1: no effect) (the convention portals understand for systemd-launched
apps: `app-<application-id>.service`). Quick experiment without touching the repo:

```bash
systemctl --user stop aujar
cp ~/.config/systemd/user/aujar.service ~/.config/systemd/user/app-dev.aujar.Aujar.service
systemctl --user daemon-reload
systemctl --user start app-dev.aujar.Aujar.service
sleep 2; aujar doctor                  # hotkey: bound: ... ?
```

Undo with `systemctl --user stop app-dev.aujar.Aujar.service`, delete that file, and
`systemctl --user start aujar`. If it works, the packaged unit will be renamed accordingly.

**Workaround that works:** `aujar shortcut install`, or bind `aujar launcher toggle` manually with
the absolute path `aujar doctor` prints. Neither needs the portal.

## `aujar color pick` seems to hang

It waits for you: the desktop shows a picker overlay (crosshair); click a pixel to choose it, or
press Esc to cancel. If nothing appears it now gives up after 120 seconds with an error. If the
overlay never shows, check `journalctl --user -u aujar` for portal errors and tell us your desktop
version.

## `EOF while parsing a value` / "daemon closed the connection" / "daemon is older than this client"

The running daemon is an older build than the `aujar` command you just ran: it does not know the
newer request and hangs up. Restart it:

```bash
systemctl --user restart aujar
```

`scripts/install-local.sh` does this for you. (Newer daemons answer with an explicit
"unrecognised request" error instead.)

## `aujar doctor` says the daemon is not reachable

```bash
systemctl --user status aujar
journalctl --user -u aujar -n 50 --no-pager
```

Check that the CLI and daemon agree on the socket (`AUJAR_SOCKET`, otherwise
`$XDG_RUNTIME_DIR/aujar.sock`).

## Changed the code but nothing changed

The installed daemon is a copy. Rebuild and restart it: `scripts/install-local.sh`.
