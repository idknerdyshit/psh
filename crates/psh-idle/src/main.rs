//! psh-idle — idle monitor and sleep hook for psh-lock.
//!
//! Monitors user idle status via ext-idle-notify-v1 and system sleep via
//! logind's PrepareForSleep D-Bus signal. Spawns `psh-lock` when either
//! condition triggers.

use std::process::{Child, Command};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use psh_core::config::{self, IdleConfig};
use smithay_client_toolkit::{
    delegate_registry, delegate_seat,
    reexports::{
        calloop::{
            self,
            channel::Sender,
            EventLoop,
        },
        calloop_wayland_source::WaylandSource,
    },
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    seat::{Capability, SeatHandler, SeatState},
};
use wayland_client::{
    globals::{registry_queue_init, GlobalList},
    protocol::wl_seat,
    Connection, Dispatch, QueueHandle,
};
use wayland_protocols::ext::idle_notify::v1::client::{
    ext_idle_notification_v1::{self, ExtIdleNotificationV1},
    ext_idle_notifier_v1::ExtIdleNotifierV1,
};

/// Internal command sent from background threads to the main event loop.
enum IdleCommand {
    /// Lock the screen (from idle timeout or sleep hook).
    Lock,
}

/// Main application state.
struct IdleState {
    registry: RegistryState,
    seat_state: SeatState,
    seat: Option<wl_seat::WlSeat>,
    config: IdleConfig,
    lock_child: Option<Child>,
    /// Set once idle notification is created (needs a seat).
    idle_notification: Option<ExtIdleNotificationV1>,
    idle_notifier: Option<ExtIdleNotifierV1>,
    globals: GlobalList,
}

impl IdleState {
    /// Spawn psh-lock if not already running.
    fn lock(&mut self) {
        // Check if an existing lock process is still running.
        if let Some(ref mut child) = self.lock_child {
            match child.try_wait() {
                Ok(Some(_)) => {
                    // Process exited, clear it.
                    self.lock_child = None;
                }
                Ok(None) => {
                    // Still running, don't spawn another.
                    tracing::debug!("psh-lock already running, skipping");
                    return;
                }
                Err(e) => {
                    tracing::warn!("failed to check psh-lock status: {e}");
                    self.lock_child = None;
                }
            }
        }

        tracing::info!("spawning lock command: {}", self.config.lock_command);

        // Parse command with proper shell quoting support.
        let parts = match shlex::split(&self.config.lock_command) {
            Some(p) if !p.is_empty() => p,
            _ => {
                tracing::error!("invalid or empty lock command: {}", self.config.lock_command);
                return;
            }
        };

        match Command::new(&parts[0]).args(&parts[1..]).spawn() {
            Ok(child) => {
                tracing::info!("psh-lock spawned (pid {})", child.id());
                self.lock_child = Some(child);
            }
            Err(e) => {
                tracing::error!("failed to spawn {}: {e}", self.config.lock_command);
            }
        }
    }

    /// Set up idle notification once we have a seat.
    fn setup_idle_notification(&mut self, qh: &QueueHandle<Self>) {
        if self.config.idle_timeout_secs == 0 {
            tracing::info!("idle timeout disabled (0), skipping idle notification");
            return;
        }

        let Some(ref seat) = self.seat else {
            tracing::debug!("no seat available yet, deferring idle setup");
            return;
        };

        if self.idle_notification.is_some() {
            return; // Already set up.
        }

        // Bind the idle notifier global.
        let notifier: ExtIdleNotifierV1 = match self.globals.bind(qh, 1..=1, ()) {
            Ok(n) => n,
            Err(e) => {
                tracing::warn!(
                    "ext-idle-notify-v1 not supported by compositor: {e}; \
                     idle timeout will not work (sleep lock may still work)"
                );
                return;
            }
        };

        let timeout_ms = self.config.idle_timeout_secs.saturating_mul(1000).min(u32::MAX as u64);
        tracing::info!("setting up idle notification with {timeout_ms}ms timeout");
        let notification = notifier.get_idle_notification(timeout_ms as u32, seat, qh, ());
        self.idle_notification = Some(notification);
        self.idle_notifier = Some(notifier);
    }
}

