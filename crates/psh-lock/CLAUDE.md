# psh-lock

Screen locker. Security-critical — must correctly prevent input from reaching underlying surfaces.

## Stack

- smithay-client-toolkit 0.19 (Wayland client, will use ext-session-lock-v1)
- wayland-client 0.31
- pam-client 0.5 (PAM authentication)
- tiny-skia 0.11 (software rendering for password UI)
- psh-core (config, logging — no `gtk` feature)

## How it works (target design)

1. Connect to Wayland, bind ext-session-lock-v1
2. Acquire the session lock (compositor blocks all other input)
3. Create a lock surface on every output
4. Render password entry UI using tiny-skia
5. Handle keyboard input, accumulate password
6. On Enter, authenticate via PAM on a dedicated thread
7. On success, destroy lock and exit
8. On failure, show error, clear password, allow retry

## Current state — stub

**Working:**
- Wayland connection + SCTK boilerplate (compositor, shm, output, registry handlers)
- All delegate macros wired up
- PAM function signature (`authenticate_pam`)
- LockState struct with password field

**Missing (see PLAN.md Phase 7):**
- ext-session-lock-v1 protocol binding (currently just creates a bare wl_surface)
- Keyboard input handling via wl_keyboard
- tiny-skia rendering of password dots, clock, user info
- PAM conversation function (currently uses `conv_null` which can't actually supply a password)
- Multi-output lock surfaces
- Auth failure handling (error display, retry)
- Graceful unlock on success
- Idle/DPMS integration

## Security considerations

- **PAM must run on a dedicated thread**, never on the Wayland event loop. The current `try_authenticate()` correctly spawns a thread and uses `mpsc` to get the result.
- The lock must cover ALL outputs. Missing an output means the user can see/interact with content behind the lock.
- ext-session-lock-v1 is the correct protocol — it atomically prevents input from reaching other surfaces. Do NOT use layer-shell for locking (it doesn't provide the security guarantee).
- Password must be zeroed from memory after use (consider `zeroize` crate).

## Key types

- `LockState` — main state struct, holds SCTK state + password + locked flag
- `LockState::draw()` — will render the password UI (currently empty)
- `LockState::try_authenticate()` — spawns PAM thread, returns bool
- `authenticate_pam()` — PAM context creation + authenticate call

## Config

```toml
[lock]
show_clock = true
```
