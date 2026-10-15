# Mouse Borders

Share one keyboard and mouse between up to four Linux machines running Aujar: move the pointer
off the edge of one screen and it appears on the next machine, with the clipboard and files
following you. It is Aujar's take on Microsoft's *Mouse Without Borders*.

> **Aujar to Aujar only.** Mouse Borders speaks its own encrypted protocol. It does not talk to
> Mouse Without Borders on Windows, and (like all of Aujar) it is not affiliated with Microsoft.

## Quick start

On every machine, install Aujar and make the daemon run (`systemctl --user enable --now aujar`).

1. **Describe the layout** in `~/.config/aujar/settings.toml`. The same text can go on every
   machine ([full example](../config/mouse-borders.example.toml)):

   ```toml
   [mouse_borders]
   enabled = true
   machine_name = "desk"          # this machine; empty means the hostname

   [[mouse_borders.machines]]
   name = "desk"
   slot = 0                       # left to right on your desk

   [[mouse_borders.machines]]
   name = "laptop"
   slot = 1
   address = "192.168.1.20"       # optional; otherwise "laptop" is looked up by name
   ```

2. **Pair the machines** with a security key. Create it on one machine and copy it to the others:

   ```bash
   aujar mwb key generate                 # on the first machine; prints the key
   aujar mwb key set K7QD-2XJP-...        # on every other machine
   ```

3. **Check** with `aujar mwb status`. Machines connect on their own (port 15101).

