#![allow(dead_code, unused_imports)]

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use psh_core::config::{self, WallMode};
use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState},
    delegate_compositor, delegate_layer, delegate_output, delegate_registry, delegate_shm,
    output::{OutputHandler, OutputInfo, OutputState},
    reexports::{
        calloop::{
            self,
            channel::{Channel, Sender},
            EventLoop, LoopHandle,
        },
        calloop_wayland_source::WaylandSource,
    },
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    shell::{
        WaylandSurface,
        wlr_layer::{
            Anchor, KeyboardInteractivity, Layer, LayerShell, LayerShellHandler, LayerSurface,
            LayerSurfaceConfigure,
        },
    },
    shm::{
        slot::{Buffer, SlotPool},
        Shm, ShmHandler,
    },
};
use wayland_client::{
    globals::registry_queue_init,
    protocol::{wl_output, wl_shm, wl_surface},
    Connection, QueueHandle,
};

// ---------------------------------------------------------------------------
// IPC command sent from the background thread to the calloop main loop
// ---------------------------------------------------------------------------

enum IpcCommand {
    SetWallpaper { path: String },
}

// ---------------------------------------------------------------------------
// Per-output state
// ---------------------------------------------------------------------------

struct OutputSurface {
    output: wl_output::WlOutput,
    layer_surface: LayerSurface,
    buffer: Option<Buffer>,
    width: u32,
    height: u32,
    scale_factor: i32,
    configured: bool,
}

// ---------------------------------------------------------------------------
// Main application state
// ---------------------------------------------------------------------------

struct WallState {
    registry: RegistryState,
    output: OutputState,
    compositor: CompositorState,
    layer_shell: LayerShell,
    shm: Shm,
    pool: SlotPool,
    surfaces: Vec<OutputSurface>,
    image_data: Option<image::RgbaImage>,
    wall_mode: WallMode,
    running: bool,
}

impl WallState {
    fn surface_idx_for_layer(&self, layer: &LayerSurface) -> Option<usize> {
        self.surfaces
            .iter()
            .position(|s| s.layer_surface.wl_surface() == layer.wl_surface())
    }

    fn surface_idx_for_wl(&self, surface: &wl_surface::WlSurface) -> Option<usize> {
        self.surfaces
            .iter()
            .position(|s| s.layer_surface.wl_surface() == surface)
    }

    fn create_output_surface(
        &mut self,
        qh: &QueueHandle<Self>,
        output: wl_output::WlOutput,
    ) {
        let scale = self
            .output
            .info(&output)
            .map(|i| i.scale_factor)
            .unwrap_or(1);

        let name = self
            .output
            .info(&output)
            .and_then(|i| i.name.clone())
            .unwrap_or_else(|| "unknown".into());

        tracing::info!("creating surface for output {name} (scale={scale})");

        let surface = self.compositor.create_surface(qh);
        let layer_surface = self.layer_shell.create_layer_surface(
            qh,
            surface,
            Layer::Background,
            Some("psh-wall"),
            Some(&output),
        );
        layer_surface.set_anchor(Anchor::all());
        layer_surface.set_exclusive_zone(-1);
        layer_surface.set_keyboard_interactivity(KeyboardInteractivity::None);
        layer_surface.wl_surface().set_buffer_scale(scale);
        layer_surface.wl_surface().commit();

        self.surfaces.push(OutputSurface {
            output,
            layer_surface,
            buffer: None,
            width: 0,
            height: 0,
            scale_factor: scale,
            configured: false,
        });
    }

    fn draw(&mut self, idx: usize) {
        let surf = &self.surfaces[idx];
        if surf.width == 0 || surf.height == 0 {
            return;
        }

        let buf_w = surf.width * surf.scale_factor as u32;
        let buf_h = surf.height * surf.scale_factor as u32;
        let stride = buf_w as i32 * 4;

        let (buffer, canvas) = self
            .pool
            .create_buffer(buf_w as i32, buf_h as i32, stride, wl_shm::Format::Argb8888)
            .expect("failed to create buffer");

        // Fill with fallback color first (#1e1e2e — Catppuccin base)
        fill_solid(canvas, 0x1e, 0x1e, 0x2e);

        if let Some(ref img) = self.image_data {
            render_wallpaper(canvas, buf_w, buf_h, img, &self.wall_mode);
        }

        let surf = &mut self.surfaces[idx];
        surf.layer_surface
            .wl_surface()
            .attach(Some(buffer.wl_buffer()), 0, 0);
        surf.layer_surface
            .wl_surface()
            .damage_buffer(0, 0, buf_w as i32, buf_h as i32);
        surf.layer_surface.wl_surface().commit();
        surf.buffer = Some(buffer);
    }

