# psh-polkit

Polkit authentication agent. Shows a GTK4 password dialog when a privileged action is requested.

## Stack

- GTK4 + gtk4-layer-shell (auth dialog)
- zbus 5 (polkit agent D-Bus interface + authority proxy)
- async-channel (D-Bus thread -> GTK thread, GTK thread -> D-Bus thread for response)
- tokio (background thread runtime, per-session mpsc channels, broadcast for cancellation)
- secrecy + zeroize (password handling)
- libc (NSS user lookup via `getpwuid_r`, caller uid via `getuid`)
- psh-core with `gtk` feature

## How it works

1. GTK4 app starts, applies theme
2. Spawns a background thread with tokio runtime running the polkit agent
3. Agent connects to D-Bus system bus, serves the `AuthenticationAgent` interface, and registers with the polkit authority for the current session
4. When polkitd calls `BeginAuthentication`, the agent sends an `AuthRequest` to the GTK thread via `async-channel`
5. GTK thread shows a layer-shell overlay dialog with exclusive keyboard grab
6. User enters password (or cancels via button/Escape/120s timeout)
7. Response routed back through a per-session `tokio::sync::mpsc` channel (keyed by cookie) to avoid cross-session confusion
8. Agent authenticates via `polkit-agent-helper-1`, retries up to 3 times on failure
9. On success, calls `AuthenticationAgentResponse2` on the polkit authority with the caller's real uid
10. On shutdown, unregisters from the polkit authority

## Current state — functional

**Working:**
- D-Bus interface at `/org/freedesktop/PolicyKit1/AuthenticationAgent` with `BeginAuthentication` and `CancelAuthentication`
- Registration with polkit authority via `RegisterAuthenticationAgent` at startup
- Session subject detection (`$XDG_SESSION_ID` with `/proc/self/sessionid` fallback, rejects "unset")
- Password authentication via `polkit-agent-helper-1` with up to 3 retries
- `AuthenticationAgentResponse2` call on success (uses caller's real uid)
- Per-session response channels with `SessionGuard` RAII cleanup — safe for concurrent auth requests
- Broadcast-based cancellation — handles concurrent `CancelAuthentication` calls correctly
- NSS-aware username resolution via `libc::getpwuid_r` (supports LDAP/SSSD/NIS)
- Password zeroization: `SecretString` + `zeroize` on intermediate `String` + GTK buffer clear
- GTK4 dialog with exclusive keyboard grab, password entry, cancel + authenticate buttons
- Escape key dismisses dialog
- 120-second auto-cancel timeout to prevent indefinite keyboard grab
- Error feedback on wrong password (show error label, allow retry)
- Graceful shutdown with polkit authority unregistration

**Not yet implemented:**
- Config fields (no `[polkit]` section options yet)
- IPC integration with psh-bar

## Key files

- `main.rs` — GTK app, `show_auth_dialog()` builds the overlay, dispatches `AgentToGtk` messages
- `agent.rs` — `PolkitAgent` D-Bus interface, `SessionGuard`, per-session channel dispatcher, `AuthRequest`/`AuthResponse` types
- `authority.rs` — polkit authority D-Bus proxy, session detection, registration, identity helpers, password verification

## Testing

```sh
cargo test -p psh-polkit -- --test-threads=1  # unit tests (env var tests need single-thread)
pkexec ls                                      # manual: triggers a polkit auth prompt
```

Unit tests cover:
- `extract_uid` — unix-user extraction, skips non-unix-user, empty list
- `uid_to_username` — resolves root (uid 0), returns None for nonexistent uid
- `detect_session_subject` — env var usage, rejects "unset", rejects empty
- `SessionGuard` — removes entry on drop, only removes own cookie
- Dispatcher routing — routes response to correct per-session channel

## Config

```toml
[polkit]
# No config fields yet
```
