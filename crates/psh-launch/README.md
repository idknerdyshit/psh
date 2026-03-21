# psh-launch

Application launcher for the psh desktop environment. Full-screen overlay with fuzzy search over `.desktop` files.

## Features

- Reads `.desktop` files from XDG application directories
- Fuzzy matching powered by [nucleo](https://github.com/helix-editor/nucleo)
- Results sorted by match score
- Layer-shell overlay with exclusive keyboard grab
- Escape to close

## Desktop file search paths

1. `$XDG_DATA_HOME/applications/` (usually `~/.local/share/applications/`)
2. `/usr/share/applications/`
3. `/usr/local/share/applications/`

## Configuration

```toml
[launch]
# terminal = "foot"
# max_results = 20
```

## Running

```sh
cargo run -p psh-launch
```

Requires a running Wayland session with layer-shell support.
