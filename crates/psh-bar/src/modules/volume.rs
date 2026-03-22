//! Volume module — displays and controls audio volume via `wpctl`.
//!
//! Polls `wpctl get-volume @DEFAULT_AUDIO_SINK@` every 2 seconds on a background
//! tokio thread. Supports scroll-to-adjust and click-to-mute via a command channel.

use gtk4::glib;
use gtk4::prelude::*;

use super::{BarModule, ModuleContext};

/// Displays the current volume level with scroll-to-adjust and click-to-mute.
pub struct VolumeModule;

/// Parsed volume state from `wpctl` output.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct VolumeState {
    /// Volume level as a fraction (0.0 to 1.0+).
    pub level: f32,
    /// Whether the sink is muted.
    pub muted: bool,
}

/// Commands sent from the GTK thread to the volume backend.
#[derive(Debug)]
enum VolumeCommand {
    /// Adjust volume (e.g., "5%+", "5%-").
    SetVolume(String),
    /// Toggle mute on the default sink.
    ToggleMute,
}

impl BarModule for VolumeModule {
    fn name(&self) -> &'static str {
        "volume"
    }

    fn widget(&self, ctx: &ModuleContext) -> gtk4::Widget {
        let label = gtk4::Label::new(Some("VOL --"));
        label.add_css_class("psh-bar-volume");

        let volume_step = ctx.config.volume_step.unwrap_or(5);

        let (state_tx, state_rx) = async_channel::bounded::<VolumeState>(4);
        let (cmd_tx, cmd_rx) = async_channel::bounded::<VolumeCommand>(8);

        // Background task: poll volume and handle commands on the shared runtime
        ctx.rt.spawn(async move {
            run_volume_backend(state_tx, cmd_rx).await;
        });

        // GTK side: update label when volume state changes
        let label_clone = label.clone();
        glib::spawn_future_local(async move {
            while let Ok(state) = state_rx.recv().await {
                apply_volume_label(&label_clone, &state);
            }
        });

        // Scroll to adjust volume
        let scroll = gtk4::EventControllerScroll::new(gtk4::EventControllerScrollFlags::VERTICAL);
        let cmd_tx_scroll = cmd_tx.clone();
        let step = volume_step;
        scroll.connect_scroll(move |_, _, dy| {
            let delta = if dy < 0.0 {
                format!("{step}%+")
            } else {
                format!("{step}%-")
            };
            if cmd_tx_scroll
                .try_send(VolumeCommand::SetVolume(delta))
                .is_err()
            {
                tracing::debug!("volume command channel full (scroll)");
            }
            glib::Propagation::Stop
        });
        label.add_controller(scroll);

        // Click to toggle mute
        let click = gtk4::GestureClick::new();
        click.connect_released(move |_, _, _, _| {
            if cmd_tx.try_send(VolumeCommand::ToggleMute).is_err() {
                tracing::debug!("volume command channel full (mute)");
            }
        });
        label.add_controller(click);

        label.upcast()
    }
}

/// Update the label text and CSS class based on volume state.
fn apply_volume_label(label: &gtk4::Label, state: &VolumeState) {
    if state.muted {
        label.set_text("MUTE");
        label.add_css_class("muted");
    } else {
        let pct = (state.level * 100.0).round() as u32;
        label.set_text(&format!("VOL {pct}%"));
        label.remove_css_class("muted");
    }
}

