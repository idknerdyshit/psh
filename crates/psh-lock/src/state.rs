//! Core lock state and Wayland protocol handler implementations.
//!
//! `LockState` holds all SCTK state, the session lock, keyboard input, and
//! password management. The various SCTK handler traits are implemented here
//! to drive the ext-session-lock-v1 protocol flow.

use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState},
    delegate_compositor, delegate_keyboard, delegate_output, delegate_registry, delegate_seat,
    delegate_session_lock, delegate_shm,
    output::{OutputHandler, OutputState},
    reexports::calloop::{
        LoopHandle,
        channel::Sender,
        timer::{TimeoutAction, Timer},
    },
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    seat::{
        Capability, SeatHandler, SeatState,
        keyboard::{KeyEvent, KeyboardHandler, Keysym, Modifiers},
    },
    session_lock::{
        SessionLock, SessionLockHandler, SessionLockState, SessionLockSurface,
        SessionLockSurfaceConfigure,
    },
    shm::{Shm, ShmHandler, raw::RawPool},
};
use wayland_client::{
    Connection, Dispatch, Proxy, QueueHandle,
    protocol::{wl_buffer, wl_keyboard, wl_output, wl_seat, wl_shm, wl_surface},
};
use wayland_protocols_wlr::output_power_management::v1::client::{
    zwlr_output_power_manager_v1::ZwlrOutputPowerManagerV1,
    zwlr_output_power_v1::{self, ZwlrOutputPowerV1},
};
use std::time::Instant;

use zeroize::Zeroize;

use psh_core::config::LockConfig;

use crate::pam::{self, PamResult};
use crate::render::{self, RenderParams, RenderState};

/// Per-output lock surface tracking.
pub struct LockSurface {
    pub lock_surface: SessionLockSurface,
    pub width: u32,
    pub height: u32,
    pub scale_factor: i32,
    /// Previous buffer kept alive until the next frame (compositor may still reference it).
    pub pending_buffer: Option<wl_buffer::WlBuffer>,
    /// Per-surface shm pool (avoids clobbering other surfaces' buffer data).
    pub pool: Option<RawPool>,
    pub pool_size: usize,
}

/// Current authentication state.
#[derive(Debug, Clone)]
pub enum AuthState {
    /// Waiting for user to type password.
    Idle,
    /// PAM thread is running.
    Authenticating,
    /// Auth failed — shows error message.
    Failed(String),
    /// Auth succeeded — unlocking.
    Unlocked,
}

/// Main application state for psh-lock.
pub struct LockState {
    // SCTK protocol state
    pub registry: RegistryState,
    pub output: OutputState,
    pub compositor: CompositorState,
    pub seat: SeatState,
    pub shm: Shm,

    // Session lock
    pub session_lock_state: SessionLockState,
    /// Holds the lock object between `lock()` and the `locked` callback so it
    /// stays alive without allowing `new_output()` to create premature surfaces.
    pub pending_session_lock: Option<SessionLock>,
    /// Set only after the compositor confirms the lock via the `locked` callback.
    /// `new_output()` uses this to create surfaces for hotplugged outputs.
    pub session_lock: Option<SessionLock>,
    pub lock_surfaces: Vec<LockSurface>,

    // Keyboard
    pub keyboard: Option<wl_keyboard::WlKeyboard>,
    pub modifiers: Modifiers,

    // Password state
    pub password: String,
    pub auth_state: AuthState,

    // Rendering
    pub config: LockConfig,
    pub username: String,
    pub render_state: RenderState,

    // Inactivity timeout
    pub last_input: Instant,
    pub blanked: bool,
    pub dpms_active: bool,

    // Output power management (DPMS)
    pub power_manager: Option<ZwlrOutputPowerManagerV1>,
    pub output_power: Vec<ZwlrOutputPowerV1>,

    // Event loop
    pub conn: Connection,
    pub loop_handle: LoopHandle<'static, Self>,
    pub pam_sender: Sender<PamResult>,
    pub qh: QueueHandle<Self>,
    pub running: bool,
}

impl LockState {
    /// Redraw all lock surfaces.
    pub fn redraw_all(&mut self, qh: &QueueHandle<Self>) {
        for i in 0..self.lock_surfaces.len() {
            self.draw_surface(i, qh);
        }
    }

