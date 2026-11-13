# Awake

Keep the computer from sleeping: until you turn it off, or for a set time. The PowerToys tool
of the same name, for Linux.

## Using it

```
aujar awake on          # keep awake until you turn it off
aujar awake for 2h      # keep awake for two hours (30m, 1h30m, 1d; a bare number is minutes)
aujar awake status      # is it on, and for how long
aujar awake off         # let the computer sleep again
```

Or open Settings, Awake. A timed mode is 1 minute to 7 days; for longer, use `on`.

The setting **Keep the screen on too** (`[awake] keep_screen_on = true` in `settings.toml`) also
stops the screen from turning off and the session from locking. Without it only sleep is
prevented. It takes effect at once, even while Awake is running.

## How it works

The Aujar daemon asks the desktop not to sleep through the standard inhibit portal
(`org.freedesktop.portal.Inhibit`). The lock lives as long as the daemon's connection to the
desktop, so if the daemon stops or crashes the computer may sleep again: a crash can never leave
it unable to sleep.

- The mode is **remembered across restarts** in `state.toml` in the state directory
  (`~/.local/state/aujar/`, readable only by you). After a restart (or a reboot, once the daemon
  starts) Awake carries on; a timed mode that ran out meanwhile is simply off. A damaged
  `state.toml` is moved aside as `state.toml.bad` and Awake starts off.
- A timed mode **ends by itself**; nothing needs to stay running for that.
- If the desktop refuses (no portal), `aujar awake status` and the Settings page say so, and
  Aujar tries again every 30 seconds.
- An administrator policy can deny it: see the `inhibit_sleep` capability in
  [POLICY](POLICY.md). Looking at the status is always allowed.

Only Linux is supported for now; other platforms report that clearly.