4. **Give your user access to the input devices** (see [Permissions](#permissions)), log out and
   in, and push the pointer through the edge of your screen.

## What works

| Mouse Without Borders feature | Aujar | Notes |
|---|---|---|
| Control up to 4 machines | ✅ | Any machine can be the one whose keyboard and mouse you use |
| Move across screen edges | ✅ X11 · ⚠️ Wayland | Wayland cannot report the pointer position: use the shortcuts there ([details](#wayland)) |
| Shared clipboard (text, images) | ✅ | Follows you when you cross over; needs `wl-clipboard` or `xclip` |
| File transfer, 100 MB limit | ✅ CLI | `aujar mwb send laptop file…`. Drag-and-drop and paste-a-file are 📅 |
| Service mode (login screen, elevated windows) | ✅ | A least-privilege system service ([below](#service-mode)) |
| Multi-machine input mode | ✅ | `Ctrl+Alt+M` |
| Device layout: row or 2×2 | ✅ config · 📅 visual editor | Edit `matrix` and `slot`; no drag-and-drop editor yet |
| Wrap mouse | ✅ | `wrap_mouse = true` |
| Easy Mouse (always / hold Shift / hold Ctrl / off) | ✅ | `easy_mouse = …`, toggle with `Ctrl+Alt+E` |
| Block mouse at corners | ✅ | `block_corners = true` |
| Hide mouse at the edge | ✅ X11 only | Hiding the pointer needs X11 |
| Move mouse relatively | ✅ | `move_relatively = true` |
| Security key | ✅ | Generated, never typed ([Security](#security)) |
| Same subnet only | ✅ | On by default |
| Shortcuts | ✅ | `Ctrl+Alt+` `1`–`4` jump, `E` easy mouse, `L` lock all, `R` reconnect, `M` multi-input |
| Screen saver control | ✅ | Machines stay awake while any of them is in use |

## Shortcuts

All use `Ctrl+Alt`. Letters are physical key positions (QWERTY), so they work on any layout.

| Keys | Does |
|---|---|
| `Ctrl+Alt+1` … `4` | Jump to the machine in that slot (`1` is slot 0). Works from any machine, even one you are controlling |
| `Ctrl+Alt+E` | Turn switching by mouse on or off |
| `Ctrl+Alt+L` | Lock every connected machine |
| `Ctrl+Alt+R` | Drop all connections and reconnect |
| `Ctrl+Alt+M` | Multi-machine input: your input goes to every machine at once. Press again to stop |

Change the letters under `[mouse_borders.hotkeys]`. `switch_keys = "function_keys"` uses `F1`–`F4`
instead, but Linux reserves `Ctrl+Alt+F1`–`F7` for virtual terminals, so the desktop switches
away when you press them on the machine you are sitting at.

**Always a way home:** `Ctrl+Alt+<your machine's slot>` takes the keyboard and mouse back, and
if the machine you are controlling disconnects, input returns to you immediately.

## Command line

```text
aujar mwb status              # state, machines, recent events, problems
aujar mwb key generate|show|set
aujar mwb switch laptop       # or `local`
aujar mwb easy-mouse on|off
aujar mwb multi-input on|off
aujar mwb lock-all
aujar mwb reconnect
aujar mwb send laptop a.pdf b.png
aujar mwb reload              # re-read settings and key now
```

`mwb` is short for `mouse-borders`. Settings and the key file are also re-read every two
seconds, so editing the file is enough.

## Permissions

Linux has no portable way to read or inject input without privileges, so Mouse Borders uses
the kernel's input layer directly (this works on X11 *and* Wayland):

* **Capture** reads `/dev/input/event*`, which needs the **`input` group**.
* **Injection** writes `/dev/uinput`. Install `packaging/udev/70-aujar-mouse-borders.rules`
  (the package does) and make sure the `uinput` module loads (`packaging/modules-load`).

```bash
sudo usermod -aG input "$USER"      # then log out and back in
```

> **What `input` membership means.** Any program running as you can then read every keystroke on
> the machine, not only Aujar. That is how Linux input access works, and the main cost of this
> feature. If that is not acceptable, run only the [system service](#service-mode) under its own
> `aujar-mwb` user and keep your own account out of the group.

`aujar mwb status` tells you exactly what is missing (no permission, no `uinput`, no
`wl-clipboard`…). A machine that cannot capture input can still be *controlled* by others.

## Security

* **Pairing.** The security key is generated (160 bits) and shown as eight groups of four. It
  lives in `~/.config/aujar/mouse-borders.key` with mode `0600`; a more permissive file is
  refused. Keys cannot be typed freely: a short password could be guessed offline.
* **Authentication and encryption.** Every connection runs a Noise `XXpsk3` handshake keyed from
  the security key, then ChaCha20-Poly1305 with a strict message counter. A machine without the
  key cannot complete a connection; modified, replayed or reordered messages are rejected and
  end the connection. Fresh keys per connection give forward secrecy.
* **Layout allow-list.** Even with the key, a machine not in your `machines` list is refused.
* **Same subnet only** (default): connections from outside the local interfaces' subnets are
  dropped before the handshake. Loopback is always allowed.
* **Off by default.** Nothing listens until `enabled = true`.
* **Files.** Received names are reduced to a single safe component, nothing is overwritten
  (`name (1).ext`), data lands in a `.part` file that only gets its final name after the size
  and SHA-256 match, the limit is 100 MB, and files are never made executable. They go to
  `~/Downloads/Aujar` (`receive_dir`).
* **Clipboard.** Shared only with the machine you are crossing to, never broadcast. Content the
  source application marks as a password-manager secret is never read. `share_clipboard = false`
  turns it off.
* **Remote lock** can be refused with `allow_remote_lock = false`.
* **What a paired machine can do.** The key is the trust boundary: a connected machine can type
  and click on yours while you are crossing to it. Only pair machines you control, and use
  `aujar mwb key generate --force` (then `key set` everywhere) to cut off a lost one.
* **Administrators** can forbid the feature with the `control_input` and `network` capabilities
  in [policy.toml](POLICY.md).

## Wayland

Wayland compositors deliberately do not let applications read the pointer position, which
edge switching needs. On Wayland:

* Keyboard shortcuts (`Ctrl+Alt+1…4`) and every command work as usual.
* Switching at screen edges is reported as unavailable in `aujar mwb status`.
* The screen size cannot be queried: set `screen = "1920x1080"` (the DRM outputs are used as a
  best guess otherwise, assuming monitors sit side by side).
* The absolute pointer placement uses a virtual tablet; on some multi-monitor compositors it
  maps to a single output. `move_relatively = true` avoids that.

A Wayland backend using the *InputCapture* portal is on the [roadmap](ROADMAP.md).

## Service mode

For the login screen, lock screen and elevated windows, run Mouse Borders as a system
service. It is a separate, optional process: stop the per-user daemon's Mouse Borders first
(`enabled = false` in your settings), because both listen on the same port.

```bash
sudo systemd-sysusers                                  # creates the aujar-mwb user (in `input`)
sudo install -d /etc/aujar
sudo install -m 644 ~/.config/aujar/settings.toml /etc/aujar/settings.toml
sudo install -o aujar-mwb -m 600 ~/.config/aujar/mouse-borders.key /etc/aujar/mouse-borders.key
sudoedit /etc/aujar/settings.toml                      # set receive_dir = "/var/lib/aujar-mwb/received"
sudo systemctl enable --now aujar-mouse-borders
```

The unit runs unprivileged, with no capabilities, a closed device policy (only `uinput` and input
nodes), a read-only filesystem and a syscall filter. It has no clipboard (there is no desktop
session to share one with). `loginctl lock-sessions` needs a polkit rule if you want remote lock
to work for that user. Logs: `journalctl -u aujar-mouse-borders`.

## Troubleshooting

| Symptom | Cause and fix |
|---|---|
| `waiting for a security key` | Run `aujar mwb key generate` or `key set` |
| `not in mouse_borders.machines` | `machine_name` (or the hostname) must match one entry's `name` |
| Machines never connect, `last_error` mentions the key | Different keys: run `key show` on both and compare |
| `outside this machine's subnets` | Different networks, or a VPN. Set `same_subnet_only = false` if that is intended |
| `cannot listen … in use` | Another Aujar instance (user daemon or system service) owns the port |
| `no permission to read /dev/input` | Join the `input` group and log in again |
| `cannot create the virtual keyboard` | Load `uinput` and install the udev rule, then re-plug or `sudo udevadm trigger` |
| Pointer does not switch at edges, shortcuts do | Wayland (above), or `easy_mouse` is off or needs a modifier |
| Keys stuck after switching | Report it; `aujar mwb reconnect` clears state on both ends |
| Clipboard does not follow | Install `wl-clipboard` (Wayland) or `xclip` (X11) |

## Limitations

* Aujar to Aujar on Linux; keys are Linux key codes, so other operating systems are out.
* Touchpads: motion and physical buttons work while controlling another machine; tap-to-click,
  scrolling and gestures are done in software by the desktop and do not cross over.
* Multi-monitor: each machine is treated as one rectangle covering its monitors. Gaps in an
  L-shaped arrangement can land the pointer off-screen unless `move_relatively` is on.
* Edge switching and `hide_mouse_at_edge` need X11. The virtual-terminal shortcuts are not
  forwarded.
* No visual layout editor, drag-and-drop or paste-a-file transfer yet.
* This was developed against in-memory and loopback tests plus two real daemons on one host. The
  evdev, uinput and X11 code paths could not be exercised on physical hardware in CI: expect
  to report some device-specific bugs.

## How it works

`modules/mouse-borders` is built so everything that decides anything is plain, tested logic and
everything that touches the desktop is behind a trait:

| Piece | Role |
|---|---|
| `router` | Where the pointer lives, edge crossings, Easy Mouse, corners, wrap, resolution mapping |
| `layout`, `keys` | The arrangement; `Ctrl+Alt` shortcuts |
| `protocol`, `secure` | Versioned binary messages; the Noise channel |
| `engine`, `link` | One actor owning all state; connections with two lanes (input first, files after) |
| `transfer`, `clipboard` | Safe file receive and clipboard reassembly |
| `service` | Watches settings and key, runs or restarts the engine, is the runtime `Module` |
| `backend` | Traits, in-memory backends for tests, and `backend::linux` (evdev, uinput, X11, CLI clipboard) |

While you control another machine, the controlling side tracks a *virtual* cursor over that
machine's screen, so switching back never depends on the remote pointer acceleration.
