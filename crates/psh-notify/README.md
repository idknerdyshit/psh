# psh-notify

Notification daemon for the psh desktop environment. Implements the `org.freedesktop.Notifications` D-Bus interface.

## Features

- Layer-shell overlay popups (top-right corner)
- Auto-dismiss with configurable timeout
- Supports replace-id for updating existing notifications
- Styled via GTK4 CSS theming

## Configuration

```toml
[notify]
max_visible = 5
default_timeout_ms = 5000
```

## Testing

```sh
notify-send "Test Title" "Test body text"
notify-send -u critical "Urgent" "This is critical"
notify-send -t 10000 "Long" "This stays for 10 seconds"
```

## Running

```sh
cargo run -p psh-notify
```

Requires a running Wayland session with layer-shell support. Only one notification daemon should be active at a time.
