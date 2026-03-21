# psh-polkit

Polkit authentication agent for the psh desktop environment. Shows a GTK4 layer-shell overlay dialog when a privileged action is requested.

## Features

- Registers with the polkit authority on startup, unregisters on shutdown
- Session subject detection via `$XDG_SESSION_ID` (falls back to `/proc/self/sessionid`)
- Password authentication via `polkit-agent-helper-1` with up to 3 retries
- Per-session channel routing — handles concurrent authentication requests safely
- Broadcast-based cancellation — concurrent `CancelAuthentication` calls don't interfere
- NSS-aware username resolution (`getpwuid_r`) — works with LDAP, SSSD, NIS
- Password zeroization: `SecretString`, `zeroize` on intermediate strings, GTK buffer clear
- Layer-shell overlay dialog with exclusive keyboard grab
- Password entry with peek toggle, cancel and authenticate buttons
- Escape key dismisses dialog
- 120-second auto-cancel timeout to prevent indefinite keyboard grab
- Error feedback on wrong password with retry

## Configuration

```toml
[polkit]
# No config fields yet
```

## Testing

```sh
cargo test -p psh-polkit -- --test-threads=1  # unit tests
pkexec ls                                      # manual: triggers a polkit auth prompt
```

## Running

```sh
cargo run -p psh-polkit
```

Requires a running Wayland session with layer-shell support (e.g., niri, sway).
