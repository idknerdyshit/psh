# psh

A Wayland desktop environment component suite, written in Rust. Designed for use with [niri](https://github.com/YaLTeR/niri) and other wlroots-based compositors.

psh provides the essential desktop shell utilities -- bar, notifications, app launcher, clipboard manager, wallpaper, screen lock, idle monitor, and polkit agent -- as independent processes that communicate over a shared IPC socket.

## Components

| Component | Description | Stack |
|-----------|-------------|-------|
| **psh-core** | Shared library: config, IPC, theming, palette export, D-Bus helpers | tokio, zbus, serde |
| **psh-bar** | System bar and IPC hub | GTK4 + layer-shell |
| **psh-notify** | Notification daemon (`org.freedesktop.Notifications`) | GTK4 + layer-shell + zbus |
| **psh-polkit** | Polkit authentication agent | GTK4 + layer-shell + zbus |
| **psh-launch** | Application launcher with fuzzy search | GTK4 + layer-shell + nucleo |
| **psh-clip** | Clipboard history manager | GTK4 + layer-shell + wayland-client |
| **psh-wall** | Wallpaper manager | smithay-client-toolkit |
| **psh-lock** | Screen locker (ext-session-lock-v1) | smithay-client-toolkit + PAM + tiny-skia |
| **psh-idle** | Idle monitor and sleep hook | wayland-protocols + zbus |
| **psh** | CLI control tool | clap + tokio |

## Build dependencies

- Rust (edition 2024)
- GTK4 development libraries (`libgtk-4-dev` / `gtk4`)
- gtk4-layer-shell (`libgtk4-layer-shell-dev` / `gtk4-layer-shell`)
- PAM (`libpam0g-dev` / `pam`)
- D-Bus (`libdbus-1-dev` / `dbus`)
- Wayland (`libwayland-dev`, `wayland-protocols`)

## Building

```sh
cargo build --workspace
```

Or with [just](https://github.com/casey/just):

```sh
just build    # release build
just test     # run tests
just clippy   # lint
just ci       # all checks
```

## Testing

```sh
cargo test --workspace   # 190 unit tests
cargo clippy --workspace # lint check
```

## Configuration

psh uses a single TOML config file at `$XDG_CONFIG_HOME/psh/psh.toml` (usually `~/.config/psh/psh.toml`). The file is entirely optional -- all values have compiled-in defaults. Unknown keys produce a warning at startup.

See [config/psh.toml](config/psh.toml) for an annotated example.

### Configuration reference

```toml
[theme]
name = "default"              # theme name (looks for <name>.css)

[bar]
position = "top"              # top | bottom
# height = 32
# modules_left = ["workspaces", "window_title"]
# modules_center = ["clock"]
# modules_right = ["volume", "network", "battery", "tray"]
# show_all_workspaces = false
# max_title_length = 50
# volume_step = 5
# battery_device = "BAT0"
# claude_session_key = "sk-ant-sid01-..."  # or CLAUDE_SESSION_KEY env var
# claude_display = "percent"               # "percent" | "both"
# claude_poll_interval = 120               # seconds

[notify]
max_visible = 5
default_timeout_ms = 5000
width = 380
gap = 10
icon_size = 48

[polkit]
# No config fields yet

[launch]
# terminal = "foot"           # terminal emulator for Terminal=true apps
# max_results = 20

[wall]
# path = "/home/user/wallpaper.png"   # file, animated GIF/APNG/WebP, or directory (slideshow)
mode = "fill"                 # fill | fit | center | stretch | tile
# interval = 300              # slideshow interval in seconds

# Per-output overrides
# [wall.outputs."DP-1"]
# path = "/path/to/other.png"
# mode = "center"

[lock]
show_clock = true
clock_format = "%H:%M"
date_format = "%A, %B %d"
show_username = true
background_color = "#1e1e2e"
# background_image = "/path/to/image.png"
font_size = 24.0
password_dot_color = "#cdd6f4"
error_color = "#f38ba8"
timeout_secs = 0              # 0 = disabled; blanks screen + clears password after idle
blur_background = false       # apply gaussian blur to background_image

[idle]
idle_timeout_secs = 300       # 0 = disabled
lock_on_sleep = true
lock_command = "psh-lock"

[clip]
max_history = 100
persist = true
image_support = true
max_image_bytes = 10000000
```

Config and theme changes are picked up automatically (hot-reload) by all running components.

## Theming

GTK4 components are styled via CSS. The default theme uses a [Catppuccin Mocha](https://catppuccin.com/) inspired palette. Custom themes can be placed at `$XDG_CONFIG_HOME/psh/themes/<name>.css` and selected via the `[theme]` config section. CSS changes are applied live.

Non-GTK components (psh-wall, psh-lock) read colors directly from the config file.

## psh CLI

The `psh` command sends IPC messages to the running psh-bar hub:

```sh
psh ping        # check if the hub is running
psh lock        # lock the screen
psh launcher    # toggle the app launcher
psh clipboard   # show clipboard history
psh wall set /path/to/image.png             # change wallpaper
psh wall set /path/to/image.png --output DP-1  # change wallpaper on specific output
psh reload      # broadcast config-reload signal
psh theme apply # generate GTK3/GTK4/Qt color overrides from psh palette
```

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

## Installing

With [just](https://github.com/casey/just):

```sh
just install          # installs binaries, systemd units, themes, config
just install-bin      # binaries only
just install-systemd  # systemd units only
just install-themes   # themes only
just install-config   # example config only
just uninstall        # remove everything
```

Packages are available for:
- **Arch Linux (AUR)**: see `packages/aur/PKGBUILD`
- **Gentoo**: see `packages/gentoo/`

## Releasing

Releases use [cargo-release](https://github.com/crate-ci/cargo-release) with per-crate independent versioning. Only bump crates that actually changed.

```sh
# Preview what will happen (always do this first)
cargo release patch -p psh-wall --dry-run

# Release a single crate (bump patch: 0.1.8 → 0.1.9)
cargo release patch -p psh-wall

# Release multiple crates together
cargo release patch -p psh-core -p psh-bar

# Minor or major bumps
cargo release minor -p psh-bar
```

Each crate gets its own git tag (e.g., `psh-wall-v0.1.9`). See `release.toml` for configuration.

## Status

All components are feature-complete.

- **psh-core**: Config (with validation + hot-reload), IPC, theming (with hot-reload), palette export (GTK3/GTK4/Qt), D-Bus, logging (27 tests)
- **psh-bar**: 11 modules (claude, clock, battery, workspaces, window title, volume, network, tray, launcher, clipboard, notifications), IPC hub, configurable layout (50 tests)
- **psh-notify**: Full fd.o Notifications spec, urgency styling, action buttons, replace-id, icons, markup sanitization
- **psh-polkit**: Full polkit agent with security hardening (12 tests)
- **psh-launch**: Fuzzy search, frecency, terminal apps, icon display (17 tests)
- **psh-clip**: Clipboard monitoring, persistence, image support, search/filter (41 tests)
- **psh-wall**: Per-output wallpapers, static/animated/slideshow, 5 display modes, config hot-reload (22 tests)
- **psh-lock**: ext-session-lock-v1, PAM, tiny-skia rendering, inactivity timeout, background blur (21 tests)
- **psh-idle**: Idle timeout + logind sleep detection, config hot-reload
- **psh**: CLI control tool (lock, launcher, clipboard, wallpaper, reload, ping, theme apply)

## License

GPL-3.0-or-later
