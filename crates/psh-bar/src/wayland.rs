//! Shared Wayland monitor thread for workspace and window-title protocols.
//!
//! Opens an independent Wayland connection (separate from GTK's) on a background
//! thread, subscribes to `ext-workspace-v1` for workspace state and
//! `zwlr-foreign-toplevel-management-v1` for focused window title.
//!
//! Both protocols share one connection and one poll loop. Modules call
//! [`workspace_handle`] or [`title_handle`] to get channel endpoints; the
//! thread is spawned lazily on the first call via `OnceLock`.

use std::collections::{HashMap, HashSet};
use std::sync::{mpsc, Mutex, OnceLock};

use tracing::{debug, error, info, warn};
use wayland_client::protocol::{wl_output, wl_registry, wl_seat};
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle, WEnum};
use wayland_protocols::ext::workspace::v1::client::{
    ext_workspace_group_handle_v1, ext_workspace_handle_v1, ext_workspace_manager_v1,
};
use wayland_protocols_wlr::foreign_toplevel::v1::client::{
    zwlr_foreign_toplevel_handle_v1, zwlr_foreign_toplevel_manager_v1,
};

use crate::modules::workspaces::{WorkspaceCommand, WorkspaceInfo, WorkspaceUpdate};

// ── Public API ──

/// Channel endpoints for workspace updates.
pub struct WaylandWorkspaceHandle {
    /// Receives workspace state updates from the Wayland thread.
    pub rx: async_channel::Receiver<WorkspaceUpdate>,
    /// Sends workspace-switch commands to the Wayland thread.
    pub cmd_tx: mpsc::Sender<WorkspaceCommand>,
}

/// Channel endpoints for window title updates.
pub struct WaylandTitleHandle {
    /// Receives the focused window's title (or `None` when no window is focused).
    pub rx: async_channel::Receiver<Option<String>>,
}

/// Returns workspace channel endpoints, spawning the monitor thread if needed.
///
/// Returns `None` if the Wayland connection fails or the compositor does not
/// advertise `ext_workspace_manager_v1`.
pub fn workspace_handle() -> Option<WaylandWorkspaceHandle> {
    let inner = ensure_started()?;

    if !inner.has_workspaces {
        return None;
    }

    let (tx, rx) = async_channel::bounded(4);
    inner
        .ws_senders
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push(tx);

    Some(WaylandWorkspaceHandle {
        rx,
        cmd_tx: inner.ws_cmd_tx.clone(),
    })
}

/// Returns window-title channel endpoints, spawning the monitor thread if needed.
///
/// Returns `None` if the Wayland connection fails or the compositor does not
/// advertise `zwlr_foreign_toplevel_manager_v1`.
pub fn title_handle() -> Option<WaylandTitleHandle> {
    let inner = ensure_started()?;

    if !inner.has_toplevel {
        return None;
    }

    let (tx, rx) = async_channel::bounded(4);
    inner
        .title_senders
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push(tx);

    Some(WaylandTitleHandle { rx })
}

// ── Thread lifecycle ──

/// Shared state accessible from the main thread after the monitor is started.
struct MonitorInner {
    ws_senders: Mutex<Vec<async_channel::Sender<WorkspaceUpdate>>>,
    title_senders: Mutex<Vec<async_channel::Sender<Option<String>>>>,
    ws_cmd_tx: mpsc::Sender<WorkspaceCommand>,
    has_workspaces: bool,
    has_toplevel: bool,
}

static MONITOR: OnceLock<Option<&'static MonitorInner>> = OnceLock::new();

