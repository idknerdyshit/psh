# psh-idle

Idle monitor and sleep hook daemon for the psh desktop environment. Watches for user inactivity and system sleep events, spawning `psh-lock` when triggered.

## Features

- Idle detection via ext-idle-notify-v1 Wayland protocol
- System sleep/suspend detection via logind PrepareForSleep D-Bus signal
- Configurable idle timeout and lock command
- Process tracking -- won't spawn a second lock instance
- Clean shutdown on SIGTERM/SIGINT

## How it works

psh-idle runs as a long-lived daemon. It monitors two sources:

1. **Idle timeout**: Uses the ext-idle-notify-v1 Wayland protocol to detect when the user has been inactive for a configurable duration. When the `idled` event fires, psh-idle spawns the lock command.

2. **Sleep hook**: Connects to the system D-Bus and listens for logind's `PrepareForSleep(true)` signal. When the system is about to sleep/suspend, psh-idle spawns the lock command before sleep occurs.

## Configuration

```toml
[idle]
idle_timeout_secs = 300   # 5 minutes (0 = disabled)
lock_on_sleep = true      # Lock on system sleep/suspend
lock_command = "psh-lock"  # Command to run for locking
```

## Running

```sh
cargo run -p psh-idle
```

Requires a running Wayland session with ext-idle-notify-v1 support and (optionally) a running logind/elogind for sleep detection.

## With systemd

```sh
systemctl --user enable --now psh-idle.service
```
