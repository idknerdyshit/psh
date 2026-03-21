# psh-wall

A lightweight Wayland wallpaper manager, part of the [psh](../../) desktop environment suite.

## Features

- **Multi-monitor support** — automatically creates a wallpaper surface for each connected output
- **Output hotplug** — handles monitors being added or removed at runtime
- **HiDPI support** — renders at native resolution using the output's scale factor
- **5 wallpaper modes:**
  - `fill` — scale to cover the entire screen, cropping overflow (default)
  - `fit` — scale to fit within the screen, with letterboxing
  - `center` — display at original size, centered
  - `stretch` — stretch to fill, ignoring aspect ratio
  - `tile` — repeat the image across the screen
- **Runtime wallpaper changes** via IPC (`SetWallpaper` message)
- **Graceful shutdown** on SIGTERM/SIGINT

## Dependencies

### Build

- Rust (edition 2024)
- wayland-client development libraries

### Runtime

- A Wayland compositor with `wlr-layer-shell` support (e.g., niri, sway, Hyprland)

## Installation

psh-wall is built as part of the psh workspace:

```sh
cargo build --release -p psh-wall
```

The binary will be at `target/release/psh-wall`.

To install system-wide:

```sh
install -Dm755 target/release/psh-wall /usr/local/bin/psh-wall
install -Dm644 systemd/psh-wall.service ~/.config/systemd/user/psh-wall.service
```

## Configuration

Configuration lives in `~/.config/psh/psh.toml` under the `[wall]` section:

```toml
[wall]
path = "/path/to/wallpaper.png"
mode = "fill"  # fill | fit | center | stretch | tile
```

All fields are optional — without a `path`, psh-wall displays a solid dark background (#1e1e2e).

## Usage

### Standalone

```sh
psh-wall
```

### With systemd

```sh
systemctl --user enable --now psh-wall
```

### Changing wallpaper at runtime

When psh-bar (the IPC hub) is running, send a `SetWallpaper` message to change the wallpaper without restarting:

```sh
# Via psh-ctl (planned)
psh-ctl wall set /path/to/new-wallpaper.jpg
```

### Logging

Control log verbosity with the `PSH_LOG` environment variable:

```sh
PSH_LOG=debug psh-wall
```

## Architecture

psh-wall uses [smithay-client-toolkit](https://github.com/Smithay/client-toolkit) directly (no GTK) to create layer-shell background surfaces. It runs a [calloop](https://github.com/Smithay/calloop) event loop that multiplexes:

- Wayland protocol events (surface configure, output add/remove, scale changes)
- IPC commands from the psh hub (wallpaper changes)
- Signal handling (SIGTERM/SIGINT for clean shutdown)

Image loading and resizing is handled by the [image](https://github.com/image-rs/image) crate with Lanczos3 filtering.
