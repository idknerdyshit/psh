# psh-lock

Screen locker. Security-critical — must correctly prevent input from reaching underlying surfaces.

## Stack

- smithay-client-toolkit 0.19 (ext-session-lock-v1, seat/keyboard, calloop integration)
- wayland-client 0.31
- pam-client 0.5 (PAM authentication via conv_mock)
- tiny-skia 0.11 (software rendering for password UI)
- ab_glyph 0.2 (font rasterization for text rendering)
- zeroize 1 (password memory zeroing)
- libc 0.2 (getpwuid_r for username, localtime_r for clock, signal handling)
- psh-core (config, logging — no `gtk` feature)

## How it works

1. Ignores SIGTERM/SIGINT/SIGHUP (lock should only unlock via auth)
2. Connects to Wayland, binds ext-session-lock-v1 via SCTK
3. Acquires session lock (compositor blocks all other input)
4. Creates a lock surface on every output (handles hotplug)
5. Renders password entry UI using tiny-skia + ab_glyph
6. Handles keyboard input via SCTK `KeyboardHandler` — accumulates password
7. On Enter, authenticates via PAM on a dedicated thread (calloop channel for result)
8. On success, calls `SessionLock::unlock()` and exits
9. On failure, shows error, clears password, resets to idle after 2s

## Invocation

On-demand only — launched directly by keybind, `psh-idle`, or any external trigger. Locks immediately on start, exits on successful unlock. Not a daemon.

## Key files

- `main.rs` — entry point, calloop event loop setup, signal ignoring
- `state.rs` — `LockState` struct, all SCTK handler impls (SessionLock, Seat, Keyboard, Compositor, Output, Shm, Registry), username resolution, time formatting
- `render.rs` — tiny-skia + ab_glyph rendering: background, clock, date, username, password dots, error messages, font loading with fallback chain
- `pam.rs` — PAM authentication on dedicated thread via `conv_mock::Conversation`

## Key types

- `LockState` — main state: SCTK globals, session lock, keyboard, password, auth state, render state
- `LockSurface` — per-output lock surface with dimensions, scale factor, and dedicated shm pool
- `AuthState` — Idle | Authenticating | Failed(String) | Unlocked
- `RenderState` — loaded font for text rendering
- `RenderParams` — snapshot of state needed for rendering (avoids borrow issues)
- `PamResult` — Success | Failed(String)

## Security considerations

- **PAM runs on a dedicated thread** — never on the Wayland event loop
- **ext-session-lock-v1** is used (not layer-shell) — atomically prevents input leaks
- **Password zeroized** on clear, auth failure, auth success, and `Drop`
- **SIGTERM/SIGINT/SIGHUP ignored** while locked — only SIGKILL can kill it
- **All outputs covered** — lock surfaces created for every output, including hotplug

## Config

```toml
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
timeout_secs = 0       # 0 = disabled
blur_background = false # placeholder for future
```

## Testing

```sh
cargo test -p psh-lock   # 16 unit tests
cargo clippy -p psh-lock # no warnings
```

Unit tests cover: hex color parsing, circle path generation, RGBA/BGRA conversion, pixmap rendering (idle, failed, authenticating, no clock, zero size), password dot layout, time formatting, username resolution, PAM result types.

Manual testing: run `psh-lock` on a Wayland session with a compositor that supports ext-session-lock-v1 (e.g., niri, sway).
