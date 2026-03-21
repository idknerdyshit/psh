# psh-core

Shared library crate — every other psh component depends on this.

## Modules

| Module | Status | Purpose |
|---|---|---|
| `config.rs` | Complete | TOML config parsing, hot-reload via inotify, per-component config structs |
| `ipc.rs` | Complete | Unix socket protocol — 4-byte length prefix + JSON body, send/recv/connect/bind |
| `theme.rs` | Complete | GTK4 CSS theme loading, `apply_theme()` / `apply_css()` helpers. Behind `gtk` feature flag. |
| `dbus.rs` | Complete | zbus connection helpers: `session_bus()`, `session_bus_with_name()`, `system_bus()` |
| `error.rs` | Complete | `PshError` enum with thiserror, `Result<T>` type alias |
| `logging.rs` | Complete | tracing-subscriber init with `PSH_LOG` env filter |

## Feature flags

- `default` — no GTK dependency
- `gtk` — enables `theme` module, pulls in `gtk4` crate. Used by all GTK components.

psh-wall and psh-lock depend on psh-core *without* the `gtk` feature. All other components use `features = ["gtk"]`.

## Adding new config fields

1. Add the field to the appropriate config struct in `config.rs` with a serde default
2. If adding a new component section, add a new struct and a field on `PshConfig`
3. Add the field to `config/psh.toml` (example config) with a comment showing the default
4. Add a test case in the `tests` module

## Adding new IPC message types

1. Add the variant to `ipc::Message` enum
2. Add the variant to the `all_variants_serialize` test
3. Handle the new message in psh-bar's IPC hub (`run_ipc_hub`)
4. Handle it in whichever client component sends/receives it

## Tests

8 unit tests: 3 in config (default parse, partial parse, missing file), 2 in ipc (roundtrip, all variants), and 3 in clip history (indirectly, via psh-clip).

Run: `cargo test -p psh-core`
