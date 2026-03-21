#![allow(dead_code, unused_imports)]

use psh_core::config;
use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState},
    delegate_compositor, delegate_output, delegate_registry, delegate_shm,
    output::{OutputHandler, OutputState},
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    shm::{
        slot::SlotPool,
        Shm, ShmHandler,
    },
};
use wayland_client::{
    globals::registry_queue_init,
    protocol::{wl_output, wl_shm, wl_surface},
    Connection, QueueHandle,
};

fn main() {
    psh_core::logging::init("psh_lock");
    tracing::info!("starting psh-lock");

    let _cfg = config::load().expect("failed to load config");

    let conn = Connection::connect_to_env().expect("failed to connect to wayland");
    let (globals, mut event_queue) = registry_queue_init(&conn).expect("failed to init registry");
    let qh = event_queue.handle();

    let compositor = CompositorState::bind(&globals, &qh).expect("wl_compositor not available");
    let shm = Shm::bind(&globals, &qh).expect("wl_shm not available");

    // TODO: Bind ext-session-lock-v1 protocol
    // For now, create a basic surface for password entry
    let surface = compositor.create_surface(&qh);
    let pool = SlotPool::new(256 * 256 * 4, &shm).expect("failed to create shm pool");

    let mut state = LockState {
        registry: RegistryState::new(&globals),
        output: OutputState::new(&globals, &qh),
        shm,
        pool,
        surface,
        password: String::new(),
        locked: false,
    };

    tracing::info!("lock screen active");

    loop {
        event_queue
            .blocking_dispatch(&mut state)
            .expect("wayland dispatch failed");
    }
}

struct LockState {
    registry: RegistryState,
    output: OutputState,
    shm: Shm,
    pool: SlotPool,
    surface: wl_surface::WlSurface,
    password: String,
    locked: bool,
}

impl LockState {
    fn draw(&mut self, _qh: &QueueHandle<Self>) {
        // TODO: Render password entry UI using tiny-skia
        // Will draw centered password dots and clock
    }

    fn try_authenticate(&self) -> bool {
        // PAM authentication — runs on a dedicated thread to avoid blocking Wayland
        let password = self.password.clone();
        let (tx, rx) = std::sync::mpsc::channel();

        std::thread::spawn(move || {
            let result = authenticate_pam(&password);
            let _ = tx.send(result);
        });

        rx.recv().unwrap_or(false)
    }
}

fn authenticate_pam(password: &str) -> bool {
    use pam_client::{Context, Flag};

    let mut context = match Context::new("psh-lock", None, pam_client::conv_null::Conversation::new()) {
        Ok(ctx) => ctx,
        Err(e) => {
            tracing::error!("PAM context creation failed: {e}");
            return false;
        }
    };

    // In a real implementation, we'd use a custom conversation function
    // that provides the password. For now, this is a placeholder.
    let _ = password;

    match context.authenticate(Flag::NONE) {
        Ok(()) => {
            tracing::info!("PAM authentication succeeded");
            true
        }
        Err(e) => {
            tracing::warn!("PAM authentication failed: {e}");
            false
        }
    }
}

impl CompositorHandler for LockState {
    fn scale_factor_changed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _new_factor: i32,
    ) {
    }

    fn transform_changed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _new_transform: wl_output::Transform,
    ) {
    }

    fn frame(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _time: u32,
    ) {
    }

    fn surface_enter(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _output: &wl_output::WlOutput,
    ) {
    }

    fn surface_leave(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _output: &wl_output::WlOutput,
    ) {
    }
}

impl OutputHandler for LockState {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output
    }

    fn new_output(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _output: wl_output::WlOutput,
    ) {
    }

    fn update_output(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _output: wl_output::WlOutput,
    ) {
    }

    fn output_destroyed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _output: wl_output::WlOutput,
    ) {
    }
}

impl ShmHandler for LockState {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

impl ProvidesRegistryState for LockState {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry
    }
    registry_handlers![OutputState];
}

delegate_compositor!(LockState);
delegate_output!(LockState);
delegate_shm!(LockState);
delegate_registry!(LockState);
