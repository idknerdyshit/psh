# psh — Wayland DE component suite

## Project overview

A monorepo Rust workspace producing 7 binaries + 1 shared library. All components target Wayland compositors (primarily niri) using layer-shell for UI placement.

## Build

```sh
cargo build --workspace
cargo test --workspace
cargo clippy --workspace
```

## Architecture

- **psh-core** — shared library: config, IPC, theming, D-Bus helpers, errors, logging
- **psh-bar** — system bar (GTK4 + layer-shell), acts as the IPC hub
- **psh-notify** — notification daemon (GTK4 + layer-shell + D-Bus)
- **psh-polkit** — polkit auth agent (GTK4 + layer-shell + D-Bus)
- **psh-launch** — app launcher (GTK4 + layer-shell + nucleo fuzzy search)
- **psh-clip** — clipboard manager (GTK4 + layer-shell)
- **psh-wall** — wallpaper manager (smithay-client-toolkit, no GTK)
- **psh-lock** — screen locker (smithay-client-toolkit, no GTK)

## Key patterns

### GTK4 + async integration
GTK4 runs the GLib main loop on the main thread. Tokio runs on a background `std::thread`. Communication is via `async-channel` with `glib::spawn_future_local` on the GTK side. Every GTK component follows this pattern — do not change it.

### IPC
Length-prefixed (4-byte big-endian) JSON over a Unix socket at `$XDG_RUNTIME_DIR/psh.sock`. psh-bar is the hub (`ipc::bind()`), all others are clients (`ipc::connect()`). Message types are in `psh_core::ipc::Message`. When adding new message types, add the variant there and update serialization tests.

### Config
Single TOML file at `$XDG_CONFIG_HOME/psh/psh.toml`. Each component has a section (`[bar]`, `[notify]`, etc.). All fields have compiled-in defaults — the config file is optional. Config structs live in `psh_core::config`. Use `#[serde(default)]` on every config struct.

### Theming
GTK4 CSS loaded via `psh_core::theme::apply_theme()`. CSS classes follow the pattern `.psh-{component}` and `.psh-{component}-{element}`. Non-GTK components (wall, lock) read colors from config TOML.

### D-Bus
Use `zbus` v5. Connection builders are at `zbus::connection::Builder` (not `zbus::ConnectionBuilder` — that was v4). D-Bus interfaces use the `#[zbus::interface]` attribute macro.

### Error handling
All fallible functions return `psh_core::Result<T>`. The `PshError` enum has `#[from]` conversions for common error types. Add new variants to `psh_core::error::PshError` when introducing new error sources.

### Logging
Every binary starts with `psh_core::logging::init("crate_name")`. Uses `tracing` macros throughout. Control level with `PSH_LOG=debug`.

## Conventions

- Edition 2024, resolver v2
- Workspace dependencies — all versions pinned in root `Cargo.toml`, crates use `{ workspace = true }`
- Binary crates have `#![allow(dead_code, unused_imports)]` at the top during scaffold phase — remove these as components become feature-complete
- CSS class names: `.psh-bar`, `.psh-notify-popup`, `.psh-launch-search`, etc.
- Systemd service files in `systemd/`, one per component + `psh.target`
- Follow XDG Base Directory Specification for all file paths:
  - Config: `$XDG_CONFIG_HOME/psh/` (via `directories::BaseDirs::config_dir()`)
  - Runtime (sockets, locks): `$XDG_RUNTIME_DIR/`
  - If adding persistent data or cache files, use `$XDG_DATA_HOME/psh/` and `$XDG_CACHE_HOME/psh/` respectively
  - Never hard-code `~/.config`, `~/.local/share`, etc. — always use `directories` crate or `$XDG_*` env vars

## Testing

- Unit tests in psh-core for config parsing and IPC serialization
- Unit tests in psh-clip for clipboard history data structure
- Unit tests in psh-polkit for identity extraction, username resolution, session detection, session guard cleanup, and dispatcher routing (`cargo test -p psh-polkit -- --test-threads=1`)
- Integration testing is manual: run component on a Wayland session and exercise it
- Test notifications with: `notify-send "title" "body"`
- Test polkit with: `pkexec ls`

## Implementation status

See `PLAN.md` for per-phase breakdown.
- **Complete:** psh-core, psh-wall, psh-notify, psh-polkit, psh-launch
- **psh-notify** — full fd.o Notifications D-Bus spec: single-window stacking, urgency styling, action buttons, signals, replace-id, icons, markup sanitization, IPC count broadcast.
- **psh-polkit** — full polkit auth agent: authority registration, session detection, per-session concurrent auth, password verification via polkit-agent-helper-1, NSS username resolution, password zeroization, Escape key + 120s timeout, 12 unit tests.
- **psh-launch** — long-lived daemon with IPC toggle, .desktop parsing, nucleo fuzzy search, GTK4 icon display, terminal app support, frecency sorting (persistent JSON), Enter/Escape keyboard nav, single-instance, desktop entry refresh on show, 4 unit tests.
- **Partial scaffolds:** psh-bar, psh-clip, psh-lock — compile and have basic structure but need significant feature work.
