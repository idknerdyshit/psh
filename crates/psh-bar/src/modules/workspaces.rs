//! Workspace module — displays workspace buttons with live updates.
//!
//! Uses niri IPC when `$NIRI_SOCKET` is set, otherwise falls back to
//! the `ext-workspace-v1` Wayland protocol. Shows a placeholder if
//! neither backend is available.

use gtk4::glib;
use gtk4::prelude::*;

use super::{BarModule, ModuleContext};
use crate::niri;

/// Information about a single workspace, abstracted from the backend.
#[derive(Debug, Clone)]
pub(crate) struct WorkspaceInfo {
    /// Unique workspace identifier.
    pub id: u64,
    /// Display index (1-based).
    pub idx: u8,
    /// Optional workspace name.
    pub name: Option<String>,
    /// Output this workspace is on.
    pub output: Option<String>,
    /// Whether this workspace is active on its output.
    pub is_active: bool,
    /// Whether this workspace is the globally focused one.
    pub is_focused: bool,
}

/// Commands sent from the GTK thread to the backend.
#[derive(Debug, Clone)]
enum WorkspaceCommand {
    /// Focus the workspace with this id.
    Focus(u64),
}

/// Displays workspace buttons that track the active workspace.
///
/// Clicking a button switches to that workspace. The module auto-detects
/// whether to use niri IPC or ext-workspace-v1.
pub struct WorkspacesModule;

impl BarModule for WorkspacesModule {
    fn name(&self) -> &'static str {
        "workspaces"
    }

    fn widget(&self, ctx: &ModuleContext) -> gtk4::Widget {
        let container = gtk4::Box::new(gtk4::Orientation::Horizontal, 4);
        container.add_css_class("psh-bar-workspaces");

        if niri::is_available() {
            setup_niri_workspaces(&container, ctx);
        } else {
            setup_placeholder_workspaces(&container);
            // TODO(phase6): ext-workspace-v1 fallback
        }

        container.upcast()
    }
}

/// Set up workspace buttons driven by the niri IPC event stream.
fn setup_niri_workspaces(container: &gtk4::Box, ctx: &ModuleContext) {
    let (ws_tx, ws_rx) = async_channel::bounded::<Vec<WorkspaceInfo>>(4);
    let (cmd_tx, cmd_rx) = async_channel::bounded::<WorkspaceCommand>(4);

    // Background task: connect to niri event stream on the shared runtime
    let ws_tx_clone = ws_tx.clone();
    ctx.rt.spawn(async move {
        if let Err(e) = run_niri_workspace_backend(ws_tx_clone, cmd_rx).await {
            tracing::error!("niri workspace backend error: {e}");
        }
    });

    // GTK side: rebuild buttons on workspace updates
    let container = container.clone();
    let cmd_tx_clone = cmd_tx.clone();
    let show_all = ctx.config.show_all_workspaces;
    glib::spawn_future_local(async move {
        while let Ok(workspaces) = ws_rx.recv().await {
            rebuild_workspace_buttons(&container, &workspaces, &cmd_tx_clone, show_all);
        }
    });
}

