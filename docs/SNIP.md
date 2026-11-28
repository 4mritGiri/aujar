# Aujar Snip

Aujar's own snipping tool. The screen freezes, you drag an area, then copy it, copy the text in
it, or save it.

```bash
aujar snip allow                # GNOME, once: let Aujar take screenshots without asking
aujar snip                      # freeze the screen and select (needs the Aujar UI running)
aujar snip shortcut-install     # GNOME: Super+Shift+S runs `aujar snip` (or --keys CTRL+SHIFT+S)
```

## In the overlay

The screen freezes and dims. A toolbar at the bottom picks how you select:

| Tool | Does |
|---|---|
| **Selection** (1) | drag an area; a magnifier shows the pixel's position and color |
| **Screen** (2, Ctrl+A) | the whole monitor |
| **Window** (3) | hover a window (it lights up with its name), click to take exactly it. Needs the Aujar Shell extension on GNOME (`aujar gnome-extension install`, then log out and back in); Sway and Hyprland work without it |
| **OCR Extract** (O) | capturing copies the text instead of the picture |
| round button, Enter, double-click | capture: copy the picture (or its text) |

Under a selection, a bar offers:

| Button | Key | Does |
|---|---|---|
| Copy | Ctrl+C | the picture to the clipboard |
| Extract Text | T | the text in it (Tesseract, see [TEXT_EXTRACTOR](TEXT_EXTRACTOR.md)) |
| Auto-Redact | R | covers emails, AWS/Stripe/GitHub/Slack/GitLab/Google keys, JWTs, long random tokens, card numbers, and whatever follows `PASSWORD:` / `SECRET=` / `token:` on the same line, with solid boxes; shown in place, so check it before copying. Again (Undo Redact) removes it |
| Save | Ctrl+S | a PNG in `~/Pictures/Screenshots/Snip <date time>.png` |

Drag a handle or inside the selection to resize or move it; arrows nudge (Shift: 10 px),
Ctrl+arrows resize. Right-click clears the selection, then closes; Esc closes. Results come as a
notification.

Auto-Redact uses text recognition, which can miss things (small or stylised text, unusual
secrets): always look at the result before sharing.

`aujar text` (Text Extractor) opens the same overlay with OCR Extract on: the text of the area is
copied as soon as you let go. With options (`--lang`, `--single-line`, `--no-copy`), or without
the UI, it uses the desktop's area selection instead.

## How the screen is captured

The daemon takes one picture of every monitor without asking you to select anything
(`CaptureScreen`, capability `screen_capture`), lists the visible windows at the same moment, and
(on Windows and macOS) says where each monitor is in the picture. The overlay crops to its own
monitor exactly, also when monitors have different scale factors. The `aujar-screen` crate holds
the backends:

| Platform | Picture | Windows (window mode) | Permission |
|---|---|---|---|
| Linux, GNOME / KDE | Screenshot portal, non-interactive | Aujar Shell extension (GNOME) | GNOME: once, `aujar snip allow` |
| Linux, Sway / Hyprland / wlroots | `grim` | `swaymsg` / `hyprctl` | none |
| Linux, other X11 | `maim` or `scrot` | — | none |
| Windows | GDI copy of the virtual screen, per-monitor DPI aware | `EnumWindows` + DWM frame bounds | none |
| macOS | CoreGraphics, one image per display | window server list | Screen Recording, once (macOS asks) |

**Windows and macOS status:** the capture backends, the Windows daemon (named pipe, event
stream, `CaptureScreen`) and the CLI build and pass their unit tests for those targets, but have
not been run on real machines yet; the UI is built by CI there for the first time. Text
recognition uses Windows OCR and Apple Vision there (Tesseract for other languages; see
[TEXT_EXTRACTOR](TEXT_EXTRACTOR.md)). On Windows bind a shortcut to `aujar snip`
yourself for now (global hotkeys are not built in there).

The picture goes to an owner-only file in `$XDG_RUNTIME_DIR`, which the UI reads and deletes at
once; the overlay keeps it in memory only. The portal itself first saves it in your Pictures
folder; Aujar moves it out immediately (your desktop's file indexer may notice it briefly).

**GNOME needs your permission once.** The daemon identifies itself to the portal as
`dev.aujar.Aujar`, and GNOME only lets a known app take a screenshot without asking when that app
is allowed. GNOME asks with a dialog, but only the focused app may show one, and the daemon has no
window, so the request is refused ("the desktop has not allowed Aujar to take screenshots").
Run `aujar snip allow` once (or press *Allow* under Settings > Text Extractor): it stores the
same entry GNOME's *Allow* button would, in the portal permission store (`screenshot` table).
`aujar snip allow --revoke` removes it; `aujar doctor` shows the state. Until then `aujar text`
uses the desktop's own selection instead of the overlay.

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
  PNG). Free-form shapes, a delay timer, annotation (Edit), scrolling capture and pinning are not
  there yet.
- Window mode on GNOME needs the updated extension (window positions were added for it).
- Tested with stand-in tools and unit tests; check it on your desktops.
