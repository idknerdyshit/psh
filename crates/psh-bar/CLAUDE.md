# psh-bar

System bar and IPC hub. The central component that other psh components connect to.

## Stack

- GTK4 + gtk4-layer-shell (bar panel)
- zbus 5 (for future tray/volume/network modules)
- tokio (IPC hub runs on background thread)
- async-channel
- psh-core with `gtk` feature

## How it works

1. GTK4 app starts, applies theme, spawns IPC hub on a background thread
2. Creates a layer-shell window anchored left+right and top (or bottom per config)
3. Uses a `CenterBox` with left/center/right module containers
4. IPC hub binds `$XDG_RUNTIME_DIR/psh.sock`, accepts client connections, handles ping/pong

## Current state — partial

**Working:**
- Layer-shell panel with configurable position (top/bottom) and height
- CenterBox layout with left/center/right sections
- IPC hub: bind, accept, per-client task, ping/pong handling
- Clock module: live-updating `%H:%M:%S` via `glib::timeout_add_seconds_local`
- Battery module: reads `/sys/class/power_supply/BAT0/capacity` and `status`, updates every 30s
- Workspaces module: static placeholder buttons 1-5

**Missing (see PLAN.md Phase 6):**
- Workspace module: niri IPC socket integration, ext-workspace-v1 fallback
- Window title module
- Tray module (SNI protocol — consider `system-tray` crate)
- Volume module (PipeWire/PulseAudio)
- Network module (NetworkManager D-Bus)
- IPC message routing (forward messages to connected clients, not just ping/pong)
- Configurable module loading from `modules_left`/`modules_center`/`modules_right` config
- `BarModule` trait and dynamic module instantiation
- Click actions (launcher button, clip button)

## Key files

- `main.rs` — GTK app, layer-shell setup, module wiring, `run_ipc_hub()` async function
- `modules/mod.rs` — module re-exports
- `modules/clock.rs` — live clock label
- `modules/battery.rs` — sysfs battery reader
- `modules/workspaces.rs` — static placeholder

## Module structure

Each module exposes a `widget() -> gtk4::Widget` function. When implementing the `BarModule` trait, this will become:

```rust
trait BarModule {
    fn name(&self) -> &str;
    fn widget(&self) -> gtk4::Widget;
    async fn start(&self, config: &ModuleConfig) -> Result<()>;
}
```

## IPC hub

The hub in `run_ipc_hub()` currently only handles `Ping -> Pong`. When adding message routing:
- Maintain a `Vec` or `HashMap` of connected client streams
- On receiving a routable message (e.g. `ToggleLauncher`), forward to all clients
- Handle client disconnects gracefully (remove from list)

## Config

```toml
[bar]
position = "top"     # top | bottom
# height = 32
# modules_left = ["workspaces"]
# modules_center = ["clock"]
# modules_right = ["battery"]
```
