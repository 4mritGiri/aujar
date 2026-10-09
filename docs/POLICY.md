# Administrator Policy

The daemon reads `/etc/aujar/policy.toml` at startup. A missing file means no restrictions.
See [`config/policy.example.toml`](../config/policy.example.toml).

```toml
denied_capabilities = ["execute_command"]
```

| Capability | Gates |
|---|---|
| `execute_command` | `Execute` (launching applications) |
| `filesystem` | `Rename` (preview and apply) |
| `read_windows` | `Windows` |
| others | reserved for upcoming modules |

Behaviour:

- Denied requests return an error naming the capability; they are never partially executed.
- Unknown capability names or unknown keys abort startup (fail closed) so typos cannot silently
  weaken a policy.
- Changes take effect after a daemon restart.

**Limits (be honest in your threat model):** the policy file is read by a daemon the user runs.
A user who can run their own copy of the binary can bypass it. For real enforcement, deploy the
daemon through a managed, non-user-writable systemd unit/package, or pair this with OS-level
controls (AppArmor/SELinux). Per-client capability grants are planned.

## What `Execute` can and cannot do

- It launches only result ids the launcher itself produced from indexed `.desktop` files
  (`apps:<desktop-id>`). Clients cannot submit a command line.
- The `Exec` line is split into arguments without a shell, and `%u`/`%F`-style field codes are
  dropped, so shell metacharacters are inert.
- `Terminal=true` entries are refused until terminal launching is designed.
- Launched apps run in their own process group, with stdio redirected to `/dev/null`.
- Every launch is logged at info level (id and program).
- Caveat: a `.desktop` file can itself contain a malicious `Exec`. The launcher trusts files in the
  user's XDG application directories, as desktop environments do.
