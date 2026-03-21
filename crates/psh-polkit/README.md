# psh-polkit

Polkit authentication agent for the psh desktop environment. Shows a GTK4 password dialog when a privileged action is requested.

## Features

- Layer-shell overlay dialog with exclusive keyboard grab
- Password entry with peek toggle
- Cancel and authenticate buttons
- Communicates with polkit over D-Bus (system bus)

## Configuration

```toml
[polkit]
# No config fields yet
```

## Testing

```sh
pkexec ls  # triggers a polkit auth prompt
```

## Running

```sh
cargo run -p psh-polkit
```

Requires a running Wayland session with layer-shell support.
