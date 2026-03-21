# psh-bar

System bar and IPC hub for the psh desktop environment.

psh-bar renders a configurable status bar using GTK4 and Wayland layer-shell, and acts as the central message router -- all other psh components connect to its IPC socket.

## Features

- Layer-shell panel anchored to top or bottom edge
- Modular layout with left, center, and right sections
- Built-in modules: clock, battery, workspaces
- IPC hub at `$XDG_RUNTIME_DIR/psh.sock`

## Configuration

```toml
[bar]
position = "top"     # top | bottom
# height = 32
# modules_left = ["workspaces"]
# modules_center = ["clock"]
# modules_right = ["battery"]
```

## Running

```sh
cargo run -p psh-bar
```

Requires a running Wayland session with layer-shell support.
