# Text Extractor

Select part of the screen and get the text in it on the clipboard, like PowerToys' Text
Extractor. The text is recognised on this computer by [Tesseract](https://github.com/tesseract-ocr/tesseract);
nothing is sent anywhere.

```bash
aujar text                       # drag over some text; it is printed and copied
aujar text --lang nep            # Nepali (or eng+nep for both)
aujar text --single-line         # join wrapped lines into one
aujar text --file shot.png       # read a picture instead of the screen (no daemon needed)
aujar text --no-copy             # print only
aujar text languages             # installed Tesseract languages
aujar text shortcut-install      # GNOME: Super+Shift+T runs `aujar text` (or --keys CTRL+ALT+T)
```

Started from a keyboard shortcut there is no terminal, so the result (or the reason it failed)
comes as a desktop notification (`notify-send`).

## Install

Tesseract and at least one language:

| Distribution | Command |
|---|---|
| Debian / Ubuntu | `sudo apt install tesseract-ocr` (English included), more: `tesseract-ocr-nep`, `tesseract-ocr-deu`, ... |
| Fedora | `sudo dnf install tesseract` (more: `tesseract-langpack-nep`, ...) |
| Arch | `sudo pacman -S tesseract tesseract-data-eng` (more: `tesseract-data-nep`, ...) |

`aujar doctor` shows whether Tesseract runs and which languages it has.

## Selecting an area

When the Aujar UI is running, `aujar text` (without options) opens Aujar's own overlay,
[Aujar Snip](SNIP.md), in text mode: the screen freezes, you drag, and the text is copied when you
let go. On GNOME this needs `aujar snip allow` once (see [SNIP](SNIP.md)); until then the desktop's
selection is used. Set `[snip] text_extractor = false` to always use the desktop's selection below.

Otherwise (no UI, or with `--lang`, `--single-line` or `--no-copy`) Aujar uses the desktop's own
area selection:

| Desktop | Backend | What you see |
|---|---|---|
| GNOME, KDE (Wayland or X11) | `portal` | The desktop's interactive screenshot dialog: choose an area there |
| Sway, Hyprland, river, Wayfire, labwc | `grim` | `slurp`'s crosshair: drag an area (needs `grim` and `slurp`) |
| Other X11 | `maim` or `scrot` | A crosshair: drag an area |

`auto` (the default) picks from the session. Press Esc to cancel; that is not an error.

The picture is enlarged and turned gray before recognition (screen text is small), and deleted as
soon as it is read, including the copy the screenshot portal saves (often in `~/Pictures`).

## Settings

The **Text Extractor** page of the settings window shows whether Tesseract works, lets you pick
the languages (one pill per installed language), the area selection and one-line mode, assigns
the GNOME shortcut, and has an *Extract text now* button. The same settings live in
`~/.config/aujar/settings.toml`:

```toml
[text_extractor]
languages = ["eng"]        # each must be installed: `aujar text languages`
single_line = false
backend = "auto"           # auto, portal, grim, maim, scrot
shortcut = "SUPER+SHIFT+T" # used by `aujar text shortcut-install`
```

More languages are slower, and similar scripts can be confused; list only the ones you read.

## How it works

`aujar text` sends `ExtractText` to the daemon (capability `screen_capture`, so an administrator
policy can deny it). The daemon lets you select the area, runs `tesseract stdin stdout -l <langs>`
with a 60-second limit (once more with page segmentation mode 6 when the first pass finds
nothing), tidies the output (trailing spaces, form feeds, extra blank lines), and answers
`Text{text, languages, cancelled}`. The command copies the text with `wl-copy`, `xclip` or `xsel`.
`--file` does the recognition in the command itself.

`AUJAR_TESSERACT_BIN` runs another Tesseract executable (tests, or one outside `PATH`).

## Limits

- Recognition quality is Tesseract's: clear, horizontal text on a plain background works best;
  handwriting, tiny or stylised text and busy backgrounds do not.
- What the portal dialog offers is up to the desktop; if it captures the whole screen, the text
  of the whole screen is recognised.
- Linux only so far (macOS and Windows: `--file` works, screen selection is not implemented).
