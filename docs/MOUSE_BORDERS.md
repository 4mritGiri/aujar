# Mouse Borders

Share one keyboard and mouse between up to four machines running **Linux, macOS or Windows**:
move the pointer off the edge of one screen and it appears on the next machine, with the clipboard
and files following you. It is Aujar's take on Microsoft's *Mouse Without Borders*, and every
combination of the three systems works together.

> **Aujar to Aujar only.** Mouse Borders speaks its own encrypted protocol. It does not talk to
> Microsoft's Mouse Without Borders, and (like all of Aujar) it is not affiliated with Microsoft.
> Linux machines can use the Aujar app or the standalone agent; macOS and Windows machines use
> the standalone **`aujar-mwb`** agent.

## Platforms

| | Linux | macOS | Windows |
|---|---|---|---|
| Capture input | evdev (`input` group) | event tap (Input Monitoring + Accessibility) | low-level hooks |
| Control this machine | uinput | Quartz events (Accessibility) | `SendInput` |
| Switch at screen edges | X11 (not Wayland) | yes | yes |
| Jump with `Ctrl+Alt+1..4` | yes | yes (`Control+Option`) | yes |
| Clipboard (text, images) | `wl-clipboard` / `xclip` | built in | built in |
| Lock all machines | `loginctl` | `CGSession` / `pmset` | `LockWorkStation` |
| Start at login | systemd user unit | LaunchAgent | Run registry key |
| Login screen / elevated windows | [system service](#service-mode) | not possible | not possible (needs an elevated agent) |
| **How well verified** | tested, smoke-tested | **compiled and unit-tested only** | **compiled and unit-tested only** |

> **Read this before relying on macOS or Windows.** Those backends are real code that compiles for
> their platforms and whose decision logic (key tables, event translation, motion handling,
> scrolling, click counting) is unit-tested. But the development machine was Linux: nobody has run
> the Windows hooks or the macOS event tap yet. The CI workflow builds and tests on real Windows
> and macOS runners; run `aujar-mwb doctor` on each machine and expect to report some bugs.

## Quick start

On every machine install Aujar and make sure the daemon runs (`systemctl --user enable --now aujar`).
Then pair them. In the settings window nothing has to be copied, sent or pasted: the two
machines find each other on the network and you compare a short code on both screens.

**In the settings window** (open the launcher, then Settings → **Mouse Borders**):

1. On the first machine choose **Start pairing**. This turns Mouse Borders on, creates the key,
   adds the machine to the layout and makes it visible on the network for two minutes.
2. On the second machine choose **Find devices**, then **Pair** next to the first machine.
3. Both screens now show the same eight digits (`4821 0937`). If they match, choose
   **Codes match** on both; if they differ, choose **They differ** (someone else may be on the
   network). Only after both of you confirm does the key travel, over the encrypted connection,
   and the second machine adopts the key and the layout.
4. For a third or fourth machine repeat steps 1 to 3 with any machine that is already paired.
5. Check the **Status** card at the top of the page: machines show as connected.
6. **Drag the machines** in **Layout** to where they sit on your desk (the board is 4 columns by
   3 rows, like the display arrangement in your system settings; dropping on another machine
   swaps them), give your user access to the input devices ([Permissions](#permissions)), and
   push the pointer through a screen edge. Machines in the same row switch left and right,
   machines in the same column switch up and down.

Pairing needs the machines on one network that lets broadcasts through (UDP 15101) and TCP
15102 between them (the sharing port plus one). On a firewalled machine
(`ufw allow from 192.168.1.0/24 to any port 15101,15102`) or a guest Wi-Fi with client isolation
**Find devices** finds nothing; use the pairing code below instead.

**Pairing code (no discovery).** **Copy pairing code** on one machine and **Join from clipboard** on
the other does the same job: the code is one line holding the key and the layout, so you never
type a key or an address. Send it yourself (it is a secret: anyone who sees it can connect to
your machines). With three or four machines, send the *latest* code to every machine that is
already part of the layout.

**From the command line** it is the same flow:

```bash
aujar mwb pair                       # first machine: prints the code
aujar mwb join 'aujar-pair1;row;…'   # other machines
aujar mwb status
```

`join` refuses to replace a different security key unless you add `--force`, and a pairing code
contains the key: treat it like a password and do not paste it anywhere public.

**Or write the settings by hand** in `~/.config/aujar/settings.toml` (the same text works on every
machine, see [the example](../config/mouse-borders.example.toml)):

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

and create the key with `aujar mwb key generate` on one machine and `aujar mwb key set <KEY>` on the
others.

### The standalone agent (Windows, macOS, or Linux without the Aujar app)

`aujar-mwb` is one small program that does everything: it *is* the agent (`aujar-mwb run`) and the
command line for it. Download it from the release or build it (`cargo build --release -p aujar-mwb`).

```bash
aujar-mwb pair                  # first machine: prints a pairing code
aujar-mwb join 'aujar-pair1;…'  # every other machine
aujar-mwb doctor                # checks permissions, network, key; says how to fix problems
aujar-mwb run                   # start sharing (leave it running)
aujar-mwb autostart enable      # start it at every login
aujar-mwb status                # who is connected, who has the keyboard
```

Every other command (`switch`, `send`, `lock-all`, `reconnect`, `easy-mouse`, `multi-input`, `key …`)
works the same as `aujar mwb …` below. The agent keeps its settings in the same
`[mouse_borders]` table of the Aujar settings file:

| OS | Settings file |
|---|---|
| Linux | `~/.config/aujar/settings.toml` |
| macOS | `~/Library/Application Support/Aujar/settings.toml` |
| Windows | `%APPDATA%\Aujar\settings.toml` |

## What works

| Mouse Without Borders feature | Aujar | Notes |
|---|---|---|
| Control up to 4 machines | ✅ | Any machine can be the one whose keyboard and mouse you use |
| Move across screen edges | ✅ X11 · ⚠️ Wayland | Wayland cannot report the pointer position: use the shortcuts there ([details](#wayland)) |
| Shared clipboard (text, images) | ✅ | Follows you when you cross over; needs `wl-clipboard` or `xclip` |
| File transfer, 100 MB limit | ✅ CLI | `aujar mwb send laptop file…`. Drag-and-drop and paste-a-file are 📅 |
| Service mode (login screen, elevated windows) | ✅ | A least-privilege system service ([below](#service-mode)) |
| Multi-machine input mode | ✅ | `Ctrl+Alt+M` |
| Device layout: row, 2×2 or free board | ✅ | Settings window (drag machines on a 4 × 3 board) or `matrix` and `slot` in the config |
| Pair over the network | ✅ | Settings window: Start pairing / Find devices, compare an 8-digit code |
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
aujar mwb pair                # print a pairing code for your other machines
aujar mwb join '<code>'       # join the machines of a pairing code
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

### macOS

macOS asks for two permissions the first time the agent runs, in **System Settings → Privacy &
Security**: *Input Monitoring* (to see the keyboard and mouse) and *Accessibility* (to hold events
back and to let other machines control this one). Allow `aujar-mwb` in both, then restart it. The
permission belongs to the program's *path*: move or rebuild it and macOS asks again. macOS also
stops delivering keystrokes to any program while a password field has focus, so passwords typed
there never cross over.

### Windows

No permission prompts, but Windows only shows input to a program that is at least as privileged as
the window receiving it: to share elevated windows (Task Manager, installers) run `aujar-mwb` as
administrator. The lock screen, UAC prompts and Ctrl+Alt+Del are out of reach. The first time the
agent listens, Windows Defender Firewall asks whether to allow it on private networks: allow it.

`aujar mwb status`/`aujar-mwb status` tells you exactly what is missing (no permission, no `uinput`, no
`wl-clipboard`…). A machine that cannot capture input can still be *controlled* by others.

## Checking your setup and the audit log

```bash
aujar-mwb doctor        # (or `aujar mwb doctor`) is this machine ready? what to do if not
aujar-mwb audit -n 30   # the last 30 audit entries
```

`doctor` checks the layout, the key, the network, the port, the folders and the platform's
permissions, and prints a concrete fix for each problem; it exits with an error if anything
blocks Mouse Borders, so scripts and deployment tools can use it.

The **audit log** (`mouse-borders-audit.log` in the state directory, JSON lines) records who
connected or was refused and why, when control moved between machines, when another machine took
control of this one, clipboard transfers (kind and size), files offered, received or refused,
remote locks (and whether they were allowed), and key changes. It never records clipboard
contents, keystrokes, file contents or the key. It is private to your user, rotates at 5 MiB
(three old files are kept), and can be switched off with `audit_log = false`.

## Security

* **Pairing.** The security key is generated (160 bits) and shown as eight groups of four. It
  lives in `~/.config/aujar/mouse-borders.key` with mode `0600`; a more permissive file is
  refused. Keys cannot be typed freely: a short password could be guessed offline.
* **Pairing over the network.** The two machines run an unauthenticated Noise `NN` handshake and
  both show an 8-digit code derived from it; you compare them. A machine in the middle has a
  different handshake with each side, so the codes differ. The handshake has 10 seconds to finish,
  which rules out trying keys until the codes happen to match. The pairing code with the key is
  sent only after *both* people confirmed, and only inside the encrypted channel. The pairing
  window is open for two minutes, answers discovery only to machines on its own subnets and stops
  after one pairing. Nothing listens for pairing outside that window.
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
* **Audit trail.** Connections, refusals, hand-offs, transfers, locks and key changes are logged
  ([above](#checking-your-setup-and-the-audit-log)).
* **Remote lock** can be refused with `allow_remote_lock = false`.
* **Local control channel.** The agent's command line talks to it over a socket in your runtime
  directory (mode `0600`, other users refused) or a per-user Windows named pipe that refuses
  remote clients.
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

* Aujar to Aujar. Keys travel as physical-key positions (USB HID), so the receiving machine's
  keyboard layout decides what is typed; keys a platform lacks (Print Screen on a Mac) are not forwarded.
* macOS and Windows are compiled and unit-tested, **not yet run on real machines** ([Platforms](#platforms)).
* Scrolling follows each system's own direction setting ("natural scrolling"), so a scroll may feel
  reversed on one of the machines.
* The login screen and elevated windows can only be shared on Linux (service mode) or with an
  elevated Windows agent.
* Touchpads: motion and physical buttons work while controlling another machine; tap-to-click,
  scrolling and gestures are done in software by the desktop and do not cross over.
* Multi-monitor: each machine is treated as one rectangle covering its monitors. Gaps in an
  L-shaped arrangement can land the pointer off-screen unless `move_relatively` is on.
* Edge switching and `hide_mouse_at_edge` need X11. The virtual-terminal shortcuts are not
  forwarded.
* No drag-and-drop or paste-a-file transfer yet. Two machines are neighbours only when they share a
  row or a column of the layout board (a machine placed diagonally has no edge towards the other).
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
| `backend` | Traits and in-memory backends for tests; `backend::linux` (evdev, uinput, X11), `backend::windows` (hooks, `SendInput`), `backend::macos` (event tap, Quartz) |
| `keymap`, `input` | One table of physical keys (USB HID, Linux, Windows, macOS codes) and the neutral button type |
| `backend::winput`, `backend::macinput` | The Windows and macOS decisions, free of OS APIs so they are tested everywhere |
| `control` | The agent's local control channel (Unix socket / Windows named pipe) |
| `audit`, `doctor`, `autostart` | Audit log, preflight checks, start-at-login for each OS |

While you control another machine, the controlling side tracks a *virtual* cursor over that
machine's screen, so switching back never depends on the remote pointer acceleration.