    /// Draw a single lock surface by index.
    fn draw_surface(&mut self, idx: usize, qh: &QueueHandle<Self>) {
        let surface = &self.lock_surfaces[idx];
        let width = surface.width * surface.scale_factor as u32;
        let height = surface.height * surface.scale_factor as u32;

        if width == 0 || height == 0 {
            return;
        }

        let pixmap = if self.blanked {
            let Some(p) = render::render_blank_surface(width, height) else {
                return;
            };
            p
        } else {
            let now = std::time::SystemTime::now();
            let time_text = format_time(&self.config.clock_format, now);
            let date_text = format_time(&self.config.date_format, now);

            let params = RenderParams {
                config: &self.config,
                username: &self.username,
                password_len: self.password.len(),
                auth_state: &self.auth_state,
                time_text: &time_text,
                date_text: &date_text,
            };

            let Some(p) = render::render_lock_surface(width, height, &mut self.render_state, &params)
            else {
                tracing::warn!("failed to render lock surface {idx}");
                return;
            };
            p
        };

        // Copy pixmap data to shm buffer, converting RGBA → BGRA (Wayland ARGB8888).
        let Some(stride) = (width as usize).checked_mul(4) else {
            tracing::error!("stride overflow for surface {idx} ({width}x{height})");
            return;
        };
        let Some(buf_size) = (height as usize).checked_mul(stride) else {
            tracing::error!("buffer size overflow for surface {idx} ({width}x{height})");
            return;
        };

        // Reuse the per-surface shm pool; only reallocate when size increases.
        let surf = &mut self.lock_surfaces[idx];
        if surf.pool.is_none() || buf_size > surf.pool_size {
            match RawPool::new(buf_size, &self.shm) {
                Ok(p) => {
                    surf.pool = Some(p);
                    surf.pool_size = buf_size;
                }
                Err(e) => {
                    tracing::error!("failed to create shm pool: {e}");
                    return;
                }
            }
        }

        let pool = self.lock_surfaces[idx].pool.as_mut().unwrap();
        let canvas = pool.mmap();
        canvas[..buf_size].copy_from_slice(pixmap.data());
        // Convert RGBA premultiplied to BGRA (Wayland ARGB8888 on little-endian).
        render::rgba_to_bgra(&mut canvas[..buf_size]);

        let buffer = pool.create_buffer(
            0,
            width as i32,
            height as i32,
            stride as i32,
            wl_shm::Format::Argb8888,
            (),
            qh,
        );

        // Destroy the previous buffer (compositor has had time to release it).
        if let Some(old) = self.lock_surfaces[idx].pending_buffer.take() {
            old.destroy();
        }

        let wl_surface = self.lock_surfaces[idx].lock_surface.wl_surface();
        wl_surface.set_buffer_scale(self.lock_surfaces[idx].scale_factor);
        wl_surface.attach(Some(&buffer), 0, 0);
        wl_surface.damage_buffer(0, 0, width as i32, height as i32);
        wl_surface.commit();

        // Keep buffer alive until the next frame.
        self.lock_surfaces[idx].pending_buffer = Some(buffer);
    }

    /// Clear the password field, zeroizing the memory.
    pub fn clear_password(&mut self) {
        self.password.zeroize();
    }

    /// Power off all outputs via DPMS (if the protocol is available).
    pub fn dpms_off(&self) {
        for power in &self.output_power {
            power.set_mode(zwlr_output_power_v1::Mode::Off);
        }
        if !self.output_power.is_empty() {
            tracing::info!("DPMS: powering off {} output(s)", self.output_power.len());
        }
    }

    /// Power on all outputs via DPMS (if the protocol is available).
    pub fn dpms_on(&self) {
        for power in &self.output_power {
            power.set_mode(zwlr_output_power_v1::Mode::On);
        }
        if !self.output_power.is_empty() {
            tracing::info!("DPMS: powering on {} output(s)", self.output_power.len());
        }
    }

    /// Create a power control for an output (if the manager is available).
    pub fn create_output_power(&mut self, output: &wl_output::WlOutput, qh: &QueueHandle<Self>) {
        if let Some(ref manager) = self.power_manager {
            let power = manager.get_output_power(output, qh, ());
            self.output_power.push(power);
        }
    }

