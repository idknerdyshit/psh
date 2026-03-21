# psh implementation plan

Status legend: **done** = working, **partial** = compiles but incomplete, **stub** = scaffold only

## Current state

| Crate | Status | What works | What's missing |
|---|---|---|---|
| psh-core | **done** | Config parsing + hot-reload, IPC protocol, theme loading, D-Bus helpers, error types, logging. 13 tests. | — |
| psh-wall | **done** | Multi-output layer surfaces, all wallpaper modes (fill/fit/center/stretch/tile), HiDPI scale factor, output hotplug, IPC `SetWallpaper` listener, calloop event loop, SIGTERM shutdown | — |
| psh-notify | **done** | Full fd.o Notifications D-Bus spec: single-window stacking, urgency styling, action buttons with `ActionInvoked` signal, `NotificationClosed` signal, replace-id, app icons (name + image-data hint), markup sanitization, `NotificationCount` IPC broadcast, `max_visible` enforcement, configurable timeouts | — |
| psh-polkit | **done** | Full polkit auth agent: authority registration, session detection, per-session concurrent auth with `SessionGuard`, password verification via `polkit-agent-helper-1`, `AuthenticationAgentResponse2`, NSS username resolution (`getpwuid_r`), password zeroization, Escape key + 120s timeout, error feedback on wrong password, 12 unit tests | Config fields, IPC integration with psh-bar |
| psh-launch | **done** | Long-lived daemon with IPC toggle, .desktop file parsing, fuzzy search with nucleo, GTK4 overlay, icon display (GTK4 icon theme), terminal app support, frecency sorting (persistent JSON), Enter/Escape/arrow key navigation, single-instance via GTK Application, desktop entry refresh on show | — |
| psh-clip | **done** | Clipboard monitoring (`zwlr-data-control-v1`), `ClipEntry` (text + image), persistent history (`$XDG_DATA_HOME/psh/clip_history.json`), image caching (`$XDG_CACHE_HOME/psh/clips/`), GTK4 picker with search/filter, paste-on-select via data-control source, image thumbnails, self-copy detection, orphan image pruning, 39 tests | — |
| psh-bar | **done** | BarModule trait + dynamic loading, bidirectional IPC hub, 10 modules (clock, battery, workspaces/niri, window_title/niri, volume/wpctl, network/NM D-Bus, tray/SNI, launcher btn, clipboard btn, notification count), configurable module layout, 35 tests | ext-workspace-v1 fallback (stub), ext-foreign-toplevel fallback (stub), wifi signal strength, tray DBusMenu right-click menus |
| psh-lock | **done** | ext-session-lock-v1, calloop event loop, SCTK keyboard input, tiny-skia + ab_glyph rendering (clock, date, username, password dots, error messages), PAM auth on dedicated thread, multi-output lock surfaces with hotplug, password zeroization, signal ignoring, 16 tests | — |
| psh-idle | **done** | ext-idle-notify-v1 idle detection, logind PrepareForSleep sleep hook, spawns psh-lock, process management, calloop event loop, SIGTERM shutdown | — |

## Phases

### Phase 1 — Make psh-wall fully functional
The simplest component. Get it production-ready first.

- [x] Handle multiple outputs (one layer surface per wl_output)
- [x] Implement wallpaper modes: fill (done), fit, center, stretch, tile
- [x] Handle output hotplug (new/removed monitors)
- [x] Handle scale factor (HiDPI)
- [x] Listen on IPC for `SetWallpaper` messages to change wallpaper at runtime
- [x] Graceful shutdown on SIGTERM

### Phase 2 — Make psh-notify fully functional
First real GTK4 component, testable with `notify-send`.

- [x] Notification stacking — single-window architecture with vertical Box layout
- [x] Track active notifications, respect `max_visible` config
- [x] Urgency levels (`low`/`normal`/`critical`) with distinct styling
- [x] Action buttons — render and emit `ActionInvoked` D-Bus signal on click
- [x] `NotificationClosed` D-Bus signal with proper reason codes
- [x] Replace-id support — update existing popup instead of creating new one
- [x] App icon display (from icon name or image data hint)
- [x] Broadcast `NotificationCount` over IPC to psh-bar

### Phase 3 — Make psh-polkit functional
Small scope, high value — needed for any privileged action.

- [x] Call `RegisterAuthenticationAgent` on the polkit authority at startup
- [x] Parse identity list properly (extract unix-user uid)
- [x] Send password back via `AuthenticationAgentResponse2` D-Bus call
- [x] Handle auth failure — show error label, allow retry
- [x] Handle `cancel_authentication` — close dialog
- [x] Proper session/subject detection for registration
- [x] NSS-aware username resolution (`getpwuid_r`)
- [x] Per-session channel routing for concurrent auth requests
- [x] Password zeroization (`SecretString` + `zeroize`)
- [x] Escape key and 120-second auto-cancel timeout
- [x] Unit tests (12 tests: identity extraction, username resolution, session detection, session guard, dispatcher routing)

