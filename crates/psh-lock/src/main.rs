//! psh-lock — screen locker for the psh desktop environment.
//!
//! A security-critical Wayland client using ext-session-lock-v1 to prevent
//! input from reaching underlying surfaces while locked. Renders a password
//! entry UI with tiny-skia and authenticates via PAM.

mod pam;
mod render;
mod state;

use smithay_client_toolkit::{
    compositor::CompositorState,
    output::OutputState,
    reexports::{calloop::EventLoop, calloop_wayland_source::WaylandSource},
    registry::RegistryState,
    seat::{SeatState, keyboard::Modifiers},
    session_lock::SessionLockState,
    shm::Shm,
};
use wayland_client::{Connection, globals::registry_queue_init};

use psh_core::config;

use crate::pam::PamResult;
use crate::render::RenderState;
use crate::state::{AuthState, LockState, get_username};

fn main() {
    psh_core::logging::init("psh_lock");
    tracing::info!("starting psh-lock");

    // Ignore SIGTERM/SIGINT while locked — the screen locker should only
    // be unlocked by successful authentication. SIGKILL cannot be caught.
    unsafe {
        libc::signal(libc::SIGTERM, libc::SIG_IGN);
        libc::signal(libc::SIGINT, libc::SIG_IGN);
        libc::signal(libc::SIGHUP, libc::SIG_IGN);
    }

    let cfg = config::load().expect("failed to load config");
    let lock_cfg = cfg.lock;

    let username = get_username();
    tracing::info!("locking session for user: {username}");

    // Connect to Wayland.
    let conn = Connection::connect_to_env().expect("failed to connect to wayland");
    let (globals, event_queue) = registry_queue_init(&conn).expect("failed to init registry");
    let qh = event_queue.handle();

    // Bind globals.
    let compositor = CompositorState::bind(&globals, &qh).expect("wl_compositor not available");
    let shm = Shm::bind(&globals, &qh).expect("wl_shm not available");
    let session_lock_state = SessionLockState::new(&globals, &qh);

    // Set up calloop event loop.
    let mut event_loop: EventLoop<LockState> =
        EventLoop::try_new().expect("failed to create event loop");
    let loop_handle = event_loop.handle();

    // PAM result channel.
    let (pam_sender, pam_channel) =
        smithay_client_toolkit::reexports::calloop::channel::channel::<PamResult>();

    // Insert PAM channel into event loop.
    loop_handle
        .insert_source(pam_channel, |event, _, state| {
            if let smithay_client_toolkit::reexports::calloop::channel::Event::Msg(result) = event {
                state.handle_pam_result(result);
            }
        })
        .expect("failed to insert PAM channel");

    // Insert Wayland source.
    WaylandSource::new(conn.clone(), event_queue)
        .insert(loop_handle.clone())
        .expect("failed to insert wayland source");

    // Build state.
    let mut state = LockState {
        registry: RegistryState::new(&globals),
        output: OutputState::new(&globals, &qh),
        compositor,
        seat: SeatState::new(&globals, &qh),
        shm,
        session_lock_state,
        session_lock: None,
        lock_surfaces: Vec::new(),
        keyboard: None,
        modifiers: Modifiers::default(),
        password: String::new(),
        auth_state: AuthState::Idle,
        config: lock_cfg,
        username,
        render_state: RenderState::new(),
        pool: None,
        pool_size: 0,
        conn: conn.clone(),
        loop_handle,
        pam_sender,
        qh: qh.clone(),
        running: true,
    };

    // Request the session lock.
    state.session_lock = Some(
        state
            .session_lock_state
            .lock(&qh)
            .expect("ext-session-lock-v1 not supported by compositor"),
    );

    tracing::info!("lock requested, entering event loop");

    // Main event loop.
    while state.running {
        event_loop
            .dispatch(std::time::Duration::from_millis(16), &mut state)
            .expect("event loop dispatch failed");
    }

    // Clean shutdown.
    state.lock_surfaces.clear();
    tracing::info!("psh-lock exiting");
}