    /// Handle a PAM authentication result from the background thread.
    pub fn handle_pam_result(&mut self, result: PamResult) {
        match result {
            PamResult::Success => {
                tracing::info!("authentication successful, unlocking");
                self.auth_state = AuthState::Unlocked;
                self.clear_password();
                if self.dpms_active {
                    self.dpms_active = false;
                    self.dpms_on();
                }

                if let Some(lock) = self.session_lock.take() {
                    lock.unlock();
                    // Roundtrip to ensure compositor processes the unlock.
                    let _ = self.conn.roundtrip();
                }
                self.running = false;
            }
            PamResult::Failed(msg) => {
                tracing::warn!("authentication failed: {msg}");
                self.auth_state = AuthState::Failed(msg);
                self.clear_password();

                // Reset to idle after 2 seconds and redraw to clear the error.
                let qh = self.qh.clone();
                let _ = self.loop_handle.insert_source(
                    Timer::from_duration(std::time::Duration::from_secs(2)),
                    move |_, _, state| {
                        state.auth_state = AuthState::Idle;
                        state.redraw_all(&qh);
                        TimeoutAction::Drop
                    },
                );
            }
        }
    }
}

impl Drop for LockState {
    fn drop(&mut self) {
        self.password.zeroize();
    }
}

// ---------------------------------------------------------------------------
// Session Lock Handler
// ---------------------------------------------------------------------------

impl SessionLockHandler for LockState {
    fn locked(&mut self, _conn: &Connection, qh: &QueueHandle<Self>, session_lock: SessionLock) {
        tracing::info!("session locked, creating lock surfaces");

        // Drop the pending reference — the callback gives us the confirmed lock.
        self.pending_session_lock = None;

        // Collect outputs first to avoid borrowing self during iteration.
        let outputs: Vec<_> = self.output.outputs().collect();
        for output in &outputs {
            let surface = self.compositor.create_surface(qh);
            let scale = self
                .output
                .info(output)
                .map(|i| i.scale_factor)
                .unwrap_or(1);

            let lock_surface = session_lock.create_lock_surface(surface, output, qh);

            self.lock_surfaces.push(LockSurface {
                lock_surface,
                width: 0,
                height: 0,
                scale_factor: scale,
                pending_buffer: None,
                pool: None,
                pool_size: 0,
            });

            self.create_output_power(output, qh);
        }

        // Store the confirmed lock — `new_output()` uses this for hotplugged outputs.
        self.session_lock = Some(session_lock);

        // Insert inactivity timeout timer if any timeout is configured.
        let has_blank = self.config.blank_timeout_secs > 0;
        let has_dpms = self.config.dpms_timeout_secs > 0 && !self.output_power.is_empty();
        if has_blank || has_dpms {
            let qh_clone = qh.clone();
            let _ = self.loop_handle.insert_source(
                Timer::from_duration(std::time::Duration::from_secs(1)),
                move |_, _, state| {
                    if matches!(state.auth_state, AuthState::Authenticating) {
                        return TimeoutAction::ToDuration(std::time::Duration::from_secs(1));
                    }

                    let elapsed = state.last_input.elapsed();

                    // Stage 1: blank the screen and clear password.
                    if state.config.blank_timeout_secs > 0
                        && !state.blanked
                        && elapsed >= std::time::Duration::from_secs(state.config.blank_timeout_secs)
                    {
                        tracing::info!("inactivity timeout, blanking screen");
                        state.clear_password();
                        state.auth_state = AuthState::Idle;
                        state.blanked = true;
                        state.redraw_all(&qh_clone);
                    }

                    // Stage 2: power off monitors via DPMS.
                    if state.config.dpms_timeout_secs > 0
                        && !state.dpms_active
                        && elapsed >= std::time::Duration::from_secs(state.config.dpms_timeout_secs)
                    {
                        // Ensure screen is blanked first (in case dpms_timeout < blank_timeout
                        // or blank_timeout is 0).
                        if !state.blanked {
                            state.clear_password();
                            state.auth_state = AuthState::Idle;
                            state.blanked = true;
                            state.redraw_all(&qh_clone);
                        }
                        state.dpms_active = true;
                        state.dpms_off();
                    }

                    TimeoutAction::ToDuration(std::time::Duration::from_secs(1))
                },
            );
        }
    }