/// Ensures the monitor thread is running. Returns the shared handle, or `None`
/// if the Wayland connection failed entirely.
fn ensure_started() -> Option<&'static MonitorInner> {
    *MONITOR.get_or_init(|| {
        // Probe the Wayland connection and discover globals before spawning.
        let conn = match Connection::connect_to_env() {
            Ok(c) => c,
            Err(e) => {
                warn!("wayland monitor: failed to connect: {e}");
                return None;
            }
        };

        let mut event_queue: EventQueue<WaylandState> = conn.new_event_queue();
        let qh = event_queue.handle();

        let (ws_cmd_tx, ws_cmd_rx) = mpsc::channel::<WorkspaceCommand>();

        let mut state = WaylandState {
            ws_senders_ref: None,
            title_senders_ref: None,
            ws_cmd_rx,
            outputs: HashMap::new(),
            ws_manager: None,
            groups: HashMap::new(),
            workspaces: HashMap::new(),
            ws_id_counter: 1,
            ws_id_map: HashMap::new(),
            ws_handle_map: HashMap::new(),
            toplevel_manager: None,
            toplevels: HashMap::new(),
            focused_toplevel: None,
        };

        conn.display().get_registry(&qh, ());
        event_queue.roundtrip(&mut state).unwrap_or_else(|e| {
            error!("wayland monitor: initial roundtrip failed: {e}");
            0
        });

        let has_workspaces = state.ws_manager.is_some();
        let has_toplevel = state.toplevel_manager.is_some();

        if !has_workspaces && !has_toplevel {
            info!("wayland monitor: no workspace or toplevel protocols available");
            return None;
        }

        if has_workspaces {
            info!("wayland monitor: ext_workspace_manager_v1 available");
        }
        if has_toplevel {
            info!("wayland monitor: zwlr_foreign_toplevel_manager_v1 available");
        }

        // Leak the inner handle so it lives forever (singleton pattern).
        let inner = Box::leak(Box::new(MonitorInner {
            ws_senders: Mutex::new(Vec::new()),
            title_senders: Mutex::new(Vec::new()),
            ws_cmd_tx,
            has_workspaces,
            has_toplevel,
        }));

        // Share the sender lists with the Wayland thread by giving it references
        // to the same Mutex'ed Vecs inside MonitorInner.
        let ws_senders_ref: &'static Mutex<Vec<async_channel::Sender<WorkspaceUpdate>>> =
            &inner.ws_senders;
        let title_senders_ref: &'static Mutex<Vec<async_channel::Sender<Option<String>>>> =
            &inner.title_senders;

        // Set the sender refs before handing state to the thread.
        state.ws_senders_ref = Some(ws_senders_ref);
        state.title_senders_ref = Some(title_senders_ref);

        if let Err(e) = std::thread::Builder::new()
            .name("wayland-monitor".into())
            .spawn(move || {
                run_monitor(conn, event_queue, state);
            })
        {
            error!("wayland monitor: failed to spawn thread: {e}");
            return None;
        }

        Some(inner)
    })
}

// ── Monitor thread ──

/// Runs the Wayland event loop on a dedicated thread.
fn run_monitor(
    conn: Connection,
    mut event_queue: EventQueue<WaylandState>,
    mut state: WaylandState,
) {
    // Second roundtrip to get initial workspace/toplevel state.
    let _ = event_queue.roundtrip(&mut state);

    info!("wayland monitor: entering event loop");

    loop {
        if let Err(e) = conn.flush() {
            error!("wayland monitor: flush error: {e}");
            break;
        }

        let guard = match conn.prepare_read() {
            Some(g) => g,
            None => {
                if let Err(e) = event_queue.dispatch_pending(&mut state) {
                    error!("wayland monitor: dispatch error: {e}");
                    break;
                }
                handle_workspace_commands(&mut state);
                continue;
            }
        };

        let wayland_fd = guard.connection_fd();
        let timeout = rustix::time::Timespec {
            tv_sec: 0,
            tv_nsec: 100_000_000, // 100ms
        };
        let poll_result = rustix::event::poll(
            &mut [rustix::event::PollFd::new(
                &wayland_fd,
                rustix::event::PollFlags::IN,
            )],
            Some(&timeout),
        );

        match poll_result {
            Ok(n) if n > 0 => {
                if let Err(e) = guard.read() {
                    error!("wayland monitor: read error: {e}");
                    break;
                }
            }
            Ok(_) => {
                drop(guard);
            }
            Err(e) => {
                error!("wayland monitor: poll error: {e}");
                drop(guard);
                break;
            }
        }

        if let Err(e) = event_queue.dispatch_pending(&mut state) {
            error!("wayland monitor: dispatch error: {e}");
            break;
        }

        handle_workspace_commands(&mut state);
    }
}