### Phase 4 — Make psh-launch functional
Keyboard-driven launcher overlay.

- [x] IPC client — listen for `ToggleLauncher`, toggle window visibility
- [x] Single-instance — second launch sends toggle via IPC instead of starting new process
- [x] Terminal app support — detect `Terminal=true` in .desktop, launch via configured terminal
- [x] Icon display — resolve icon names to paths via icon theme spec
- [x] Frecency sorting — track launch counts, weight recent + frequent apps higher
- [x] Enter key activates selected row
- [x] Up/Down arrow key navigation in results

### Phase 5 — Make psh-clip functional
Clipboard daemon + picker.

- [x] Implement `zwlr-data-control-v1` clipboard monitoring via independent wayland-client connection
- [x] Store clipboard entries in `ClipHistory` as they arrive
- [x] Paste-on-select — when user picks a history item, set it as the active clipboard selection
- [x] Persistent history — save/load to `$XDG_DATA_HOME/psh/clip_history.json`
- [x] Image clipboard support (store as paths to cache files)
- [x] Search/filter in picker UI

### Phase 6 — Make psh-bar the integration hub
Biggest component. Depends on stable IPC + other components.

- [x] **Workspace module** — niri IPC event stream for workspace list, click to switch. ext-workspace-v1 fallback stub ready for implementation.
- [x] **Window title module** — niri IPC EventStream for focused window title, with title truncation
- [x] **Tray module** — `system-tray` crate integration for SNI items with icon rendering
- [x] **Volume module** — `wpctl` subprocess polling, scroll-to-adjust, click-to-mute
- [x] **Network module** — NetworkManager D-Bus interface via zbus, shows connection type + name
- [x] **IPC message routing** — bidirectional hub: client messages fanned out to modules, module messages broadcast to clients
- [x] **Configurable module loading** — `modules_left`/`modules_center`/`modules_right` from config with sensible defaults
- [x] **Click actions** — launcher button (ToggleLauncher), clipboard button (ShowClipboardHistory), notification count badge
- [x] **Module trait** — `BarModule` trait with `ModuleContext` (IPC channels + config), 10 modules registered

### Phase 7 — Make psh-lock security-complete
Built last. Security-critical — must be correct.

- [x] Bind `ext-session-lock-v1` protocol — acquire lock, get lock surfaces for all outputs
- [x] Keyboard input handling via `wl_keyboard` — accumulate password characters
- [x] Render password UI with tiny-skia + ab_glyph — centered input field with dots, clock, date, user info
- [x] PAM conversation function — supply password from keyboard input to PAM via conv_mock
- [x] On successful auth, destroy lock and exit
- [x] On failed auth, show error, clear password, allow retry
- [x] Multi-output — render lock surface on every output, handle hotplug
- [x] Idle integration — separate `psh-idle` binary with ext-idle-notify-v1 + logind PrepareForSleep
- [x] Ensure no input leaks through to underlying surfaces while locked (ext-session-lock-v1 guarantee + SIGTERM ignored)

### Phase 8 — Polish and integration

- [ ] `.gitignore` and CI (`cargo build`, `cargo test`, `cargo clippy`, `cargo fmt --check`)
- [ ] `README.md` with screenshots, install instructions, config reference
- [ ] Config validation — warn on unknown keys, suggest corrections
- [ ] Hot-reload for all components — broadcast `ConfigReloaded` via IPC, components re-read their section
- [ ] Theme hot-reload — watch CSS file, re-apply on change
- [ ] Graceful shutdown for all components (handle SIGTERM, clean up resources)
- [ ] `psh-ctl` CLI tool — send IPC commands (`psh-ctl lock`, `psh-ctl wall set /path/to/img`)
- [ ] Packaging — `Makefile` / `just` recipes for install, systemd unit installation
- [ ] AUR / Gentoo ebuild

## Key risks

| Risk | Mitigation |
|---|---|
| GTK4-layer-shell keyboard grab on niri | Test psh-launch early on niri, file upstream bugs if needed |
| SNI tray protocol complexity | Consider `system-tray` crate before hand-rolling |
| `zwlr-data-control` clipboard source lifetime | psh-clip daemon must stay alive and re-offer data on each paste |
| PAM thread safety in psh-lock | Dedicated thread for PAM, never on Wayland event loop |
| Multi-monitor hotplug | Every layer-shell component must handle output add/remove events |