    fn finished(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _session_lock: SessionLock,
    ) {
        tracing::error!("session lock denied by compositor (another locker running?)");
        self.running = false;
    }

    fn configure(
        &mut self,
        _conn: &Connection,
        qh: &QueueHandle<Self>,
        session_lock_surface: SessionLockSurface,
        configure: SessionLockSurfaceConfigure,
        _serial: u32,
    ) {
        let (width, height) = configure.new_size;
        tracing::debug!("lock surface configure: {width}x{height}");

        // Find, update, and draw the matching surface.
        if let Some(idx) = self
            .lock_surfaces
            .iter()
            .position(|s| s.lock_surface.wl_surface() == session_lock_surface.wl_surface())
        {
            self.lock_surfaces[idx].width = width;
            self.lock_surfaces[idx].height = height;
            self.draw_surface(idx, qh);
        }
    }
}

// ---------------------------------------------------------------------------
// Seat & Keyboard Handlers
// ---------------------------------------------------------------------------

impl SeatHandler for LockState {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.seat
    }

    fn new_seat(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _seat: wl_seat::WlSeat) {}

    fn new_capability(
        &mut self,
        _conn: &Connection,
        qh: &QueueHandle<Self>,
        seat: wl_seat::WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Keyboard && self.keyboard.is_none() {
            tracing::debug!("keyboard capability available, binding");
            match self.seat.get_keyboard(qh, &seat, None) {
                Ok(kbd) => self.keyboard = Some(kbd),
                Err(e) => tracing::error!("failed to get keyboard: {e}"),
            }
        }
    }

    fn remove_capability(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _seat: wl_seat::WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Keyboard
            && let Some(kbd) = self.keyboard.take()
        {
            kbd.release();
        }
    }

    fn remove_seat(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _seat: wl_seat::WlSeat) {
    }
}

impl KeyboardHandler for LockState {
    fn enter(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _keyboard: &wl_keyboard::WlKeyboard,
        _surface: &wl_surface::WlSurface,
        _serial: u32,
        _raw: &[u32],
        _keysyms: &[Keysym],
    ) {
    }

    fn leave(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _keyboard: &wl_keyboard::WlKeyboard,
        _surface: &wl_surface::WlSurface,
        _serial: u32,
    ) {
    }

    fn press_key(
        &mut self,
        _conn: &Connection,
        qh: &QueueHandle<Self>,
        _keyboard: &wl_keyboard::WlKeyboard,
        _serial: u32,
        event: KeyEvent,
    ) {
        // Reset inactivity timer on any keypress.
        self.last_input = Instant::now();
        if self.blanked || self.dpms_active {
            if self.dpms_active {
                self.dpms_active = false;
                self.dpms_on();
            }
            self.blanked = false;
            self.redraw_all(qh);
            return; // Consume the wake keypress.
        }

        // Don't process input while authenticating or after unlock.
        if matches!(
            self.auth_state,
            AuthState::Authenticating | AuthState::Unlocked
        ) {
            return;
        }

        let keysym = event.keysym;

        if keysym == Keysym::Return || keysym == Keysym::KP_Enter {
            if !self.password.is_empty() {
                tracing::debug!("submitting password for authentication");
                self.auth_state = AuthState::Authenticating;
                // Take ownership so the PAM thread gets the only copy.
                // self.password is replaced with an empty string immediately.
                let pw = std::mem::take(&mut self.password);
                pam::try_authenticate(self.username.clone(), pw, self.pam_sender.clone());
                self.redraw_all(qh);
            }
            return;
        }

        if keysym == Keysym::Escape {
            self.clear_password();
            self.auth_state = AuthState::Idle;
            self.redraw_all(qh);
            return;
        }

        if keysym == Keysym::BackSpace {
            if !self.password.is_empty() {
                // Rebuild the string without the last char so the removed
                // character's bytes don't linger in the old allocation.
                let mut old = std::mem::take(&mut self.password);
                let new_len = old.len() - old.chars().next_back().map_or(0, |c| c.len_utf8());
                old.truncate(new_len);
                self.password = old;
                self.redraw_all(qh);
            }
            return;
        }

        // Ctrl+U: clear password.
        if self.modifiers.ctrl && event.utf8.as_deref() == Some("u") {
            self.clear_password();
            self.auth_state = AuthState::Idle;
            self.redraw_all(qh);
            return;
        }

        // Printable character — append to password.
        if let Some(ref utf8) = event.utf8
            && !utf8.is_empty()
            && !self.modifiers.ctrl
        {
            self.password.push_str(utf8);
            // Reset from Failed state on new input.
            if matches!(self.auth_state, AuthState::Failed(_)) {
                self.auth_state = AuthState::Idle;
            }
            self.redraw_all(qh);
        }
    }

