# IPC Protocol (v1)

Transport: Unix domain socket (`$XDG_RUNTIME_DIR/aujar.sock`, mode `0600`). Only the daemon's
user (and root) may connect.

Framing: **one JSON object per line** (UTF-8, `\n` terminated), each wrapped in an envelope:

```json
{"version":1,"payload": ... }
```

Requests are limited to 64 KB and must arrive within 5 seconds of connecting.

## Versioning

- `PROTOCOL_VERSION` (currently 1) is the version this build speaks; `MIN_PROTOCOL_VERSION` is
  the oldest one the daemon still answers. Both live in `aujar-ipc`.
- **Additive changes keep the version:** new request, response or event variants, and new fields
  marked `#[serde(default)]`. Clients must tolerate variants they do not know; a daemon answers an
  unknown request with `Error("unrecognised request ...")`.
- **Breaking changes bump the version:** renaming or removing a variant or field, or changing what
  one means. Keep the previous version in `MIN_PROTOCOL_VERSION` for at least one release.
- The daemon reads the envelope's `version` before the payload, and answers **in the client's
  version**. A client outside the supported range gets `Error` explaining the mismatch, still in
  its own version. `Error(String)` has the same shape in every version, so any client can read it.
- A client reads the version of every reply first, and reports a daemon outside its range as a
  mismatch ("install the same Aujar release for both and restart the daemon"), not as bad JSON.
- `Version` asks the daemon which versions it speaks. Older daemons answer it with
  `Error`; treat that as "v1 only". `aujar doctor` prints the result.

## Request / response

One request per connection, one response, then the daemon closes it.

| Request | Response | Capability |
|---|---|---|
| `Ping` | `Pong` | — |
| `Health` | `Health{...}` | — |
| `Modules` | `Modules{...}` | — |
| `Windows` | `Windows[...]` | `read_windows` |
| `Search{query, limit?}` | `Search{results}` | — |
| `Execute{id}` | `Executed{title, pid}` | `execute_command` |
| `Rename{...}` | `Rename{...}` | `filesystem` |
| `Status` | `Status{subscribers, hotkeys}` | — |
| `Version` | `Version{protocol, min_protocol, daemon}` | — |
| `PickColor` | `Color{hex, rgb, hsl}` (waits until the user picks or cancels) | `screen_capture` |
| `CaptureScreen` | `Screen{path}`: an owner-only PNG of the whole screen in `$XDG_RUNTIME_DIR`; the client reads and deletes it | `screen_capture` |
| `ExtractText{languages, single_line}` | `Text{text, languages, cancelled}` (waits until the user selects an area or cancels) | `screen_capture` |
| `Launcher("Show"\|"Hide"\|"Toggle"\|"Settings"\|"ColorPicker"\|"ColorEditor"\|"Snip"\|"SnipText")` | `Delivered{subscribers}` | — |
| `Subscribe` | `Subscribed`, then events | — |
| `MouseBorders(Status\|GenerateKey\|ShowKey\|SetKey\|Reload)` | `MouseBorders(Status\|Key\|NoKey\|Done)` | — |
| `MouseBorders(Switch\|SetEasyMouse\|LockAll\|Reconnect\|MultiMachineInput)` | `MouseBorders(Done{message})` | `control_input` |
| `MouseBorders(SendFiles{machine, paths})` (absolute paths) | `MouseBorders(Done{message})` | `filesystem` |

Any request can instead return `Error("message")`, including when policy denies its capability.

## Event stream

`Subscribe` turns the connection into a stream:

1. Daemon replies `{"version":1,"payload":"Subscribed"}`.
2. Daemon pushes one envelope per event: `{"version":1,"payload":{"Launcher":"Toggle"}}`.
3. The client sends nothing further. Closing the connection unsubscribes; sending data ends the
   stream.

| Event | Meaning |
|---|---|
| `Launcher(Show\|Hide\|Toggle)` | A trigger (keyboard shortcut, CLI) wants the launcher UI shown/hidden |
| `Launcher(ColorPicker\|ColorEditor)` | The color picker shortcut (starts the desktop color picker or the editor, as set up in settings), or the color editor |
| `Shutdown` | Daemon is exiting; the stream ends after this event |

Limits: at most 16 concurrent subscribers; a subscriber that falls more than 64 events behind
loses the oldest events (logged by the daemon).

`aujar events` prints the stream; `aujar launcher toggle` produces events.

## UI integration recipe

1. UI starts and calls `subscribe(socket)`.
2. On `Launcher(Toggle)` it shows or hides its window.
3. On user input it sends `Search`; on selection it sends `Execute`.
4. On `Shutdown` or EOF it exits, or reconnects with backoff when the daemon restarts.
