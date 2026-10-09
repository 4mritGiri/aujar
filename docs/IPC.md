# IPC Protocol (v1)

Transport: Unix domain socket (`$XDG_RUNTIME_DIR/aujar.sock`, mode `0600`). Only the daemon's
user (and root) may connect.

Framing: **one JSON object per line** (UTF-8, `\n` terminated), each wrapped in an envelope:

```json
{"version":1,"payload": ... }
```

Requests are limited to 64 KB and must arrive within 5 seconds of connecting.
Unknown protocol versions are rejected. New request/response/event variants are added without
bumping the version; clients must tolerate variants they do not know.

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
| `Launcher("Show"\|"Hide"\|"Toggle")` | `Delivered{subscribers}` | — |
| `Subscribe` | `Subscribed`, then events | — |

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
| `Shutdown` | Daemon is exiting; the stream ends after this event |

Limits: at most 16 concurrent subscribers; a subscriber that falls more than 64 events behind
loses the oldest events (logged by the daemon).

`aujar events` prints the stream; `aujar launcher toggle` produces events.

## UI integration recipe

1. UI starts and calls `subscribe(socket)`.
2. On `Launcher(Toggle)` it shows or hides its window.
3. On user input it sends `Search`; on selection it sends `Execute`.
4. On `Shutdown` or EOF it exits, or reconnects with backoff when the daemon restarts.