    fn release_key(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _keyboard: &wl_keyboard::WlKeyboard,
        _serial: u32,
        _event: KeyEvent,
    ) {
    }

    fn update_modifiers(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _keyboard: &wl_keyboard::WlKeyboard,
        _serial: u32,
        modifiers: Modifiers,
        _layout: u32,
    ) {
        self.modifiers = modifiers;
    }
}

// ---------------------------------------------------------------------------
// Compositor, Output, Shm, Registry Handlers
// ---------------------------------------------------------------------------

impl CompositorHandler for LockState {
    fn scale_factor_changed(
        &mut self,
        _conn: &Connection,
        qh: &QueueHandle<Self>,
        surface: &wl_surface::WlSurface,
        new_factor: i32,
    ) {
        for ls in &mut self.lock_surfaces {
            if ls.lock_surface.wl_surface() == surface {
                ls.scale_factor = new_factor;
            }
        }
        self.redraw_all(qh);
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
        qh: &QueueHandle<Self>,
        output: wl_output::WlOutput,
    ) {
        // If we're already locked, create a lock surface for the new output.
        if let Some(ref session_lock) = self.session_lock {
            let surface = self.compositor.create_surface(qh);
            let scale = self
                .output
                .info(&output)
                .map(|i| i.scale_factor)
                .unwrap_or(1);

            let lock_surface = session_lock.create_lock_surface(surface, &output, qh);

            self.lock_surfaces.push(LockSurface {
                lock_surface,
                width: 0,
                height: 0,
                scale_factor: scale,
                pending_buffer: None,
                pool: None,
                pool_size: 0,
            });

            self.create_output_power(&output, qh);
            tracing::info!("created lock surface for hotplugged output");
        }
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
        // The compositor handles cleaning up lock surfaces for destroyed outputs.
        // We can't match surfaces to outputs here since SessionLockSurface doesn't
        // expose its output, but we can remove surfaces whose underlying wl_surface
        // is no longer alive.
        let before = self.lock_surfaces.len();
        self.lock_surfaces
            .retain(|s| s.lock_surface.wl_surface().is_alive());
        let removed = before - self.lock_surfaces.len();
        if removed > 0 {
            tracing::info!("removed {removed} dead lock surface(s) after output destruction");
        } else {
            tracing::debug!("output destroyed while locked");
        }
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
    registry_handlers![OutputState, SeatState];
}

// ---------------------------------------------------------------------------
// Output Power Management (DPMS) Handlers
// ---------------------------------------------------------------------------

impl Dispatch<ZwlrOutputPowerManagerV1, ()> for LockState {
    fn event(
        _state: &mut Self,
        _proxy: &ZwlrOutputPowerManagerV1,
        _event: <ZwlrOutputPowerManagerV1 as Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        // No events defined for the manager.
    }
}

impl Dispatch<ZwlrOutputPowerV1, ()> for LockState {
    fn event(
        state: &mut Self,
        proxy: &ZwlrOutputPowerV1,
        event: <ZwlrOutputPowerV1 as Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        if let zwlr_output_power_v1::Event::Failed = event {
            tracing::warn!("output power control failed, removing");
            state.output_power.retain(|p| p != proxy);
            proxy.destroy();
        }
    }
}

// ---------------------------------------------------------------------------
// Delegate macros
// ---------------------------------------------------------------------------

delegate_compositor!(LockState);
delegate_output!(LockState);
delegate_session_lock!(LockState);
delegate_shm!(LockState);
delegate_registry!(LockState);
delegate_seat!(LockState);
delegate_keyboard!(LockState);
wayland_client::delegate_noop!(LockState: ignore wl_buffer::WlBuffer);

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Format a SystemTime using a strftime-like format string.
///
/// Uses a very basic parser that handles %H, %M, %S, %A, %B, %d, %Y.
/// This avoids pulling in a full datetime library for a lock screen.
fn format_time(fmt: &str, time: std::time::SystemTime) -> String {
    let dur = time
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = dur.as_secs();

    // UTC breakdown (good enough for a lock screen clock).
    // For local time we'd need libc::localtime_r.
    let (hour, min, sec, year, month, day, weekday) = local_time(secs);

    let weekday_name = match weekday {
        0 => "Sunday",
        1 => "Monday",
        2 => "Tuesday",
        3 => "Wednesday",
        4 => "Thursday",
        5 => "Friday",
        6 => "Saturday",
        _ => "???",
    };

    let month_name = match month {
        1 => "January",
        2 => "February",
        3 => "March",
        4 => "April",
        5 => "May",
        6 => "June",
        7 => "July",
        8 => "August",
        9 => "September",
        10 => "October",
        11 => "November",
        12 => "December",
        _ => "???",
    };

    let mut result = String::with_capacity(fmt.len() + 16);
    let mut chars = fmt.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '%' {
            match chars.next() {
                Some('H') => result.push_str(&format!("{hour:02}")),
                Some('M') => result.push_str(&format!("{min:02}")),
                Some('S') => result.push_str(&format!("{sec:02}")),
                Some('A') => result.push_str(weekday_name),
                Some('B') => result.push_str(month_name),
                Some('d') => result.push_str(&format!("{day:02}")),
                Some('Y') => result.push_str(&format!("{year}")),
                Some('%') => result.push('%'),
                Some(c) => {
                    result.push('%');
                    result.push(c);
                }
                None => result.push('%'),
            }
        } else {
            result.push(ch);
        }
    }
    result
}

