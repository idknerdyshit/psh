# psh-lock

Screen locker for the psh desktop environment. Uses smithay-client-toolkit and ext-session-lock-v1 for secure screen locking, with PAM for authentication.

## Features

- Wayland-native locking via ext-session-lock-v1 (planned)
- PAM authentication on a dedicated thread
- Software-rendered password UI via tiny-skia (planned)

## Security

psh-lock uses the ext-session-lock-v1 Wayland protocol, which atomically prevents input from reaching other surfaces. This provides a stronger security guarantee than layer-shell overlays.

PAM authentication runs on a dedicated thread to avoid blocking the Wayland event loop.

## Configuration

```toml
[lock]
show_clock = true
```

## Running

```sh
cargo run -p psh-lock
```

Requires a running Wayland session with ext-session-lock-v1 support.