/// Processes pending workspace-switch commands from GTK button clicks.
fn handle_workspace_commands(state: &mut WaylandState) {
    while let Ok(WorkspaceCommand::Focus(id)) = state.ws_cmd_rx.try_recv() {
        if let Some(handle) = state.ws_handle_map.get(&id) {
            handle.activate();
            if let Some(manager) = &state.ws_manager {
                manager.commit();
            }
            debug!("wayland: sent activate for workspace {id}");
        } else {
            warn!("wayland: no workspace handle for id {id}");
        }
    }
}

// ── Internal state ──

/// Data for a workspace group (tracks output assignments).
struct GroupData {
    /// wl_output proxy IDs assigned to this group.
    outputs: HashSet<wayland_client::backend::ObjectId>,
    /// Workspace proxy IDs belonging to this group.
    workspaces: Vec<wayland_client::backend::ObjectId>,
}

/// Data for a single workspace handle.
struct WsData {
    name: Option<String>,
    coordinates: Vec<u32>,
    state: u32,
    group: Option<wayland_client::backend::ObjectId>,
    // Pending values applied atomically on manager `done`.
    pending_name: Option<Option<String>>,
    pending_state: Option<u32>,
    pending_coordinates: Option<Vec<u32>>,
}

/// Data for a single foreign-toplevel handle.
struct ToplevelData {
    title: String,
    activated: bool,
    // Pending values applied atomically on handle `done`.
    pending_title: Option<String>,
    pending_activated: Option<bool>,
}

/// Full Wayland state for the monitor thread.
struct WaylandState {
    // References to the shared sender lists in MonitorInner (set after thread handoff).
    ws_senders_ref: Option<&'static Mutex<Vec<async_channel::Sender<WorkspaceUpdate>>>>,
    title_senders_ref: Option<&'static Mutex<Vec<async_channel::Sender<Option<String>>>>>,
    ws_cmd_rx: mpsc::Receiver<WorkspaceCommand>,

    // wl_output name tracking
    outputs: HashMap<wayland_client::backend::ObjectId, String>,

    // ext-workspace-v1
    ws_manager: Option<ext_workspace_manager_v1::ExtWorkspaceManagerV1>,
    groups: HashMap<wayland_client::backend::ObjectId, GroupData>,
    workspaces: HashMap<wayland_client::backend::ObjectId, WsData>,
    ws_id_counter: u64,
    /// Protocol object ID → synthetic numeric ID for WorkspaceInfo.
    ws_id_map: HashMap<wayland_client::backend::ObjectId, u64>,
    /// Synthetic numeric ID → protocol handle (for activate requests).
    ws_handle_map: HashMap<u64, ext_workspace_handle_v1::ExtWorkspaceHandleV1>,

    // zwlr-foreign-toplevel-management-v1
    toplevel_manager: Option<zwlr_foreign_toplevel_manager_v1::ZwlrForeignToplevelManagerV1>,
    toplevels: HashMap<wayland_client::backend::ObjectId, ToplevelData>,
    focused_toplevel: Option<wayland_client::backend::ObjectId>,
}

// ── Broadcast helpers ──

/// Sends a workspace update to all registered subscribers, pruning closed channels.
fn broadcast_ws(
    senders: &Mutex<Vec<async_channel::Sender<WorkspaceUpdate>>>,
    update: &WorkspaceUpdate,
) {
    let mut locked = senders.lock().unwrap_or_else(|e| e.into_inner());
    locked.retain(|tx| !tx.is_closed());
    for tx in locked.iter() {
        let _ = tx.try_send(update.clone());
    }
}