/// Run the niri workspace backend: subscribe to events and relay workspace updates.
///
/// Automatically reconnects to the niri event stream with exponential backoff
/// if the connection drops (e.g., niri restarts).
async fn run_niri_workspace_backend(
    tx: async_channel::Sender<Vec<WorkspaceInfo>>,
    cmd_rx: async_channel::Receiver<WorkspaceCommand>,
) -> psh_core::Result<()> {
    use tokio::io::AsyncBufReadExt;

    const MAX_BACKOFF: std::time::Duration = std::time::Duration::from_secs(30);

    // Handle workspace focus commands in a separate task
    tokio::spawn(async move {
        while let Ok(WorkspaceCommand::Focus(id)) = cmd_rx.recv().await {
            let req = niri_ipc::Request::Action(niri_ipc::Action::FocusWorkspace {
                reference: niri_ipc::WorkspaceReferenceArg::Id(id),
            });
            if let Err(e) = niri::request(&req).await {
                tracing::warn!("failed to focus workspace {id}: {e}");
            }
        }
    });

    let mut backoff = std::time::Duration::from_secs(2);

    loop {
        match niri::event_stream().await {
            Ok(mut reader) => {
                backoff = std::time::Duration::from_secs(2);
                let mut line = String::new();

                loop {
                    line.clear();
                    match reader.read_line(&mut line).await {
                        Ok(0) => {
                            tracing::warn!("niri workspace event stream closed, reconnecting...");
                            break;
                        }
                        Ok(_) => {
                            backoff = std::time::Duration::from_secs(2);
                            match niri::parse_event(&line) {
                                Ok(niri_ipc::Event::WorkspacesChanged { workspaces }) => {
                                    let infos = workspaces.into_iter().map(workspace_from_niri).collect();
                                    if tx.send(infos).await.is_err() {
                                        return Ok(());
                                    }
                                }
                                Ok(niri_ipc::Event::WorkspaceActivated { id, focused }) => {
                                    tracing::debug!("workspace {id} activated (focused={focused})");
                                }
                                Ok(_) => {}
                                Err(e) => {
                                    tracing::debug!("failed to parse niri event: {e}");
                                }
                            }
                        }
                        Err(e) => {
                            tracing::warn!("niri workspace stream error: {e}, reconnecting...");
                            break;
                        }
                    }
                }
            }
            Err(e) => {
                tracing::warn!("failed to connect to niri event stream: {e}");
            }
        }

        tracing::debug!("reconnecting to niri workspace stream in {backoff:?}");
        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(MAX_BACKOFF);
    }
}

/// Convert a niri workspace to our abstracted `WorkspaceInfo`.
fn workspace_from_niri(ws: niri_ipc::Workspace) -> WorkspaceInfo {
    WorkspaceInfo {
        id: ws.id,
        idx: ws.idx,
        name: ws.name,
        output: ws.output,
        is_active: ws.is_active,
        is_focused: ws.is_focused,
    }
}

/// Filter workspaces to only the focused output when `show_all` is false.
///
/// When `show_all` is true, returns all workspaces unchanged. When false,
/// finds the focused workspace's output and retains only workspaces on
/// that output.
fn filter_workspaces(workspaces: &[WorkspaceInfo], show_all: bool) -> Vec<&WorkspaceInfo> {
    let mut result: Vec<&WorkspaceInfo> = workspaces.iter().collect();

    if !show_all {
        let focused_output = result
            .iter()
            .find(|ws| ws.is_focused)
            .and_then(|ws| ws.output.as_deref());
        if let Some(output) = focused_output {
            let output = output.to_string();
            result.retain(|ws| ws.output.as_deref() == Some(&output));
        }
    }

    result.sort_by(|a, b| {
        a.output
            .cmp(&b.output)
            .then_with(|| a.idx.cmp(&b.idx))
    });

    result
}

/// Clear and rebuild workspace buttons in the container.
fn rebuild_workspace_buttons(
    container: &gtk4::Box,
    workspaces: &[WorkspaceInfo],
    cmd_tx: &async_channel::Sender<WorkspaceCommand>,
    show_all: bool,
) {
    // Remove all existing children
    while let Some(child) = container.first_child() {
        container.remove(&child);
    }

    let sorted = filter_workspaces(workspaces, show_all);

    for ws in sorted {
        let display = ws
            .name
            .as_deref()
            .unwrap_or(&ws.idx.to_string())
            .to_string();

        let btn = gtk4::Button::with_label(&display);
        btn.add_css_class("psh-bar-workspace-btn");

        if ws.is_active {
            btn.add_css_class("active");
        }
        if ws.is_focused {
            btn.add_css_class("focused");
        }

        let tx = cmd_tx.clone();
        let id = ws.id;
        btn.connect_clicked(move |_| {
            let tx = tx.clone();
            glib::spawn_future_local(async move {
                let _ = tx.send(WorkspaceCommand::Focus(id)).await;
            });
        });

        container.append(&btn);
    }
}

