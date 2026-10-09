# Keyboard Shortcuts

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
