# psh-idle

Idle monitor and sleep hook daemon. Spawns `psh-lock` when the user is idle or the system is going to sleep.

## Stack

- smithay-client-toolkit 0.19 (seat binding, calloop integration)
- wayland-client 0.31 + wayland-protocols 0.32 (ext-idle-notify-v1)
- zbus 5 (logind PrepareForSleep D-Bus signal)
- tokio (async runtime for D-Bus thread)
- signal-hook 0.3 (SIGTERM/SIGINT handling)
- psh-core (config, logging)

## How it works

1. Connects to Wayland, binds seat
2. Binds `ext_idle_notifier_v1` global, creates idle notification with configured timeout
3. On `idled` event → spawns `psh-lock` (or configured lock command)
4. On `resumed` event → no action (psh-lock handles its own unlock)
5. Optionally monitors logind `PrepareForSleep(true)` signal → spawns `psh-lock` before sleep
6. Tracks spawned psh-lock PID, won't spawn another if one is already running
7. Clean shutdown on SIGTERM/SIGINT

## Key types

- `IdleState` — main state: seat, idle notification, lock child process, config
- `IdleCommand` — internal enum for calloop channel (Lock)
- `LogindManagerProxy` — zbus-generated proxy for logind PrepareForSleep signal

## Config

```toml
[idle]
idle_timeout_secs = 300   # 5 minutes, 0 = disabled
lock_on_sleep = true
lock_command = "psh-lock"
```

## Testing

```sh
cargo build -p psh-idle
cargo clippy -p psh-idle
```

Manual testing: run `psh-idle`, wait for idle timeout, verify psh-lock spawns. Test sleep with `systemctl suspend`.
