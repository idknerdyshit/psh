# psh-clip

Clipboard history manager for the psh desktop environment. A long-lived daemon that monitors clipboard changes via `zwlr-data-control-v1` and provides a GTK4 picker overlay for browsing and pasting from history.

## Features

- Clipboard monitoring via `zwlr-data-control-v1` (independent Wayland connection)
- Text and image clipboard support (PNG, JPEG, BMP)
- Persistent history at `$XDG_DATA_HOME/psh/clip_history.json`
- Image caching at `$XDG_CACHE_HOME/psh/clips/` with orphan pruning
- Thread-safe deduplicating history with configurable max capacity
- GTK4 picker overlay with search/filter and image thumbnails
- Paste-on-select — activating a row sets the clipboard via data-control source
- Self-copy detection to avoid feedback loops
- Escape to close picker

## Configuration

```toml
[clip]
max_history = 100           # max entries to keep
persist = true              # save/load history to disk
image_support = true        # monitor image clipboard entries
max_image_bytes = 10000000  # skip images larger than this (bytes)
```

## Tests

```sh
cargo test -p psh-clip
```

39 unit tests covering history operations, clipboard monitoring helpers, and persistence.

## Running

```sh
cargo run -p psh-clip
```

Requires a running Wayland session with `zwlr-data-control-v1` support (wlroots-based compositors, niri). Connects to psh-bar's IPC hub for `ShowClipboardHistory` messages; runs standalone if the hub is unavailable.
