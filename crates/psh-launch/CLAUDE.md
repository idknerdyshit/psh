# psh-launch

Application launcher. Long-lived daemon with keyboard-driven overlay and fuzzy search over .desktop files.

## Stack

- GTK4 + gtk4-layer-shell (overlay window)
- nucleo-matcher 0.3 (fuzzy search)
- async-channel (IPC thread → GTK thread)
- directories (XDG data dirs for .desktop file discovery)
- psh-core with `gtk` feature

## How it works

1. Starts as a long-lived daemon with a hidden overlay window
2. Listens for `ToggleLauncher` IPC messages from psh-bar hub
3. On toggle: shows overlay, refreshes desktop entries, focuses search
4. As the user types, filters entries using nucleo fuzzy matching combined with frecency scores
5. Icons displayed via GTK4 icon theme resolution (`Image::from_icon_name`)
6. Enter activates the selected row, launching the app
7. Terminal apps (`Terminal=true`) launch in the configured terminal emulator
8. Escape hides the window (process stays running)
9. Frecency (frequency + recency) tracked in `$XDG_DATA_HOME/psh/launch_history.json`

## Current state — complete

**Working:**
- .desktop file parsing (`desktop.rs`): reads `Name`, `Exec`, `Comment`, `Icon`, `Terminal`, `NoDisplay` fields
- Strips field codes (`%f`, `%F`, `%u`, `%U`) from Exec
- Fuzzy search with nucleo, results sorted by combined fuzzy + frecency score
- GTK4 overlay with exclusive keyboard grab
- IPC client listening for `ToggleLauncher` messages (background tokio thread)
- Single-instance via GTK Application ID
- Icon display using GTK4 icon theme resolution
- Terminal app support (configurable terminal, auto-detection fallback)
- Frecency sorting with persistent JSON storage
- Enter to activate selected row, Escape to hide
- Up/Down arrow navigation (GTK ListBox default)
- Auto-selects first row on results change
- Desktop entries refreshed on each show (picks up new installs)
- Deduplication by exec, frecency-first then alphabetical default sort

## Key files

- `main.rs` — GTK daemon, IPC listener, toggle logic, search wiring, row activation, keyboard handling
- `desktop.rs` — `DesktopEntry` struct, `load_desktop_entries()`, `parse_desktop_file()`
- `frecency.rs` — `FrecencyTracker`: load/save JSON, record launches, score by frequency × recency weight

## Tests

4 unit tests in `frecency.rs`: empty_tracker_scores_zero, record_increases_score, recency_weight_decays, multiple_records_increase_score.

Run: `cargo test -p psh-launch`

## Config

```toml
[launch]
# terminal = "foot"
# max_results = 20
```

## Desktop file search paths

1. `$XDG_DATA_HOME/applications/` (usually `~/.local/share/applications/`)
2. `/usr/share/applications/`
3. `/usr/local/share/applications/`
