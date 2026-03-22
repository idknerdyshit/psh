# psh-lock

Screen locker for the psh desktop environment. Uses ext-session-lock-v1 for secure Wayland screen locking, tiny-skia + ab_glyph for rendering, and PAM for authentication.

## Features

- Wayland-native locking via ext-session-lock-v1 -- atomically prevents input from reaching other surfaces
- PAM authentication on a dedicated thread via `pam-client` conv_mock conversation
- Software-rendered password UI via tiny-skia with ab_glyph text rendering
- Configurable clock, date, username, password dot indicators, and error messages
- Multi-output support with hotplug handling
- Password zeroization in memory on clear, failure, success, and drop
- SIGTERM/SIGINT/SIGHUP ignored while locked -- only authentication or SIGKILL can unlock
- Keyboard handling: Enter (submit), Escape (clear), Backspace (delete char), Ctrl+U (clear all)
- Font loading with system font fallback chain and embedded fallback font

## Security

psh-lock uses the ext-session-lock-v1 Wayland protocol, which provides a compositor-level guarantee that no input can reach underlying surfaces while the lock is active. This is strictly stronger than layer-shell overlays.

PAM authentication runs on a dedicated thread to avoid blocking the Wayland event loop. The password string is zeroized from memory at every opportunity.

Signals (SIGTERM, SIGINT, SIGHUP) are ignored while locked to prevent accidental or malicious termination. Only successful PAM authentication or SIGKILL (which cannot be caught) will end the lock.

## Configuration

```toml
[lock]
show_clock = true           # Show clock on lock screen
clock_format = "%H:%M"      # strftime format for clock
date_format = "%A, %B %d"   # strftime format for date
show_username = true         # Show current username
background_color = "#1e1e2e" # Background color (hex)
# background_image = "/path/to/image.png"  # Optional background image
font_size = 24.0             # Base font size in pixels
password_dot_color = "#cdd6f4"  # Color for password dots
error_color = "#f38ba8"      # Color for error messages
timeout_secs = 0             # Inactivity timeout: blanks screen + clears password (0 = disabled)
blur_background = false      # Apply gaussian blur to background_image
```

## Running

```sh
cargo run -p psh-lock
```

Requires a running Wayland session with ext-session-lock-v1 support (e.g., niri, sway).

Typically invoked by `psh-idle` (for idle/sleep locking) or a keybind in your compositor config.

## Tests

```sh
cargo test -p psh-lock  # 16 unit tests
```
