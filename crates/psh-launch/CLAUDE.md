# psh-launch

Application launcher. Full-screen overlay with fuzzy search over .desktop files.

## Stack

- GTK4 + gtk4-layer-shell (overlay window)
- nucleo-matcher 0.3 (fuzzy search)
- directories (XDG data dirs for .desktop file discovery)
- psh-core with `gtk` feature

## How it works

1. On activate, loads all .desktop files from XDG application directories
2. Shows a layer-shell overlay with a search entry and a scrollable list of results
3. As the user types, filters entries using nucleo fuzzy matching, sorted by score
4. Activating a row launches the app via `sh -c "{exec}"` and closes the window
5. Escape closes the window

## Current state — partial

**Working:**
- .desktop file parsing (`desktop.rs`): reads `Name`, `Exec`, `Comment`, `Icon`, `Terminal`, `NoDisplay` fields
- Strips field codes (`%f`, `%F`, `%u`, `%U`) from Exec
- Fuzzy search with nucleo, results sorted by match score
- GTK4 overlay with exclusive keyboard grab
- Escape to close, row activation to launch
- Deduplication by exec, alphabetical initial sort

**Missing (see PLAN.md Phase 4):**
- IPC listener for `ToggleLauncher` (currently only shows on app activate, no toggle)
- Single-instance prevention (second launch should toggle via IPC)
- Terminal app support (`Terminal=true` should launch in configured terminal)
- Icon display (icon names are parsed but not rendered)
- Frecency sorting (track and weight frequently/recently launched apps)
- Enter key to activate the currently selected row
- Up/Down arrow navigation in the result list

## Key files

- `main.rs` — GTK app, search entry wiring, fuzzy match loop, row activation handler
- `desktop.rs` — `DesktopEntry` struct, `load_desktop_entries()`, `parse_desktop_file()`

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
