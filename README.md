# psh

A Wayland desktop environment component suite, written in Rust. Designed for use with [niri](https://github.com/YaLTeR/niri) and other wlroots-based compositors.

psh provides the essential desktop shell utilities -- bar, notifications, app launcher, clipboard manager, wallpaper, screen lock, and polkit agent -- as independent processes that communicate over a shared IPC socket.

## Components

| Component | Description | Stack |
|-----------|-------------|-------|
| **psh-core** | Shared library: config, IPC, theming, D-Bus helpers | tokio, zbus, serde |
| **psh-bar** | System bar and IPC hub | GTK4 + layer-shell |
| **psh-notify** | Notification daemon (`org.freedesktop.Notifications`) | GTK4 + layer-shell + zbus |
| **psh-polkit** | Polkit authentication agent | GTK4 + layer-shell + zbus |
| **psh-launch** | Application launcher with fuzzy search | GTK4 + layer-shell + nucleo |
| **psh-clip** | Clipboard history manager | GTK4 + layer-shell + wayland-client |
| **psh-wall** | Wallpaper manager | smithay-client-toolkit |
| **psh-lock** | Screen locker | smithay-client-toolkit + PAM |

## Building

Requires Rust (edition 2024), GTK4 development libraries, and Wayland development headers.

```sh
cargo build --workspace
```

## Configuration

psh uses a single TOML config file at `$XDG_CONFIG_HOME/psh/psh.toml` (usually `~/.config/psh/psh.toml`). The file is entirely optional -- all values have compiled-in defaults.

See [config/psh.toml](config/psh.toml) for an annotated example.

## Theming

GTK4 components are styled via CSS. The default theme uses a [Catppuccin Mocha](https://catppuccin.com/) inspired palette. Custom themes can be placed in `assets/themes/` and selected via the `[theme]` config section.

Non-GTK components (psh-wall, psh-lock) read colors directly from the config file.

## IPC

All components communicate over a Unix socket at `$XDG_RUNTIME_DIR/psh.sock`. psh-bar acts as the central hub -- it binds the socket, and all other components connect as clients. Messages are length-prefixed (4-byte big-endian) JSON.

## Running with systemd

Systemd user services are provided in `systemd/`. To install:

```sh
# Copy service files
cp systemd/*.service systemd/*.target ~/.config/systemd/user/

# Enable and start
systemctl --user enable --now psh.target
```

`psh.target` pulls in all components. Individual services can be started or stopped independently.

## Status

This project is under active development. psh-core, psh-wall, psh-notify, psh-polkit, psh-launch, and psh-clip are feature-complete. psh-bar and psh-lock are functional scaffolds. See [PLAN.md](PLAN.md) for the detailed roadmap.

## License

GPL-3.0-or-later