/// Sends a title update to all registered subscribers, pruning closed channels.
fn broadcast_title(
    senders: &Mutex<Vec<async_channel::Sender<Option<String>>>>,
    title: &Option<String>,
) {
    let mut locked = senders.lock().unwrap_or_else(|e| e.into_inner());
    locked.retain(|tx| !tx.is_closed());
    for tx in locked.iter() {
        let _ = tx.try_send(title.clone());
    }
}

// ── Dispatch implementations ──

impl Dispatch<wl_registry::WlRegistry, ()> for WaylandState {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
        {
            match interface.as_str() {
                "wl_output" => {
                    registry.bind::<wl_output::WlOutput, _, _>(name, version.min(4), qh, ());
                    debug!("bound wl_output v{}", version.min(4));
                }
                "wl_seat" => {
                    registry.bind::<wl_seat::WlSeat, _, _>(name, version.min(1), qh, ());
                    debug!("bound wl_seat v{version}");
                }
                "ext_workspace_manager_v1" => {
                    let manager = registry
                        .bind::<ext_workspace_manager_v1::ExtWorkspaceManagerV1, _, _>(
                            name,
                            version.min(1),
                            qh,
                            (),
                        );
                    state.ws_manager = Some(manager);
                    debug!("bound ext_workspace_manager_v1 v{version}");
                }
                "zwlr_foreign_toplevel_manager_v1" => {
                    let manager = registry
                        .bind::<zwlr_foreign_toplevel_manager_v1::ZwlrForeignToplevelManagerV1, _, _>(
                            name,
                            version.min(3),
                            qh,
                            (),
                        );
                    state.toplevel_manager = Some(manager);
                    debug!("bound zwlr_foreign_toplevel_manager_v1 v{version}");
                }
                _ => {}
            }
        }
    }
}

impl Dispatch<wl_output::WlOutput, ()> for WaylandState {
    fn event(
        state: &mut Self,
        output: &wl_output::WlOutput,
        event: wl_output::Event,
        _: &(),
        _: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        if let wl_output::Event::Name { name } = event {
            state.outputs.insert(output.id(), name);
        }
    }
}

impl Dispatch<wl_seat::WlSeat, ()> for WaylandState {
    fn event(
        _state: &mut Self,
        _seat: &wl_seat::WlSeat,
        _event: wl_seat::Event,
        _: &(),
        _: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
    }
}

// ── ext-workspace-v1 ──

impl Dispatch<ext_workspace_manager_v1::ExtWorkspaceManagerV1, ()> for WaylandState {
    fn event(
        state: &mut Self,
        _manager: &ext_workspace_manager_v1::ExtWorkspaceManagerV1,
        event: ext_workspace_manager_v1::Event,
        _: &(),
        _: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        match event {
            ext_workspace_manager_v1::Event::WorkspaceGroup { workspace_group } => {
                state.groups.insert(
                    workspace_group.id(),
                    GroupData {
                        outputs: HashSet::new(),
                        workspaces: Vec::new(),
                    },
                );
            }
            ext_workspace_manager_v1::Event::Workspace { workspace } => {
                let obj_id = workspace.id();
                let synthetic_id = state.ws_id_counter;
                state.ws_id_counter += 1;
                state.ws_id_map.insert(obj_id.clone(), synthetic_id);
                state.ws_handle_map.insert(synthetic_id, workspace);
                state.workspaces.insert(
                    obj_id,
                    WsData {
                        name: None,
                        coordinates: Vec::new(),
                        state: 0,
                        group: None,
                        pending_name: None,
                        pending_state: None,
                        pending_coordinates: None,
                    },
                );
            }
            ext_workspace_manager_v1::Event::Done => {
                // Apply all pending state atomically.
                for ws in state.workspaces.values_mut() {
                    if let Some(name) = ws.pending_name.take() {
                        ws.name = name;
                    }
                    if let Some(s) = ws.pending_state.take() {
                        ws.state = s;
                    }
                    if let Some(coords) = ws.pending_coordinates.take() {
                        ws.coordinates = coords;
                    }
                }

                // Build full workspace info list and broadcast.
                let infos = build_workspace_infos(state);
                if let Some(senders) = &state.ws_senders_ref {
                    broadcast_ws(senders, &WorkspaceUpdate::Full(infos));
                }
            }
            ext_workspace_manager_v1::Event::Finished => {
                info!("wayland: workspace manager finished");
                state.ws_manager = None;
            }
            _ => {}
        }
    }
}

