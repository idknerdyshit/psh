//! Window title module — displays the focused window's title.
//!
//! Uses niri IPC when `$NIRI_SOCKET` is set, otherwise falls back to
//! `ext-foreign-toplevel-list-v1`. Shows an empty label if neither is available.

use gtk4::glib;
use gtk4::prelude::*;

use super::{BarModule, ModuleContext};
use crate::niri;

/// Displays the title of the currently focused window.
///
/// The title is truncated to `max_title_length` characters (default 50)
/// with an ellipsis when exceeded.
pub struct WindowTitleModule;

impl BarModule for WindowTitleModule {
    fn name(&self) -> &'static str {
        "window_title"
    }

    fn widget(&self, ctx: &ModuleContext) -> gtk4::Widget {
        let max_len = ctx.config.max_title_length.unwrap_or(50);

        let label = gtk4::Label::new(None);
        label.add_css_class("psh-bar-window-title");
        label.set_ellipsize(gtk4::pango::EllipsizeMode::End);
        label.set_max_width_chars(max_len as i32);

        if niri::is_available() {
            setup_niri_window_title(&label, max_len, &ctx.rt);
        }
        // TODO(phase6): ext-foreign-toplevel-v1 fallback

        label.upcast()
    }
}

/// Set up window title tracking via niri IPC event stream.
fn setup_niri_window_title(label: &gtk4::Label, max_len: usize, rt: &tokio::runtime::Handle) {
    let (tx, rx) = async_channel::bounded::<Option<String>>(4);

    // Background task: connect to niri event stream on the shared runtime
    rt.spawn(async move {
        if let Err(e) = run_niri_title_backend(tx).await {
            tracing::error!("niri window title backend error: {e}");
        }
    });

    // GTK side: update label on title changes
    let label = label.clone();
    glib::spawn_future_local(async move {
        while let Ok(title) = rx.recv().await {
            let text = match title {
                Some(t) => truncate_title(&t, max_len),
                None => String::new(),
            };
            label.set_text(&text);
        }
    });
}

/// Run the niri window title backend: track focused window and relay title updates.
///
/// Automatically reconnects to the niri event stream with exponential backoff
/// if the connection drops (e.g., niri restarts).
async fn run_niri_title_backend(
    tx: async_channel::Sender<Option<String>>,
) -> psh_core::Result<()> {
    use std::collections::HashMap;
    use tokio::io::AsyncBufReadExt;

    const MAX_BACKOFF: std::time::Duration = std::time::Duration::from_secs(30);
    let mut backoff = std::time::Duration::from_secs(2);

    loop {
        match niri::event_stream().await {
            Ok(mut reader) => {
                backoff = std::time::Duration::from_secs(2);

                // Fresh state on each connection
                let mut windows: HashMap<u64, String> = HashMap::new();
                let mut focused_id: Option<u64> = None;
                let mut line = String::new();

                // Clear displayed title on reconnect
                if tx.send(None).await.is_err() {
                    return Ok(());
                }

                loop {
                    line.clear();
                    match reader.read_line(&mut line).await {
                        Ok(0) => {
                            tracing::warn!("niri title event stream closed, reconnecting...");
                            break;
                        }
                        Ok(_) => {
                            backoff = std::time::Duration::from_secs(2);
                            match niri::parse_event(&line) {
                                Ok(niri_ipc::Event::WindowsChanged { windows: new_windows }) => {
                                    windows.clear();
                                    for w in &new_windows {
                                        if let Some(ref title) = w.title {
                                            windows.insert(w.id, title.clone());
                                        }
                                    }
                                    focused_id = new_windows.iter().find(|w| w.is_focused).map(|w| w.id);
                                    let title = focused_id.and_then(|id| windows.get(&id).cloned());
                                    if tx.send(title).await.is_err() {
                                        return Ok(());
                                    }
                                }
                                Ok(niri_ipc::Event::WindowOpenedOrChanged { window }) => {
                                    if let Some(ref title) = window.title {
                                        windows.insert(window.id, title.clone());
                                    }
                                    if window.is_focused {
                                        focused_id = Some(window.id);
                                        if tx.send(window.title.clone()).await.is_err() {
                                            return Ok(());
                                        }
                                    } else if focused_id == Some(window.id)
                                        && tx.send(window.title.clone()).await.is_err()
                                    {
                                        return Ok(());
                                    }
                                }
                                Ok(niri_ipc::Event::WindowClosed { id }) => {
                                    windows.remove(&id);
                                    if focused_id == Some(id) {
                                        focused_id = None;
                                        if tx.send(None).await.is_err() {
                                            return Ok(());
                                        }
                                    }
                                }
                                Ok(niri_ipc::Event::WindowFocusChanged { id }) => {
                                    focused_id = id;
                                    let title = id.and_then(|i| windows.get(&i).cloned());
                                    if tx.send(title).await.is_err() {
                                        return Ok(());
                                    }
                                }
                                Ok(_) => {}
                                Err(e) => {
                                    tracing::debug!("failed to parse niri event: {e}");
                                }
                            }
                        }
                        Err(e) => {
                            tracing::warn!("niri title stream error: {e}, reconnecting...");
                            break;
                        }
                    }
                }
            }
            Err(e) => {
                tracing::warn!("failed to connect to niri title stream: {e}");
            }
        }

        tracing::debug!("reconnecting to niri title stream in {backoff:?}");
        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(MAX_BACKOFF);
    }
}

/// Truncate a title to the given maximum length, appending an ellipsis if truncated.
pub(crate) fn truncate_title(title: &str, max_len: usize) -> String {
    let chars: Vec<char> = title.chars().collect();
    if chars.len() <= max_len {
        title.to_string()
    } else {
        let truncated: String = chars[..max_len.saturating_sub(1)].iter().collect();
        format!("{truncated}\u{2026}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_short_title() {
        assert_eq!(truncate_title("hello", 50), "hello");
    }

    #[test]
    fn truncate_exact_length() {
        let title = "a".repeat(50);
        assert_eq!(truncate_title(&title, 50), title);
    }

    #[test]
    fn truncate_long_title() {
        let title = "a".repeat(60);
        let result = truncate_title(&title, 50);
        // Should be 49 'a' chars + 1 ellipsis char
        assert_eq!(result.chars().count(), 50);
        assert!(result.ends_with('\u{2026}'));
    }

    #[test]
    fn truncate_empty_title() {
        assert_eq!(truncate_title("", 50), "");
    }

    #[test]
    fn truncate_unicode_title() {
        // Ensure we truncate by chars, not bytes
        let title = "\u{1f600}".repeat(60); // 60 emoji
        let result = truncate_title(&title, 10);
        assert_eq!(result.chars().count(), 10);
        assert!(result.ends_with('\u{2026}'));
    }
}