// ---------------------------------------------------------------------------
// Manual Dispatch for ext-idle-notify-v1 (no SCTK wrapper available)
// ---------------------------------------------------------------------------

impl Dispatch<ExtIdleNotifierV1, ()> for IdleState {
    fn event(
        _state: &mut Self,
        _proxy: &ExtIdleNotifierV1,
        _event: <ExtIdleNotifierV1 as wayland_client::Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        // The notifier itself doesn't emit events.
    }
}

impl Dispatch<ExtIdleNotificationV1, ()> for IdleState {
    fn event(
        state: &mut Self,
        _proxy: &ExtIdleNotificationV1,
        event: ext_idle_notification_v1::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        match event {
            ext_idle_notification_v1::Event::Idled => {
                tracing::info!("user idle detected, locking");
                state.lock();
            }
            ext_idle_notification_v1::Event::Resumed => {
                tracing::debug!("user activity resumed");
            }
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------
// Seat handler (needed to get a wl_seat for idle notification)
// ---------------------------------------------------------------------------

impl SeatHandler for IdleState {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.seat_state
    }

    fn new_seat(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, seat: wl_seat::WlSeat) {
        if self.seat.is_none() {
            self.seat = Some(seat);
        }
    }

    fn new_capability(
        &mut self,
        _conn: &Connection,
        qh: &QueueHandle<Self>,
        _seat: wl_seat::WlSeat,
        _capability: Capability,
    ) {
        // Once we have any capability, try to set up idle notification.
        self.setup_idle_notification(qh);
    }

    fn remove_capability(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _seat: wl_seat::WlSeat,
        _capability: Capability,
    ) {
    }

    fn remove_seat(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _seat: wl_seat::WlSeat,
    ) {
    }
}

// ---------------------------------------------------------------------------
// Registry handler
// ---------------------------------------------------------------------------

impl ProvidesRegistryState for IdleState {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry
    }
    registry_handlers![SeatState];
}

delegate_registry!(IdleState);
delegate_seat!(IdleState);

// ---------------------------------------------------------------------------
// logind PrepareForSleep D-Bus monitor
// ---------------------------------------------------------------------------

/// Spawn a background tokio thread that monitors logind's PrepareForSleep signal.
///
/// The `shutdown_notify` is signaled from the main thread to stop the monitor.
fn spawn_sleep_monitor(sender: Sender<IdleCommand>, shutdown_notify: Arc<tokio::sync::Notify>) {
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("failed to create tokio runtime");

        rt.block_on(async move {
            if let Err(e) = monitor_sleep(sender, shutdown_notify).await {
                tracing::error!("sleep monitor failed: {e}");
            }
        });
    });
}

/// zbus proxy for logind Manager interface.
#[zbus::proxy(
    interface = "org.freedesktop.login1.Manager",
    default_service = "org.freedesktop.login1",
    default_path = "/org/freedesktop/login1"
)]
trait LogindManager {
    /// PrepareForSleep signal — `true` before sleep, `false` after wake.
    #[zbus(signal)]
    fn prepare_for_sleep(&self, going_to_sleep: bool) -> zbus::Result<()>;
}