impl Dispatch<ext_workspace_group_handle_v1::ExtWorkspaceGroupHandleV1, ()> for WaylandState {
    fn event(
        state: &mut Self,
        group: &ext_workspace_group_handle_v1::ExtWorkspaceGroupHandleV1,
        event: ext_workspace_group_handle_v1::Event,
        _: &(),
        _: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        let group_id = group.id();
        match event {
            ext_workspace_group_handle_v1::Event::OutputEnter { output } => {
                if let Some(g) = state.groups.get_mut(&group_id) {
                    g.outputs.insert(output.id());
                }
            }
            ext_workspace_group_handle_v1::Event::OutputLeave { output } => {
                if let Some(g) = state.groups.get_mut(&group_id) {
                    g.outputs.remove(&output.id());
                }
            }
            ext_workspace_group_handle_v1::Event::WorkspaceEnter { workspace } => {
                let ws_id = workspace.id();
                if let Some(g) = state.groups.get_mut(&group_id)
                    && !g.workspaces.contains(&ws_id)
                {
                    g.workspaces.push(ws_id.clone());
                }
                if let Some(ws) = state.workspaces.get_mut(&ws_id) {
                    ws.group = Some(group_id.clone());
                }
            }
            ext_workspace_group_handle_v1::Event::WorkspaceLeave { workspace } => {
                let ws_id = workspace.id();
                if let Some(g) = state.groups.get_mut(&group_id) {
                    g.workspaces.retain(|id| *id != ws_id);
                }
                if let Some(ws) = state.workspaces.get_mut(&ws_id)
                    && ws.group.as_ref() == Some(&group_id)
                {
                    ws.group = None;
                }
            }
            ext_workspace_group_handle_v1::Event::Removed => {
                state.groups.remove(&group_id);
            }
            _ => {}
        }
    }
}

impl Dispatch<ext_workspace_handle_v1::ExtWorkspaceHandleV1, ()> for WaylandState {
    fn event(
        state: &mut Self,
        handle: &ext_workspace_handle_v1::ExtWorkspaceHandleV1,
        event: ext_workspace_handle_v1::Event,
        _: &(),
        _: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        let obj_id = handle.id();
        match event {
            ext_workspace_handle_v1::Event::Name { name } => {
                if let Some(ws) = state.workspaces.get_mut(&obj_id) {
                    ws.pending_name = Some(Some(name));
                }
            }
            ext_workspace_handle_v1::Event::Coordinates { coordinates } => {
                // Coordinates come as a byte array of u32 values (little-endian).
                let coords: Vec<u32> = coordinates
                    .chunks_exact(4)
                    .map(|c| u32::from_ne_bytes([c[0], c[1], c[2], c[3]]))
                    .collect();
                if let Some(ws) = state.workspaces.get_mut(&obj_id) {
                    ws.pending_coordinates = Some(coords);
                }
            }
            ext_workspace_handle_v1::Event::State {
                state: WEnum::Value(s),
            } => {
                if let Some(ws) = state.workspaces.get_mut(&obj_id) {
                    ws.pending_state = Some(s.bits());
                }
            }
            ext_workspace_handle_v1::Event::Removed => {
                if let Some(synthetic_id) = state.ws_id_map.remove(&obj_id) {
                    state.ws_handle_map.remove(&synthetic_id);
                }
                state.workspaces.remove(&obj_id);
                // Remove from any group.
                for g in state.groups.values_mut() {
                    g.workspaces.retain(|id| *id != obj_id);
                }
            }
            _ => {}
        }
    }
}

