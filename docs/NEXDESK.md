# NexDesk integration

[NexDesk](https://github.com/4mritGiri/nexdesk) is a separate Rust remote-access suite: an RDP
client built on IronRDP, a GPUI connection manager, an isolated RDP engine process
(`nexdesk-rdp`), a password vault and a native remote-control stack. Aujar integrates it as a
**separate program**, not as linked code.

## What Aujar does

`modules/nexdesk` (`aujar-nexdesk`) is a normal runtime module (`aujar modules` lists it). It:

- finds the `nexdesk` executable;
- starts the NexDesk manager on request, as a detached process in its own process group (stopping
  the daemon never closes your remote sessions);
- reports whether NexDesk is installed and whether the manager Aujar started is still running.

It does **not** read NexDesk's profiles, vault or settings, and no password, key or connection
detail ever crosses Aujar's IPC socket.

```bash
aujar nexdesk status     # installed? where? running?
aujar nexdesk open       # open the manager (nothing new if Aujar already opened one)
```

## Why a separate process

- NexDesk's security model depends on its own boundary: the RDP engine runs in `nexdesk-rdp`,
  passwords reach it through `NEXDESK_PASSWORD`, and the vault is its own file. Embedding it in
  the Aujar daemon would weaken that.
- [THREAT_MODEL](THREAT_MODEL.md) rules out in-process native plugins.
- Both projects use GPUI and tokio, but with different pins, patched IronRDP (`vendor/`) and
  feature sets. Aujar already has to build its UI and daemon in separate cargo invocations
  ([ADR 0002](adr/0002-ui-framework-gpui.md)); one more dependency graph would make that worse.

## Where the executable is looked for

1. `$AUJAR_NEXDESK_BIN`, if set. If it is set but is not an executable file, NexDesk is reported
   as not installed (a wrong override is never silently replaced).
2. Next to the running `aujar` binary (how packages install it).
3. `PATH`.

`nexdesk` and `nexdesk-rdp` must be installed **together**: the manager looks for the engine beside
its own executable.

## Source layout: git submodule

NexDesk's source is a git submodule at `integrations/nexdesk`, excluded from Aujar's Cargo workspace
(`exclude` in the root `Cargo.toml`). Aujar's CI never needs it.

```bash
scripts/add-nexdesk-submodule.sh v0.2.0   # once: add and pin a tag or commit
git commit -m "chore: add nexdesk submodule"

git clone --recurse-submodules <aujar-url>   # new clones
git submodule update --init                  # existing clones

scripts/build-nexdesk.sh --install           # build in its own workspace, install beside aujar
```

NexDesk needs its own system libraries (Vulkan, xkbcommon, PipeWire, ...); see its README.
Bump the pinned revision deliberately, in its own commit.

## Policy and capabilities

The module declares `execute_command` and `network`.

| Request | Capability |
|---|---|
| `aujar nexdesk status` | none |
| `aujar nexdesk open` | `execute_command` |

Denying `execute_command` in `/etc/aujar/policy.toml` stops Aujar from opening NexDesk
(see [POLICY](POLICY.md)). It cannot stop a user from starting NexDesk themselves. The module does
not itself open sockets; `network` is declared because the program it starts does.

## Not done yet

- Listing NexDesk's saved connections in the launcher (`rdp <name>`) and connecting to one
  directly. This needs a stable "connect to this profile" command line in NexDesk, and a decision
  about reading its profile format.
- A NexDesk page in the settings window.
- The Aujar daemon runs as a user service: the NexDesk manager inherits its environment, so
  `DISPLAY` / `WAYLAND_DISPLAY` must be available to it (as for any application launched from the
  launcher).