/// Set up placeholder workspace buttons when no backend is available.
fn setup_placeholder_workspaces(container: &gtk4::Box) {
    for i in 1..=5u8 {
        let btn = gtk4::Button::with_label(&i.to_string());
        btn.add_css_class("psh-bar-workspace-btn");
        if i == 1 {
            btn.add_css_class("active");
        }
        container.append(&btn);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_from_niri_maps_fields() {
        let niri_ws = niri_ipc::Workspace {
            id: 42,
            idx: 3,
            name: Some("dev".into()),
            output: Some("DP-1".into()),
            is_active: true,
            is_focused: false,
            is_urgent: false,
            active_window_id: None,
        };
        let info = workspace_from_niri(niri_ws);
        assert_eq!(info.id, 42);
        assert_eq!(info.idx, 3);
        assert_eq!(info.name.as_deref(), Some("dev"));
        assert_eq!(info.output.as_deref(), Some("DP-1"));
        assert!(info.is_active);
        assert!(!info.is_focused);
    }

    #[test]
    fn workspace_display_name_uses_name_or_idx() {
        let with_name = WorkspaceInfo {
            id: 1,
            idx: 1,
            name: Some("main".into()),
            output: None,
            is_active: false,
            is_focused: false,
        };
        let display = with_name
            .name
            .as_deref()
            .map(|n| n.to_string())
            .unwrap_or_else(|| with_name.idx.to_string());
        assert_eq!(display, "main");

        let without_name = WorkspaceInfo {
            id: 2,
            idx: 3,
            name: None,
            output: None,
            is_active: false,
            is_focused: false,
        };
        let display = without_name
            .name
            .as_deref()
            .map(|n| n.to_string())
            .unwrap_or_else(|| without_name.idx.to_string());
        assert_eq!(display, "3");
    }

    fn make_ws(id: u64, idx: u8, output: &str, is_focused: bool) -> WorkspaceInfo {
        WorkspaceInfo {
            id,
            idx,
            name: None,
            output: Some(output.into()),
            is_active: false,
            is_focused,
        }
    }

    #[test]
    fn filter_workspaces_show_all_returns_everything() {
        let workspaces = vec![
            make_ws(1, 1, "DP-1", false),
            make_ws(2, 2, "DP-1", true),
            make_ws(3, 1, "HDMI-1", false),
        ];
        let filtered = filter_workspaces(&workspaces, true);
        assert_eq!(filtered.len(), 3);
    }

    #[test]
    fn filter_workspaces_focused_output_only() {
        let workspaces = vec![
            make_ws(1, 1, "DP-1", false),
            make_ws(2, 2, "DP-1", true),
            make_ws(3, 1, "HDMI-1", false),
            make_ws(4, 2, "HDMI-1", false),
        ];
        let filtered = filter_workspaces(&workspaces, false);
        assert_eq!(filtered.len(), 2);
        assert!(filtered.iter().all(|ws| ws.output.as_deref() == Some("DP-1")));
    }

    #[test]
    fn filter_workspaces_no_focused_shows_all() {
        let workspaces = vec![
            make_ws(1, 1, "DP-1", false),
            make_ws(2, 1, "HDMI-1", false),
        ];
        // When no workspace is focused, show all as fallback
        let filtered = filter_workspaces(&workspaces, false);
        assert_eq!(filtered.len(), 2);
    }

    #[test]
    fn filter_workspaces_sorted_by_output_then_idx() {
        let workspaces = vec![
            make_ws(4, 2, "HDMI-1", false),
            make_ws(1, 1, "DP-1", false),
            make_ws(3, 1, "HDMI-1", true),
            make_ws(2, 2, "DP-1", false),
        ];
        let filtered = filter_workspaces(&workspaces, true);
        let ids: Vec<u64> = filtered.iter().map(|ws| ws.id).collect();
        assert_eq!(ids, vec![1, 2, 3, 4]);
    }
}