// ── zwlr-foreign-toplevel-management-v1 ──

impl Dispatch<zwlr_foreign_toplevel_manager_v1::ZwlrForeignToplevelManagerV1, ()>
    for WaylandState
{
    fn event(
        state: &mut Self,
        _manager: &zwlr_foreign_toplevel_manager_v1::ZwlrForeignToplevelManagerV1,
        event: zwlr_foreign_toplevel_manager_v1::Event,
        _: &(),
        _: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        match event {
            zwlr_foreign_toplevel_manager_v1::Event::Toplevel { toplevel } => {
                state.toplevels.insert(
                    toplevel.id(),
                    ToplevelData {
                        title: String::new(),
                        activated: false,
                        pending_title: None,
                        pending_activated: None,
                    },
                );
            }
            zwlr_foreign_toplevel_manager_v1::Event::Finished => {
                info!("wayland: toplevel manager finished");
                state.toplevel_manager = None;
            }
            _ => {}
        }
    }
}

impl Dispatch<zwlr_foreign_toplevel_handle_v1::ZwlrForeignToplevelHandleV1, ()> for WaylandState {
    fn event(
        state: &mut Self,
        handle: &zwlr_foreign_toplevel_handle_v1::ZwlrForeignToplevelHandleV1,
        event: zwlr_foreign_toplevel_handle_v1::Event,
        _: &(),
        _: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        let obj_id = handle.id();
        match event {
            zwlr_foreign_toplevel_handle_v1::Event::Title { title } => {
                if let Some(tl) = state.toplevels.get_mut(&obj_id) {
                    tl.pending_title = Some(title);
                }
            }
            zwlr_foreign_toplevel_handle_v1::Event::AppId { .. } => {}
            zwlr_foreign_toplevel_handle_v1::Event::State { state: tl_state } => {
                // State is an array of u32 values; activated = 2.
                let activated = tl_state
                    .chunks_exact(4)
                    .map(|c| u32::from_ne_bytes([c[0], c[1], c[2], c[3]]))
                    .any(|v| v == 2);
                if let Some(tl) = state.toplevels.get_mut(&obj_id) {
                    tl.pending_activated = Some(activated);
                }
            }
            zwlr_foreign_toplevel_handle_v1::Event::Done => {
                // Apply pending state atomically.
                let Some(tl) = state.toplevels.get_mut(&obj_id) else {
                    return;
                };
                if let Some(title) = tl.pending_title.take() {
                    tl.title = title;
                }
                if let Some(activated) = tl.pending_activated.take() {
                    tl.activated = activated;
                }

                // Determine if the focused window changed.
                if tl.activated {
                    state.focused_toplevel = Some(obj_id.clone());
                    let title = Some(tl.title.clone());
                    if let Some(senders) = &state.title_senders_ref {
                        broadcast_title(senders, &title);
                    }
                } else if state.focused_toplevel.as_ref() == Some(&obj_id) {
                    // This toplevel lost focus.
                    state.focused_toplevel = None;
                    if let Some(senders) = &state.title_senders_ref {
                        broadcast_title(senders, &None);
                    }
                }
            }
            zwlr_foreign_toplevel_handle_v1::Event::Closed => {
                if state.focused_toplevel.as_ref() == Some(&obj_id) {
                    state.focused_toplevel = None;
                    if let Some(senders) = &state.title_senders_ref {
                        broadcast_title(senders, &None);
                    }
                }
                state.toplevels.remove(&obj_id);
            }
            _ => {}
        }
    }
}

// ── Helpers ──