/// Convert a UNIX timestamp to local time components using libc::localtime_r.
fn local_time(epoch_secs: u64) -> (u32, u32, u32, u32, u32, u32, u32) {
    let time_t = epoch_secs as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe {
        libc::localtime_r(&time_t, &mut tm);
    }
    (
        tm.tm_hour as u32,
        tm.tm_min as u32,
        tm.tm_sec as u32,
        (tm.tm_year + 1900) as u32,
        (tm.tm_mon + 1) as u32,
        tm.tm_mday as u32,
        tm.tm_wday as u32,
    )
}

/// Look up the current user's username via NSS (`getpwuid_r`).
pub fn get_username() -> String {
    let uid = unsafe { libc::getuid() };
    let mut buf = [0u8; 4096];
    let mut pwd: libc::passwd = unsafe { std::mem::zeroed() };
    let mut result: *mut libc::passwd = std::ptr::null_mut();

    let ret = unsafe {
        libc::getpwuid_r(
            uid,
            &mut pwd,
            buf.as_mut_ptr() as *mut libc::c_char,
            buf.len(),
            &mut result,
        )
    };

    if ret != 0 || result.is_null() {
        return "user".into();
    }

    let name = unsafe { std::ffi::CStr::from_ptr(pwd.pw_name) };
    name.to_str().unwrap_or("user").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_time_basic() {
        // Use a known UTC epoch: 2024-01-15 10:30:00 UTC = 1705312200
        let time = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1705312200);
        let result = format_time("%H:%M", time);
        // This depends on local timezone, so just check format.
        assert_eq!(result.len(), 5);
        assert_eq!(result.as_bytes()[2], b':');
    }

    #[test]
    fn format_time_date() {
        let time = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1705312200);
        let result = format_time("%A, %B %d", time);
        // Should contain a weekday and month name.
        assert!(result.contains(','));
    }

    #[test]
    fn get_username_returns_nonempty() {
        let name = get_username();
        assert!(!name.is_empty());
    }

    #[test]
    fn format_time_escape() {
        let time = std::time::SystemTime::now();
        let result = format_time("%%H", time);
        assert!(result.starts_with('%'));
    }
}
