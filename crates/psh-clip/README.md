# psh-clip

Clipboard history manager for the psh desktop environment. Background daemon that tracks clipboard changes and provides a GTK4 picker overlay.

## Features

- Thread-safe clipboard history with configurable max capacity
- Deduplication (re-copying an item moves it to the top)
- GTK4 picker overlay triggered via IPC
- Escape to close

## Configuration

```toml
[clip]
max_history = 100
```

## Tests

```sh
cargo test -p psh-clip
```

## Running

```sh
cargo run -p psh-clip
```

Requires a running Wayland session with layer-shell support. Connects to psh-bar's IPC hub for `ShowClipboardHistory` messages.
