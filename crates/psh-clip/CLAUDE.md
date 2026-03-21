# psh-clip

Clipboard manager daemon with GTK4 picker overlay.

## Stack

- GTK4 + gtk4-layer-shell (picker UI)
- wayland-client + wayland-protocols-wlr (clipboard monitoring via `zwlr-data-control-v1`)
- async-channel (IPC/clipboard threads -> GTK thread)
- psh-core with `gtk` feature

## How it works

1. GTK4 app starts, loads persisted history from `$XDG_DATA_HOME/psh/clip_history.json`
2. Spawns clipboard monitor thread with independent Wayland connection (separate from GTK's)
3. Monitor binds `zwlr_data_control_manager_v1`, gets data device for the seat
4. On clipboard change: reads offer data via pipe, creates `ClipEntry`, pushes to history
5. Spawns IPC listener thread connecting to psh-bar hub
6. On `ShowClipboardHistory` IPC message: opens picker overlay
7. Picker has search/filter, image thumbnails, paste-on-select via row activation
8. Paste-on-select: sends entry to monitor thread, which creates a data-control source

## Architecture — three threads

- **Main thread (GTK4)**: GLib main loop, picker UI, receives events via async-channel
- **IPC thread**: tokio runtime, listens for `ShowClipboardHistory` from psh-bar
- **Clipboard monitor thread**: independent wayland-client connection, data-control protocol

## Key files

- `main.rs` — GTK app, thread wiring, `show_picker()` with search and paste-on-select
- `history.rs` — `ClipEntry` enum (Text/Image), `ClipHistory` (thread-safe, dedup, search)
- `monitor.rs` — `zwlr-data-control-v1` clipboard monitoring + setting via Wayland Dispatch
- `persist.rs` — save/load history JSON, image cache management, orphan pruning

## Tests

39 unit tests: 22 in history (push, dedup, max, load_from, search, display, clear, serde roundtrip, multiline, empty content, image filename match, eviction ordering), 8 in monitor (MIME selection, pipe, write_to_fd roundtrip, large payload, data integrity), 9 in persist (serde roundtrip, tagged format, missing file, corrupted JSON, orphan pruning, no-orphans noop, empty-history prune-all, parent dir creation).

Run: `cargo test -p psh-clip`

## Config

```toml
[clip]
max_history = 100       # max entries to keep
persist = true          # save/load history to disk
image_support = true    # monitor image clipboard entries
max_image_bytes = 10000000  # skip images larger than this
```