/// Monitor logind PrepareForSleep signal.
///
/// Exits when `shutdown_notify` is signaled or the channel is closed.
async fn monitor_sleep(
    sender: Sender<IdleCommand>,
    shutdown_notify: Arc<tokio::sync::Notify>,
) -> zbus::Result<()> {
    use futures_util::StreamExt;

    let conn = zbus::Connection::system().await?;
    let proxy = LogindManagerProxy::new(&conn).await?;

    let mut stream = proxy.receive_prepare_for_sleep().await?;
    tracing::info!("listening for logind PrepareForSleep signals");

    loop {
        let signal = tokio::select! {
            s = stream.next() => {
                match s {
                    Some(s) => s,
                    None => break,
                }
            }
            _ = shutdown_notify.notified() => {
                tracing::debug!("sleep monitor shutting down");
                break;
            }
        };

        let args = signal.args()?;
        if args.going_to_sleep {
            tracing::info!("system preparing for sleep, requesting lock");
            if let Err(e) = sender.send(IdleCommand::Lock) {
                tracing::error!("failed to send lock command to main loop: {e}");
                break;
            }
        } else {
            tracing::debug!("system resumed from sleep");
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

fn main() {
    psh_core::logging::init("psh_idle");
    tracing::info!("starting psh-idle");

    let cfg = config::load().expect("failed to load config");
    let idle_cfg = cfg.idle;
    tracing::info!(
        "idle_timeout={}s, lock_on_sleep={}, lock_command={}",
        idle_cfg.idle_timeout_secs,
        idle_cfg.lock_on_sleep,
        idle_cfg.lock_command
    );

    // Signal handling.
    let shutdown = Arc::new(AtomicBool::new(false));
    signal_hook::flag::register(signal_hook::consts::SIGTERM, Arc::clone(&shutdown))
        .expect("failed to register SIGTERM");
    signal_hook::flag::register(signal_hook::consts::SIGINT, Arc::clone(&shutdown))
        .expect("failed to register SIGINT");

    // Connect to Wayland.
    let conn = Connection::connect_to_env().expect("failed to connect to wayland");
    let (globals, event_queue) =
        registry_queue_init(&conn).expect("failed to init registry");
    let qh = event_queue.handle();

    // Set up calloop event loop.
    let mut event_loop: EventLoop<IdleState> =
        EventLoop::try_new().expect("failed to create event loop");
    let loop_handle = event_loop.handle();

    // Insert Wayland source.
    WaylandSource::new(conn.clone(), event_queue)
        .insert(loop_handle.clone())
        .expect("failed to insert wayland source");

    // Insert command channel for sleep monitor.
    let (cmd_sender, cmd_channel) = calloop::channel::channel::<IdleCommand>();
    loop_handle
        .insert_source(cmd_channel, |event, _, state| {
            if let calloop::channel::Event::Msg(cmd) = event {
                match cmd {
                    IdleCommand::Lock => state.lock(),
                }
            }
        })
        .expect("failed to insert command channel");

    // Spawn logind sleep monitor if enabled.
    let shutdown_notify = Arc::new(tokio::sync::Notify::new());
    if idle_cfg.lock_on_sleep {
        spawn_sleep_monitor(cmd_sender, Arc::clone(&shutdown_notify));
    }

    let mut state = IdleState {
        registry: RegistryState::new(&globals),
        seat_state: SeatState::new(&globals, &qh),
        seat: None,
        config: idle_cfg,
        lock_child: None,
        idle_notification: None,
        idle_notifier: None,
        globals,
    };

    tracing::info!("entering event loop");

    loop {
        if shutdown.load(Ordering::Relaxed) {
            tracing::info!("received signal, shutting down");
            shutdown_notify.notify_waiters();
            break;
        }
        event_loop
            .dispatch(std::time::Duration::from_millis(250), &mut state)
            .expect("event loop dispatch failed");
    }

    // Reap any running lock child to avoid zombie processes.
    if let Some(mut child) = state.lock_child.take() {
        tracing::debug!("waiting for lock child to exit");
        let _ = child.kill();
        let _ = child.wait();
    }

    // Clean up idle notification.
    if let Some(notification) = state.idle_notification.take() {
        notification.destroy();
    }
    if let Some(notifier) = state.idle_notifier.take() {
        notifier.destroy();
    }

    tracing::info!("psh-idle exiting");
}
