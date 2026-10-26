# integrations

Sources of other programs that Aujar integrates but does not compile.

| Path | What | How it is used |
|---|---|---|
| `nexdesk/` | [NexDesk](https://github.com/4mritGiri/nexdesk), remote-access suite (git submodule) | Built separately with `scripts/build-nexdesk.sh`; started by `modules/nexdesk` as a separate process. See [docs/NEXDESK.md](../docs/NEXDESK.md). |

Add the submodule once with `scripts/add-nexdesk-submodule.sh`. After cloning Aujar, fetch it with
`git submodule update --init`.

This directory is listed under `exclude` in the root `Cargo.toml`: Aujar's workspace never builds
or lints anything in here.
