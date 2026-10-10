# Keyboard Shortcuts

## Native shortcut (automatic)

The daemon asks your desktop for a global shortcut through the `GlobalShortcuts` portal,
suggesting **Ctrl+Alt+Space**. On first start your desktop may show a dialog (or a new entry in
its shortcut settings) where you accept or change the key. The portal is provided by KDE Plasma,
GNOME 48 or newer, and Hyprland's portal backend; it is not provided by `xdg-desktop-portal-wlr`
(Sway) or on X11-only setups, and older GNOME versions lack it. Check yours.

If the portal is missing, the daemon logs `global shortcuts unavailable` and keeps working: use
the manual binding below.

GNOME and some other backends identify the app by its desktop file. Install it once for source
builds (packages do this for you):

```bash
install -Dm644 packaging/desktop/dev.aujar.Aujar.desktop ~/.local/share/applications/dev.aujar.Aujar.desktop
```

Disable native shortcuts by denying `global_hotkeys` in the [policy](POLICY.md).

## Manual binding (works everywhere)

Use a manual binding **instead of** the native one, or on different keys: two bindings on the same
key conflict. Not working? See [TROUBLESHOOTING](TROUBLESHOOTING.md) or run `aujar doctor`.

Linux has no universal way for an app to grab a global hotkey, especially on Wayland. Until Aujar
implements the portal `GlobalShortcuts` API and X11 key grabs natively, bind **your desktop's own
shortcut** to a command. It works on every compositor, because the compositor owns the key.

```bash
aujar launcher toggle      # show/hide the launcher UI (also: show, hide)
```

The command sends a `Launcher` request to the daemon, which pushes an event to the running UI
(see [IPC](IPC.md)). If no UI is subscribed, the command prints a notice and does nothing else.

| Environment | How to bind `Super+Space` |
|---|---|
| Sway / i3 | `bindsym $mod+space exec aujar launcher toggle` |
| Hyprland | `bind = SUPER, SPACE, exec, aujar launcher toggle` |
| GNOME | Settings → Keyboard → View and Customize Shortcuts → Custom Shortcuts → add command `aujar launcher toggle` |
| KDE Plasma | System Settings → Keyboard → Shortcuts → Add New → Command or Script |
| Other | Use your desktop's custom-shortcut settings with the same command |

Note: `Super+Space` may already be taken (for example by input-method switching); pick any free
combination.

Make sure `aujar` is on the `PATH` seen by your compositor, or use its absolute path.
