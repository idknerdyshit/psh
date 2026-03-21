# psh-polkit

Polkit authentication agent. Shows a GTK4 password dialog when a privileged action is requested.

## Stack

- GTK4 + gtk4-layer-shell (auth dialog)
- zbus 5 (polkit agent D-Bus interface)
- async-channel (D-Bus thread -> GTK thread, GTK thread -> D-Bus thread for response)
- psh-core with `gtk` feature

## How it works

1. GTK4 app starts, applies theme
2. Spawns a background thread with the polkit agent D-Bus server on the system bus
3. When `BeginAuthentication` is called, sends an `AuthRequest` to the GTK thread
4. GTK thread shows a layer-shell overlay dialog with password entry + cancel/authenticate buttons
5. User response is sent back to the D-Bus thread via a second channel

## Current state — partial

**Working:**
- D-Bus interface at `/org/freedesktop/PolicyKit1/AuthenticationAgent` with `BeginAuthentication` and `CancelAuthentication` methods
- GTK4 dialog with exclusive keyboard grab, password entry, cancel + authenticate buttons
- Bidirectional channel communication between D-Bus and GTK threads

**Missing (see PLAN.md Phase 3):**
- `RegisterAuthenticationAgent` call to the polkit authority at startup (currently the agent is served but never registered, so polkit doesn't know about it)
- Passing the password back through polkit's `AuthenticationAgentResponse2` D-Bus call
- Proper identity parsing (extract unix-user uid from the identity tuples)
- Error feedback on wrong password (show error label, allow retry)
- Session/subject detection for registration

## Key files

- `main.rs` — GTK app, `show_auth_dialog()` builds the overlay
- `agent.rs` — `PolkitAgent` struct with D-Bus interface, `AuthRequest`/`AuthResponse` types

## Testing

```sh
pkexec ls  # triggers a polkit auth prompt
```

## Config

```toml
[polkit]
# No config fields yet
```