/// Builds the full workspace info list from current state.
fn build_workspace_infos(state: &WaylandState) -> Vec<WorkspaceInfo> {
    let mut infos = Vec::new();

    for (obj_id, ws) in &state.workspaces {
        let Some(&synthetic_id) = state.ws_id_map.get(obj_id) else {
            continue;
        };

        // Resolve output name from group.
        let output = ws.group.as_ref().and_then(|gid| {
            state.groups.get(gid).and_then(|g| {
                // Take the first output's name (groups typically have one output).
                g.outputs
                    .iter()
                    .find_map(|oid| state.outputs.get(oid))
                    .cloned()
            })
        });

        // Derive display index from coordinates (1-based).
        let idx = if let Some(&first) = ws.coordinates.first() {
            // Coordinates are 0-based, idx is 1-based.
            (first.saturating_add(1)).min(255) as u8
        } else {
            // Fallback: position within group's workspace list.
            ws.group
                .as_ref()
                .and_then(|gid| {
                    state.groups.get(gid).and_then(|g| {
                        g.workspaces
                            .iter()
                            .position(|id| id == obj_id)
                            .map(|p| (p as u8).saturating_add(1))
                    })
                })
                .unwrap_or(1)
        };

        let is_active =
            ws.state & ext_workspace_handle_v1::State::Active.bits() != 0;

        infos.push(WorkspaceInfo {
            id: synthetic_id,
            idx,
            name: ws.name.clone(),
            output,
            is_active,
            is_focused: is_active,
        });
    }

    // Sort by output then idx for consistent display.
    infos.sort_by(|a, b| a.output.cmp(&b.output).then_with(|| a.idx.cmp(&b.idx)));
    infos
}

/// Parses ext-workspace-v1 state bitfield.
#[cfg(test)]
fn parse_ws_state(bits: u32) -> (bool, bool, bool) {
    let active = bits & 1 != 0;
    let urgent = bits & 2 != 0;
    let hidden = bits & 4 != 0;
    (active, urgent, hidden)
}

/// Parses WLR toplevel state array for the activated flag.
#[cfg(test)]
fn parse_toplevel_activated(state_bytes: &[u8]) -> bool {
    state_bytes
        .chunks_exact(4)
        .map(|c| u32::from_ne_bytes([c[0], c[1], c[2], c[3]]))
        .any(|v| v == 2)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ws_state_active() {
        let (active, urgent, hidden) = parse_ws_state(1);
        assert!(active);
        assert!(!urgent);
        assert!(!hidden);
    }

    #[test]
    fn ws_state_urgent() {
        let (active, urgent, hidden) = parse_ws_state(2);
        assert!(!active);
        assert!(urgent);
        assert!(!hidden);
    }

    #[test]
    fn ws_state_hidden() {
        let (active, urgent, hidden) = parse_ws_state(4);
        assert!(!active);
        assert!(!urgent);
        assert!(hidden);
    }

    #[test]
    fn ws_state_combined() {
        let (active, urgent, hidden) = parse_ws_state(3);
        assert!(active);
        assert!(urgent);
        assert!(!hidden);
    }

    #[test]
    fn ws_state_empty() {
        let (active, urgent, hidden) = parse_ws_state(0);
        assert!(!active);
        assert!(!urgent);
        assert!(!hidden);
    }

    #[test]
    fn toplevel_activated_present() {
        // State array: [maximized=0, activated=2]
        let state: Vec<u8> = [0u32, 2u32]
            .iter()
            .flat_map(|v| v.to_ne_bytes())
            .collect();
        assert!(parse_toplevel_activated(&state));
    }

    #[test]
    fn toplevel_activated_absent() {
        // State array: [maximized=0, minimized=1]
        let state: Vec<u8> = [0u32, 1u32]
            .iter()
            .flat_map(|v| v.to_ne_bytes())
            .collect();
        assert!(!parse_toplevel_activated(&state));
    }

    #[test]
    fn toplevel_activated_empty() {
        assert!(!parse_toplevel_activated(&[]));
    }

    #[test]
    fn toplevel_activated_only() {
        let state: Vec<u8> = 2u32.to_ne_bytes().to_vec();
        assert!(parse_toplevel_activated(&state));
    }
}