/// Run the volume backend: poll periodically and handle commands.
///
/// Runs on a background tokio thread. Sends [`VolumeState`] updates to the
/// GTK thread via `state_tx`. Receives scroll/click commands via `cmd_rx`.
async fn run_volume_backend(
    state_tx: async_channel::Sender<VolumeState>,
    cmd_rx: async_channel::Receiver<VolumeCommand>,
) {
    // Initial poll
    if let Some(state) = poll_volume().await {
        match state_tx.try_send(state) {
            Err(async_channel::TrySendError::Full(_)) => {
                tracing::debug!("volume state channel full, skipping stale update");
            }
            Err(async_channel::TrySendError::Closed(_)) => return,
            Ok(()) => {}
        }
    }

    loop {
        tokio::select! {
            cmd = cmd_rx.recv() => {
                match cmd {
                    Ok(VolumeCommand::SetVolume(delta)) => {
                        run_wpctl(&["set-volume", "@DEFAULT_AUDIO_SINK@", &delta]).await;
                    }
                    Ok(VolumeCommand::ToggleMute) => {
                        run_wpctl(&["set-mute", "@DEFAULT_AUDIO_SINK@", "toggle"]).await;
                    }
                    Err(_) => break,
                }
                // Re-poll immediately after a command for responsive feedback
                if let Some(state) = poll_volume().await {
                    match state_tx.try_send(state) {
                        Err(async_channel::TrySendError::Full(_)) => {
                            tracing::debug!("volume state channel full, skipping stale update");
                        }
                        Err(async_channel::TrySendError::Closed(_)) => break,
                        Ok(()) => {}
                    }
                }
            }
            _ = tokio::time::sleep(std::time::Duration::from_secs(2)) => {
                if let Some(state) = poll_volume().await {
                    match state_tx.try_send(state) {
                        Err(async_channel::TrySendError::Full(_)) => {
                            tracing::debug!("volume state channel full, skipping stale update");
                        }
                        Err(async_channel::TrySendError::Closed(_)) => break,
                        Ok(()) => {}
                    }
                }
            }
        }
    }
}

/// Poll the current volume state by running `wpctl get-volume`.
async fn poll_volume() -> Option<VolumeState> {
    let output = tokio::process::Command::new("wpctl")
        .args(["get-volume", "@DEFAULT_AUDIO_SINK@"])
        .output()
        .await
        .ok()?;

    if output.status.success() {
        let stdout = String::from_utf8(output.stdout).ok()?;
        parse_wpctl_volume(&stdout)
    } else {
        tracing::debug!("wpctl get-volume exited with {}", output.status);
        None
    }
}

/// Parse the output of `wpctl get-volume`.
///
/// Expected formats:
/// - `"Volume: 0.50"`
/// - `"Volume: 0.50 [MUTED]"`
pub(crate) fn parse_wpctl_volume(output: &str) -> Option<VolumeState> {
    let output = output.trim();
    let rest = output.strip_prefix("Volume:")?;
    let rest = rest.trim();

    let muted = rest.contains("[MUTED]");
    let level_str = if muted {
        rest.split_whitespace().next()?
    } else {
        rest
    };

    let level = level_str.parse::<f32>().ok()?;
    Some(VolumeState { level, muted })
}

/// Run a wpctl command (fire-and-forget).
async fn run_wpctl(args: &[&str]) {
    if let Err(e) = tokio::process::Command::new("wpctl")
        .args(args)
        .status()
        .await
    {
        tracing::warn!("wpctl failed: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_normal_volume() {
        let state = parse_wpctl_volume("Volume: 0.50").unwrap();
        assert!((state.level - 0.50).abs() < f32::EPSILON);
        assert!(!state.muted);
    }

    #[test]
    fn parse_muted_volume() {
        let state = parse_wpctl_volume("Volume: 0.50 [MUTED]").unwrap();
        assert!((state.level - 0.50).abs() < f32::EPSILON);
        assert!(state.muted);
    }

    #[test]
    fn parse_zero_volume() {
        let state = parse_wpctl_volume("Volume: 0.00").unwrap();
        assert!((state.level - 0.0).abs() < f32::EPSILON);
        assert!(!state.muted);
    }

    #[test]
    fn parse_full_volume() {
        let state = parse_wpctl_volume("Volume: 1.00").unwrap();
        assert!((state.level - 1.0).abs() < f32::EPSILON);
        assert!(!state.muted);
    }

    #[test]
    fn parse_over_100_volume() {
        let state = parse_wpctl_volume("Volume: 1.50").unwrap();
        assert!((state.level - 1.50).abs() < f32::EPSILON);
    }

    #[test]
    fn parse_with_trailing_newline() {
        let state = parse_wpctl_volume("Volume: 0.75\n").unwrap();
        assert!((state.level - 0.75).abs() < f32::EPSILON);
    }

    #[test]
    fn parse_invalid_output() {
        assert!(parse_wpctl_volume("").is_none());
        assert!(parse_wpctl_volume("not volume output").is_none());
        assert!(parse_wpctl_volume("Volume:").is_none());
        assert!(parse_wpctl_volume("Volume: abc").is_none());
    }
}
