# psh-clip

Clipboard manager. Background daemon that monitors clipboard changes + GTK4 picker overlay.

## Stack

- GTK4 + gtk4-layer-shell (picker UI)
- async-channel (IPC thread -> GTK thread)
- psh-core with `gtk` feature

## How it works

1. GTK4 app starts, creates a `ClipHistory` instance
2. Spawns a background thread that connects to the psh-bar IPC hub
3. When `ShowClipboardHistory` IPC message arrives, signals the GTK thread
4. GTK thread opens a picker overlay listing clipboard history
5. Escape closes the picker

## Current state — partial

**Working:**
- `ClipHistory` data structure (`history.rs`): thread-safe VecDeque with max capacity, deduplication, LIFO ordering. Has 3 unit tests.
- GTK4 picker overlay with scrollable list
- IPC listener for `ShowClipboardHistory`
- Escape to close

**Missing (see PLAN.md Phase 5):**
- Actual clipboard monitoring via `zwlr-data-control-v1` (the history struct exists but nothing populates it)
- Paste-on-select (picking an item should set it as the active clipboard)
- Persistent history across restarts (save/load to disk)
- Image clipboard support
- Search/filter in picker UI

## Key files

- `main.rs` — GTK app, IPC listener, `show_picker()` builds the overlay
- `history.rs` — `ClipHistory` struct with `push()`, `items()`, `clear()`

## Tests

3 unit tests in `history.rs`: push_and_retrieve, deduplicates, respects_max.

Run: `cargo test -p psh-clip`

## Config

```toml
[clip]
max_history = 100
```
