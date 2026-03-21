# psh-core

Shared library for the psh desktop environment suite. Every other psh component depends on this crate.

## Modules

- **config** -- TOML config parsing with per-component structs and compiled-in defaults
- **ipc** -- Unix socket protocol (length-prefixed JSON) with `send`/`recv`/`connect`/`bind`
- **theme** -- GTK4 CSS theme loading (behind the `gtk` feature flag)
- **dbus** -- zbus connection helpers for session and system bus
- **error** -- `PshError` enum with `thiserror` derives and a `Result<T>` alias
- **logging** -- tracing-subscriber init with `PSH_LOG` env filter

## Config sections

The top-level `PshConfig` struct includes sections for each component:

| Section | Struct | Description |
|---------|--------|-------------|
| `[theme]` | `ThemeConfig` | Theme name |
| `[bar]` | `BarConfig` | Bar position, height, module layout |
| `[notify]` | `NotifyConfig` | Max visible, timeout, width, icon size |
| `[polkit]` | `PolkitConfig` | (empty, reserved) |
| `[launch]` | `LaunchConfig` | Terminal app, max results |
| `[wall]` | `WallConfig` | Wallpaper path and mode |
| `[lock]` | `LockConfig` | Clock, colors, font size, background |
| `[idle]` | `IdleConfig` | Idle timeout, sleep hook, lock command |
| `[clip]` | `ClipConfig` | History size, persistence, image support |

## Feature flags

- `default` -- no GTK dependency
- `gtk` -- enables the `theme` module, pulls in `gtk4`

psh-wall, psh-lock, and psh-idle use the default features. All GTK components use `features = ["gtk"]`.

## Usage

```rust
// Initialize logging
psh_core::logging::init("my_component");

// Load config (returns defaults if no file exists)
let cfg = psh_core::config::load()?;

// Connect to the IPC hub
let mut stream = psh_core::ipc::connect().await?;
psh_core::ipc::send(&mut stream, &psh_core::ipc::Message::Ping).await?;
```

## Tests

```sh
cargo test -p psh-core  # 13 unit tests
```
