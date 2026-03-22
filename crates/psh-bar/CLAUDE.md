# psh-bar

System bar and IPC hub. The central component that other psh components connect to.

## Stack

- GTK4 + gtk4-layer-shell (bar panel)
- zbus 5 (tray/network D-Bus)
- niri-ipc (workspace/window title from niri)
- system-tray (SNI tray items)
- tokio (IPC hub + async backends on background threads)
- async-channel (GTK ↔ tokio communication)
- psh-core with `gtk` feature

## How it works

1. GTK4 app starts, applies theme
2. Reads `modules_left/center/right` from config (or uses sensible defaults)
3. Creates `BarModule` instances via `create_module()` registry
4. Each module gets a `ModuleContext` with bidirectional IPC channels
5. Spawns IPC hub on a background tokio thread
6. IPC hub fans out client messages to all module inbound channels and forwards module outbound messages to all IPC clients
7. Creates a layer-shell window with CenterBox layout (left/center/right sections)

## Module architecture

Every module implements the `BarModule` trait (`modules/mod.rs`):
```rust
pub trait BarModule {
    fn name(&self) -> &'static str;
    fn widget(&self, ctx: &ModuleContext) -> gtk4::Widget;
}
```

Modules that need async data spawn their own background tasks inside `widget()` using `glib::spawn_future_local` + `async_channel`.

## Available modules

| Name | File | Description |
|------|------|-------------|
| `claude` | `modules/claude.rs` | Claude.ai usage quota (session key auth, configurable format) |
| `clock` | `modules/clock.rs` | Live-updating `%H:%M:%S` label |
| `battery` | `modules/battery.rs` | Sysfs battery reader, configurable device |
| `workspaces` | `modules/workspaces.rs` | Niri IPC workspace buttons (ext-workspace-v1 fallback stub) |
| `window_title` | `modules/window_title.rs` | Focused window title from niri IPC |
| `volume` | `modules/volume.rs` | wpctl-based volume display, scroll-to-adjust, click-to-mute |
| `network` | `modules/network.rs` | NetworkManager D-Bus connection status |
| `tray` | `modules/tray.rs` | SNI system tray via system-tray crate |
| `launcher` | `modules/launcher_btn.rs` | Button sending ToggleLauncher IPC |
| `clipboard` | `modules/clipboard_btn.rs` | Button sending ShowClipboardHistory IPC |
| `notifications` | `modules/notifications.rs` | Notification count badge from IPC |

## IPC hub

The hub in `run_ipc_hub()`:
- Binds `$XDG_RUNTIME_DIR/psh.sock`, accepts client connections
- Broadcasts routable messages (ToggleLauncher, NotificationCount, etc.) to all clients
- Fans out incoming client messages to per-module inbound channels
- Reads from module outbound channel and broadcasts to IPC clients

## Config

```toml
[bar]
position = "top"     # top | bottom
# height = 32
# modules_left = ["workspaces", "window_title"]
# modules_center = ["clock"]
# modules_right = ["volume", "network", "battery", "tray"]
# show_all_workspaces = false
# max_title_length = 50
# volume_step = 5
# battery_device = "BAT0"
# claude_session_key = "sk-ant-sid01-..."  # or CLAUDE_SESSION_KEY env var
# claude_display = "percent"               # "percent" | "both"
# claude_poll_interval = 120               # seconds
```

## Tests

50 unit tests covering:
- Claude usage parsing, formatting, CSS class thresholds, display format
- Module registry (create known/unknown, default lists, name consistency)
- Battery sysfs parsing and icon selection
- Volume wpctl output parsing
- Network state formatting, NM type parsing, CSS class mapping
- Window title truncation (including unicode)
- Niri IPC event parsing round-trip

Run: `cargo test -p psh-bar`
