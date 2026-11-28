# Aujar Snip

Aujar's own snipping tool. The screen freezes, you drag an area, then copy it, copy the text in
it, or save it.

```bash
aujar snip                      # freeze the screen and select (needs the Aujar UI running)
aujar snip shortcut-install     # GNOME: Super+Shift+S runs `aujar snip` (or --keys CTRL+SHIFT+S)
```

## In the overlay

| Do | To |
|---|---|
| Drag | select an area (the size in pixels and a magnifier with the pixel's position and color follow the pointer) |
| Drag a handle / inside the selection | resize / move it |
| Arrows (Shift: 10 px) / Ctrl+Arrows | nudge / resize |
| Ctrl+A | select the whole screen |
| **Enter**, Ctrl+C, double-click | copy the picture |
| **T** | copy the text in it (Tesseract, see [TEXT_EXTRACTOR](TEXT_EXTRACTOR.md)) |
| **Ctrl+S** | save a PNG (`~/Pictures/Screenshots/Snip <date time>.png`) |
| Right-click | clear the selection; again to close |
| Esc | close |

After a selection, a bar under it has the same actions. Results come as a notification.

`aujar text` (Text Extractor) opens the same overlay in text mode when the UI is running: the
text of the area is copied as soon as you let go. With options (`--lang`, `--single-line`,
`--no-copy`), or without the UI, it uses the desktop's area selection instead.

## How the screen is captured

The daemon takes one picture of the whole screen without asking you to select anything
(`CaptureScreen`, capability `screen_capture`):

| Desktop | How |
|---|---|
| GNOME, KDE | Screenshot portal, non-interactive |
| Sway, Hyprland and other wlroots compositors | `grim` |
| Other X11 | `maim` or `scrot` |

The picture goes to an owner-only file in `$XDG_RUNTIME_DIR`, which the UI reads and deletes at
once; the overlay keeps it in memory only. The portal itself first saves it in your Pictures
folder; Aujar moves it out immediately (your desktop's file indexer may notice it briefly).

GNOME allows the non-interactive screenshot for programs it does not see as an app, such as the
Aujar services (`aujar.service`). Started from a terminal or the dash, GNOME may ask once (or
refuse when no Aujar window has focus); run the services: `systemctl --user enable --now aujar
aujar-ui`.

## Settings

```toml
[snip]
shortcut = "SUPER+SHIFT+S"  # used by `aujar snip shortcut-install`
folder = ""                 # where Ctrl+S saves; empty: ~/Pictures/Screenshots
text_extractor = true       # `aujar text` uses this overlay when the UI runs
```

The capture backend is `[text_extractor] backend` (`auto` picks from the session).

## Limits

- The overlay covers the monitor the desktop puts it on (usually the one you are using). With
  several monitors, the picture is cropped to that monitor from the desktop's layout; mixed
  scale factors across monitors may crop inaccurately.
- About half a second passes between the shortcut and the frozen screen (the portal writes a
  PNG). Window snipping, free-form shapes, a delay timer, annotation and pinning are not there yet.
- Tested with stand-in tools and unit tests; check it on your desktops.