    fn redraw_all(&mut self) {
        let count = self.surfaces.len();
        for i in 0..count {
            if self.surfaces[i].configured {
                self.draw(i);
            }
        }
    }

    fn handle_ipc(&mut self, cmd: IpcCommand) {
        match cmd {
            IpcCommand::SetWallpaper { path } => {
                tracing::info!("IPC: changing wallpaper to {path}");
                self.image_data = load_image(Path::new(&path));
                self.redraw_all();
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Image loading
// ---------------------------------------------------------------------------

fn load_image(path: &Path) -> Option<image::RgbaImage> {
    match image::open(path) {
        Ok(img) => {
            tracing::info!("loaded wallpaper: {}", path.display());
            Some(img.to_rgba8())
        }
        Err(e) => {
            tracing::error!("failed to load wallpaper {}: {e}", path.display());
            None
        }
    }
}

// ---------------------------------------------------------------------------
// Wallpaper rendering
// ---------------------------------------------------------------------------

fn fill_solid(canvas: &mut [u8], r: u8, g: u8, b: u8) {
    for chunk in canvas.chunks_exact_mut(4) {
        chunk[0] = b;
        chunk[1] = g;
        chunk[2] = r;
        chunk[3] = 0xff;
    }
}

#[inline]
fn write_pixel(canvas: &mut [u8], offset: usize, pixel: &image::Rgba<u8>) {
    // ARGB8888 byte order: B, G, R, A
    canvas[offset] = pixel[2];
    canvas[offset + 1] = pixel[1];
    canvas[offset + 2] = pixel[0];
    canvas[offset + 3] = pixel[3];
}

/// Blit `img` onto `canvas` at position (dst_x, dst_y), clipping to canvas bounds.
fn blit_at(canvas: &mut [u8], buf_w: u32, buf_h: u32, img: &image::RgbaImage, dst_x: u32, dst_y: u32) {
    let (iw, ih) = (img.width(), img.height());
    let copy_w = iw.min(buf_w.saturating_sub(dst_x));
    let copy_h = ih.min(buf_h.saturating_sub(dst_y));

    for iy in 0..copy_h {
        let cy = dst_y + iy;
        for ix in 0..copy_w {
            let cx = dst_x + ix;
            let offset = ((cy * buf_w + cx) * 4) as usize;
            write_pixel(canvas, offset, img.get_pixel(ix, iy));
        }
    }
}

/// Blit a sub-region of `img` starting at (src_x, src_y) onto the canvas at (0,0).
fn blit_region(
    canvas: &mut [u8],
    buf_w: u32,
    buf_h: u32,
    img: &image::RgbaImage,
    src_x: u32,
    src_y: u32,
) {
    for dy in 0..buf_h {
        let sy = src_y + dy;
        if sy >= img.height() {
            break;
        }
        for dx in 0..buf_w {
            let sx = src_x + dx;
            if sx >= img.width() {
                break;
            }
            let offset = ((dy * buf_w + dx) * 4) as usize;
            write_pixel(canvas, offset, img.get_pixel(sx, sy));
        }
    }
}

fn render_wallpaper(
    canvas: &mut [u8],
    buf_w: u32,
    buf_h: u32,
    img: &image::RgbaImage,
    mode: &WallMode,
) {
    match mode {
        WallMode::Fill => render_fill(canvas, buf_w, buf_h, img),
        WallMode::Fit => render_fit(canvas, buf_w, buf_h, img),
        WallMode::Stretch => render_stretch(canvas, buf_w, buf_h, img),
        WallMode::Center => render_center(canvas, buf_w, buf_h, img),
        WallMode::Tile => render_tile(canvas, buf_w, buf_h, img),
    }
}

fn render_fill(canvas: &mut [u8], buf_w: u32, buf_h: u32, img: &image::RgbaImage) {
    let (iw, ih) = (img.width() as f64, img.height() as f64);
    let (bw, bh) = (buf_w as f64, buf_h as f64);
    let scale = (bw / iw).max(bh / ih);
    let rw = (iw * scale).ceil() as u32;
    let rh = (ih * scale).ceil() as u32;
    let resized = image::imageops::resize(img, rw, rh, image::imageops::Lanczos3);
    let x_off = rw.saturating_sub(buf_w) / 2;
    let y_off = rh.saturating_sub(buf_h) / 2;
    blit_region(canvas, buf_w, buf_h, &resized, x_off, y_off);
}

fn render_fit(canvas: &mut [u8], buf_w: u32, buf_h: u32, img: &image::RgbaImage) {
    let (iw, ih) = (img.width() as f64, img.height() as f64);
    let (bw, bh) = (buf_w as f64, buf_h as f64);
    let scale = (bw / iw).min(bh / ih);
    let rw = (iw * scale).round() as u32;
    let rh = (ih * scale).round() as u32;
    let resized = image::imageops::resize(img, rw, rh, image::imageops::Lanczos3);
    let x_off = buf_w.saturating_sub(rw) / 2;
    let y_off = buf_h.saturating_sub(rh) / 2;
    blit_at(canvas, buf_w, buf_h, &resized, x_off, y_off);
}

fn render_stretch(canvas: &mut [u8], buf_w: u32, buf_h: u32, img: &image::RgbaImage) {
    let resized = image::imageops::resize(img, buf_w, buf_h, image::imageops::Lanczos3);
    blit_at(canvas, buf_w, buf_h, &resized, 0, 0);
}

fn render_center(canvas: &mut [u8], buf_w: u32, buf_h: u32, img: &image::RgbaImage) {
    let x_off = buf_w.saturating_sub(img.width()) / 2;
    let y_off = buf_h.saturating_sub(img.height()) / 2;
    blit_at(canvas, buf_w, buf_h, img, x_off, y_off);
}

fn render_tile(canvas: &mut [u8], buf_w: u32, buf_h: u32, img: &image::RgbaImage) {
    let (iw, ih) = (img.width(), img.height());
    if iw == 0 || ih == 0 {
        return;
    }
    let mut y = 0u32;
    while y < buf_h {
        let mut x = 0u32;
        while x < buf_w {
            blit_at(canvas, buf_w, buf_h, img, x, y);
            x += iw;
        }
        y += ih;
    }
}

// ---------------------------------------------------------------------------
// IPC background thread
// ---------------------------------------------------------------------------

fn spawn_ipc_listener(sender: Sender<IpcCommand>) {
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("failed to build tokio runtime");

        rt.block_on(async move {
            loop {
                match psh_core::ipc::connect().await {
                    Ok(mut stream) => {
                        tracing::info!("connected to IPC hub");
                        loop {
                            match psh_core::ipc::recv(&mut stream).await {
                                Ok(psh_core::ipc::Message::SetWallpaper { path }) => {
                                    if sender.send(IpcCommand::SetWallpaper { path }).is_err() {
                                        tracing::info!("main loop gone, IPC thread exiting");
                                        return;
                                    }
                                }
                                Ok(_) => {} // ignore other messages
                                Err(e) => {
                                    tracing::warn!("IPC recv error: {e}, reconnecting…");
                                    break;
                                }
                            }
                        }
                    }
                    Err(e) => {
                        tracing::debug!("IPC connect failed: {e}, retrying in 5s");
                    }
                }
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            }
        });
    });
}

// ---------------------------------------------------------------------------
// Wayland protocol handler impls
// ---------------------------------------------------------------------------

impl CompositorHandler for WallState {
    fn scale_factor_changed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        surface: &wl_surface::WlSurface,
        new_factor: i32,
    ) {
        if let Some(idx) = self.surface_idx_for_wl(surface) {
            let surf = &mut self.surfaces[idx];
            if surf.scale_factor != new_factor {
                tracing::info!("scale factor changed to {new_factor}");
                surf.scale_factor = new_factor;
                surf.layer_surface
                    .wl_surface()
                    .set_buffer_scale(new_factor);
                if surf.configured {
                    self.draw(idx);
                }
            }
        }
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

impl OutputHandler for WallState {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output
    }

    fn new_output(
        &mut self,
        _conn: &Connection,
        qh: &QueueHandle<Self>,
        output: wl_output::WlOutput,
    ) {
        self.create_output_surface(qh, output);
    }

    fn update_output(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        output: wl_output::WlOutput,
    ) {
        if let (Some(info), Some(surf)) = (
            self.output.info(&output),
            self.surfaces.iter_mut().find(|s| s.output == output),
        ) {
            let new_scale = info.scale_factor;
            if surf.scale_factor != new_scale {
                tracing::info!("output scale updated to {new_scale}");
                surf.scale_factor = new_scale;
                surf.layer_surface
                    .wl_surface()
                    .set_buffer_scale(new_scale);
            }
        }
    }

    fn output_destroyed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        output: wl_output::WlOutput,
    ) {
        let name = self
            .output
            .info(&output)
            .and_then(|i| i.name.clone())
            .unwrap_or_else(|| "unknown".into());
        tracing::info!("output destroyed: {name}");
        self.surfaces.retain(|s| s.output != output);
    }
}

impl LayerShellHandler for WallState {
    fn closed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        layer: &LayerSurface,
    ) {
        if let Some(idx) = self.surface_idx_for_layer(layer) {
            tracing::info!("layer surface closed, removing");
            self.surfaces.remove(idx);
        }
        if self.surfaces.is_empty() {
            tracing::info!("all surfaces closed, exiting");
            self.running = false;
        }
    }

