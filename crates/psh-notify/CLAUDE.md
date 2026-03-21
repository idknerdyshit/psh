# psh-notify

Notification daemon implementing `org.freedesktop.Notifications` D-Bus interface.

## Stack

- GTK4 + gtk4-layer-shell (single overlay window)
- zbus 5 (D-Bus server)
- async-channel (D-Bus thread -> GTK main thread)
- psh-core with `gtk` feature

## How it works

1. GTK4 app starts, applies theme
2. Spawns a background thread running a tokio runtime with the D-Bus server + IPC client
3. D-Bus server owns `org.freedesktop.Notifications` on the session bus
4. When `Notify` is called, sends a `Notification` struct over `async-channel` to the GTK thread
5. GTK thread's `NotificationManager` appends notification widgets to a single layer-shell overlay window (top-right, vertical stack), auto-dismisses after timeout
6. IPC client sends `NotificationCount` updates to psh-bar hub with automatic reconnection

## Current state

**Working:**
- D-Bus server with `GetCapabilities`, `Notify`, `CloseNotification`, `GetServerInformation`
- `NotificationClosed` and `ActionInvoked` D-Bus signal emission
- Single overlay window with vertical Box stacking (hidden when no notifications)
- Urgency levels with CSS classes (`.psh-notify-urgency-{low,normal,critical}`)
- Action buttons (non-default rendered as buttons, `default` action on body click)
- Visual replace-id (updates existing popup content in-place)
- App icon display (icon-name lookup + `image-data`/`image_data`/`icon_data` hints)
- IPC `NotificationCount` broadcast to psh-bar
- Body markup sanitization (allowed: `<b>`, `<i>`, `<u>`, `</a>`; all else escaped)
- Configurable timeout with fd.o spec compliance (0=never, -1=default, >0=ms)
- Critical notifications never auto-dismiss

**Known limitations:**
- `<a href="...">` markup not restored (link text renders, but without hyperlink)

## Key files

- `main.rs` — GTK app setup, IPC client with reconnection, channel wiring
- `dbus_server.rs` — `NotificationServer` D-Bus interface, hint parsing (urgency, image-data, actions)
- `manager.rs` — `NotificationManager` owns the single overlay window + stack Box, handles popup lifecycle: append/remove widgets, replace-id, timeouts, dismiss, actions, markup sanitization

## Testing

```sh
notify-send "Test Title" "Test body text"
notify-send -u critical "Urgent" "This is critical"
notify-send -t 10000 "Long" "This stays for 10 seconds"
notify-send -t 0 "Persistent" "Never auto-dismisses"
notify-send "Markup" "<b>bold</b> and <i>italic</i>"
```

## Config

```toml
[notify]
max_visible = 5
default_timeout_ms = 5000
width = 380
gap = 10           # currently unused — spacing handled by CSS
icon_size = 48
```
