# Lookups, window switching, drag-out

## Lookups (dictionary, weather, stocks, sports)

Each lookup runs only when you type its trigger, sends only what you typed to one service,
and can be switched off in Settings (`[plugins] dictionary / weather / stocks / sports`).

| Trigger | Service | Notes |
|---|---|---|
| `define word`, `meaning of word`, `what does word mean`, Ctrl+L | dictionaryapi.dev | English. Enter opens the entry on Wiktionary. |
| `weather paris`, `weather in new york`, `oslo forecast` | Open-Meteo (geocoding + forecast) | No key. `[weather] imperial = true` for °F and mph. |
| `stock aapl`, `$tsla`, `msft share price`, `quote brk-b` | Yahoo Finance chart endpoint | Unofficial and keyless: it may rate-limit or change. Delayed quotes, for information only. |
| `nba`, `nfl scores`, `premier league`, `nba lakers`, `ucl` | ESPN public scoreboard | Unofficial and keyless. Leagues: NBA, WNBA, NFL, MLB, NHL, Premier League, La Liga, Bundesliga, Serie A, Ligue 1, MLS, Champions League. |

How it stays fast: a keystroke never waits on the network. The provider answers at once from
its cache, or with a "Looking up..." placeholder while a worker thread runs `curl`; the UI asks
again every 450 ms (up to about 20 s) and the real answer replaces the placeholder. Failures
(offline, rate limited) are shown as a message, not hidden. Requests are HTTPS only, use `curl`
(no shell), and inputs are validated and percent-encoded.

## Window switching

`< firefox`, `win term`, `switch to mail`, or `<` alone for every window.

| Session | Backend | Setup |
|---|---|---|
| GNOME (Wayland or X11) | Aujar Windows Shell extension over D-Bus (`dev.aujar.Windows`) | `aujar gnome-extension install`, then log out and back in. `aujar gnome-extension status` checks it. |
| Sway | `swaymsg -t get_tree` | none |
| Hyprland | `hyprctl clients -j` | none |
| X11 window managers | `wmctrl` | install `wmctrl` |

GNOME gives applications no view of other windows, so the extension runs inside the shell. It
exposes only `List()` and `Activate(id)`: it can read window titles/app names and focus a
window; it cannot close, move or type into windows. Any process in your session can call it, so
window titles are visible to your own session's programs (the same trust boundary as the
session bus). Remove it with `aujar gnome-extension remove`.

Not verified on real desktops: the parsers are unit-tested against sample output of each tool,
but only a real session can confirm each backend end to end.

## Drag a file out

File and folder results can be dragged into a file manager, chat or mail window (GPUI hands the
drag to the compositor when the pointer leaves the launcher). This is implemented for Wayland;
on X11 GPUI has no outbound drag, so dragging does nothing outside the window. While a drag is
in flight the window ignores focus loss, and closes itself 30 s after the drag leaves.

## Actions menu

Right-click a result, or press Ctrl+K / Shift+F10: Open (Enter), Show in Folder (Ctrl+Enter),
Copy (Ctrl+C), Look Up (Ctrl+L), Search the Web (Ctrl+B).