    fn configure(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        layer: &LayerSurface,
        configure: LayerSurfaceConfigure,
        _serial: u32,
    ) {
        if let Some(idx) = self.surface_idx_for_layer(layer) {
            self.surfaces[idx].width = configure.new_size.0;
            self.surfaces[idx].height = configure.new_size.1;
            self.surfaces[idx].configured = true;
            self.draw(idx);
        }
    }
}

impl ShmHandler for WallState {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

impl ProvidesRegistryState for WallState {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry
    }
    registry_handlers![OutputState];
}

delegate_compositor!(WallState);
delegate_output!(WallState);
delegate_layer!(WallState);
delegate_shm!(WallState);
delegate_registry!(WallState);

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

fn main() {
    psh_core::logging::init("psh_wall");
    tracing::info!("starting psh-wall");

    let cfg = config::load().expect("failed to load config");
    let wall_cfg = cfg.wall;

    // Set up SIGTERM/SIGINT flag
    let shutdown = Arc::new(AtomicBool::new(false));
    signal_hook::flag::register(signal_hook::consts::SIGTERM, Arc::clone(&shutdown))
        .expect("failed to register SIGTERM handler");
    signal_hook::flag::register(signal_hook::consts::SIGINT, Arc::clone(&shutdown))
        .expect("failed to register SIGINT handler");

    // Connect to Wayland
    let conn = Connection::connect_to_env().expect("failed to connect to wayland");
    let (globals, event_queue) = registry_queue_init(&conn).expect("failed to init registry");
    let qh = event_queue.handle();

    let compositor = CompositorState::bind(&globals, &qh).expect("wl_compositor not available");
    let layer_shell = LayerShell::bind(&globals, &qh).expect("wlr-layer-shell not available");
    let shm = Shm::bind(&globals, &qh).expect("wl_shm not available");

    let pool = SlotPool::new(256 * 256 * 4, &shm).expect("failed to create shm pool");
    let image_data = wall_cfg.path.as_deref().and_then(load_image);

    let mut state = WallState {
        registry: RegistryState::new(&globals),
        output: OutputState::new(&globals, &qh),
        compositor,
        layer_shell,
        shm,
        pool,
        surfaces: Vec::new(),
        image_data,
        wall_mode: wall_cfg.mode,
        running: true,
    };

    // Set up calloop event loop
    let mut event_loop: EventLoop<WallState> =
        EventLoop::try_new().expect("failed to create event loop");
    let loop_handle = event_loop.handle();

    // Insert Wayland source
    WaylandSource::new(conn, event_queue)
        .insert(loop_handle.clone())
        .expect("failed to insert wayland source");

    // Insert IPC channel
    let (ipc_sender, ipc_channel) = calloop::channel::channel::<IpcCommand>();
    loop_handle
        .insert_source(ipc_channel, |event, _, state| {
            if let calloop::channel::Event::Msg(cmd) = event {
                state.handle_ipc(cmd);
            }
        })
        .expect("failed to insert IPC channel");

    // Spawn IPC listener thread
    spawn_ipc_listener(ipc_sender);

    // Main event loop
    while state.running {
        if shutdown.load(Ordering::Relaxed) {
            tracing::info!("received signal, shutting down");
            break;
        }
        event_loop
            .dispatch(std::time::Duration::from_millis(250), &mut state)
            .expect("event loop dispatch failed");
    }

    // Clean shutdown — drop surfaces to send destroy requests
    state.surfaces.clear();
    tracing::info!("psh-wall exiting");
}
