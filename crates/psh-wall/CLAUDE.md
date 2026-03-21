# psh-wall

Wallpaper manager. Simplest Wayland client — no GTK, uses smithay-client-toolkit directly.

## Stack

- smithay-client-toolkit 0.19 (wlr-layer-shell, wl_shm, calloop integration)
- wayland-client 0.31
- image 0.25 (loading + resizing)
- signal-hook 0.3 (SIGTERM/SIGINT handling)
- psh-core (config, IPC, logging — no `gtk` feature)

## How it works

1. Connects to Wayland, binds compositor + layer-shell + shm
2. Uses calloop `EventLoop` with `WaylandSource` + IPC channel + signal flag
3. Creates one layer surface per output via `OutputHandler::new_output()` (Layer::Background, Anchor::all(), exclusive zone -1)
4. On configure: allocates an shm buffer at HiDPI-scaled dimensions, renders wallpaper per configured mode, attaches + commits
5. Falls back to Catppuccin base color (#1e1e2e) if no wallpaper path is configured
6. IPC listener runs on a background thread (tokio), receives `SetWallpaper` messages and redraws all surfaces
7. Shuts down cleanly on SIGTERM/SIGINT via `signal-hook` AtomicBool flag

## Key types

- `OutputSurface` — per-output state: LayerSurface, buffer, dimensions, scale factor
- `WallState` — main state: SCTK globals, `Vec<OutputSurface>`, loaded image, wall mode
- `WallState::draw(idx)` — renders wallpaper to the shm buffer for surface at index
- `WallState::redraw_all()` — redraws all configured surfaces (used after IPC wallpaper change)
- `IpcCommand` — internal enum for calloop channel messages from IPC thread

## Wallpaper modes

| Mode | Behavior |
|------|----------|
| `fill` | Scale to cover, crop overflow (default) |
| `fit` | Scale to fit within bounds, letterbox with fallback color |
| `stretch` | Resize to exact dimensions, ignoring aspect ratio |
| `center` | Place at original size, centered, no scaling |
| `tile` | Repeat at original size across the surface |

## Config

```toml
[wall]
path = "/path/to/wallpaper.png"
mode = "fill"  # fill | fit | center | stretch | tile
```

## Important SCTK patterns

- `WaylandSurface` trait must be in scope to call `.wl_surface()` on a `LayerSurface`
- Buffer format is ARGB8888 (byte order: B, G, R, A)
- Buffer dimensions are surface dimensions * scale_factor for HiDPI
- The `delegate_*!` macros at the bottom wire up Wayland protocol dispatching — every handler trait needs its delegate macro
- Surfaces are stored in a `Vec` (not HashMap) because `WlOutput` doesn't implement `Hash`
- calloop + WaylandSource replaces bare `blocking_dispatch` — allows multiplexing Wayland events with IPC channel and signal handling
