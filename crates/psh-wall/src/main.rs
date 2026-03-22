//! psh-wall — wallpaper manager for the psh desktop environment.
//!
//! A pure Wayland client (no GTK) using smithay-client-toolkit to render
//! wallpapers on layer-shell background surfaces. Supports per-output wallpapers,
//! animated images (GIF/APNG/WebP), slideshow mode (directory of images),
//! five wallpaper modes (fill/fit/center/stretch/tile), output hotplug,
//! and live wallpaper changes via IPC.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use psh_core::config::{self, WallConfig, WallMode, expand_tilde};
use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState},
    delegate_compositor, delegate_layer, delegate_output, delegate_registry, delegate_shm,
    output::{OutputHandler, OutputState},
    reexports::{
        calloop::{self, EventLoop, LoopHandle, channel::Sender, timer::{TimeoutAction, Timer}},
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
        Shm, ShmHandler,
        slot::{Buffer, SlotPool},
    },
};
use wayland_client::{
    Connection, QueueHandle,
    globals::registry_queue_init,
    protocol::{wl_output, wl_shm, wl_surface},
};

/// Command sent from the IPC background thread to the calloop main loop via a
/// [`calloop::channel`]. Each variant corresponds to an IPC message or internal event.
enum IpcCommand {
    /// Change the displayed wallpaper to the image at `path`, optionally for a specific output.
    SetWallpaper { path: String, output: Option<String> },
    /// Reload config from disk and apply changes.
    ReloadConfig,
}

/// A single decoded animation frame with its display delay.
struct AnimFrame {
    /// Pre-decoded RGBA image for this frame.
    image: image::RgbaImage,
    /// Delay before advancing to the next frame.
    delay: Duration,
}

/// The image content assigned to an output. An output can display a static image,
/// an animation, or cycle through a directory of images (slideshow).
enum OutputContent {
    /// A single static wallpaper image.
    Static(image::RgbaImage),
    /// An animated image (GIF, APNG, animated WebP) with pre-decoded frames.
    Animated {
        frames: Vec<AnimFrame>,
        current_frame: usize,
    },
    /// Slideshow cycling through images in a directory.
    Slideshow {
        /// Sorted list of image file paths in the directory.
        entries: Vec<PathBuf>,
        /// Index of the currently displayed image in `entries`.
        current_index: usize,
        /// The currently decoded image.
        current_image: image::RgbaImage,
        /// Interval between image changes.
        interval: Duration,
    },
}

/// Deep-clone an `OutputContent`, resetting animation/slideshow position to the start.
fn clone_content(content: &OutputContent) -> OutputContent {
    match content {
        OutputContent::Static(img) => OutputContent::Static(img.clone()),
        OutputContent::Animated { frames, .. } => OutputContent::Animated {
            frames: frames
                .iter()
                .map(|f| AnimFrame {
                    image: f.image.clone(),
                    delay: f.delay,
                })
                .collect(),
            current_frame: 0,
        },
        OutputContent::Slideshow {
            entries,
            current_image,
            interval,
            ..
        } => OutputContent::Slideshow {
            entries: entries.clone(),
            current_index: 0,
            current_image: current_image.clone(),
            interval: *interval,
        },
    }
}

/// Per-output Wayland surface state. One instance exists for each connected monitor.
struct OutputSurface {
    /// Unique ID for timer callback identification (survives index shifts from hotplug).
    id: u64,
    /// Wayland output name (e.g. "DP-1"), resolved from OutputState::info().
    output_name: String,
    /// The Wayland output (monitor) this surface is bound to.
    output: wl_output::WlOutput,
    /// The wlr-layer-shell surface anchored to this output's background layer.
    layer_surface: LayerSurface,
    /// Per-surface shared memory pool for buffer allocation.
    pool: SlotPool,
    /// The most recently attached shm buffer, kept alive to prevent use-after-free.
    buffer: Option<Buffer>,
    /// Surface width in logical pixels (set by the compositor on configure).
    width: u32,
    /// Surface height in logical pixels (set by the compositor on configure).
    height: u32,
    /// HiDPI scale factor for this output. Buffer dimensions are multiplied by this.
    scale_factor: i32,
    /// Whether the compositor has sent an initial configure event for this surface.
    configured: bool,
    /// The wallpaper content this output is currently displaying.
    content: Option<OutputContent>,
    /// The wallpaper scaling mode for this specific output.
    wall_mode: WallMode,
    /// Registration token for the active animation/slideshow timer, if any.
    timer_token: Option<calloop::RegistrationToken>,
}

/// Main application state for psh-wall.
///
/// Holds all Wayland protocol state, per-output surfaces with their own content
/// and shm pools, and the wall configuration for per-output resolution.
struct WallState {
    registry: RegistryState,
    output: OutputState,
    compositor: CompositorState,
    layer_shell: LayerShell,
    shm: Shm,
    /// One [`OutputSurface`] per connected monitor.
    surfaces: Vec<OutputSurface>,
    /// Full wall config for per-output lookup and fallback values.
    config: WallConfig,
    /// calloop loop handle for inserting timer sources.
    loop_handle: LoopHandle<'static, WallState>,
    /// Monotonically increasing ID counter for output surfaces.
    next_surface_id: u64,
    /// Set to `false` to exit the main event loop.
    running: bool,
}

impl WallState {
    /// Returns the index of the [`OutputSurface`] that owns the given layer surface.
    fn surface_idx_for_layer(&self, layer: &LayerSurface) -> Option<usize> {
        self.surfaces
            .iter()
            .position(|s| s.layer_surface.wl_surface() == layer.wl_surface())
    }

    /// Returns the index of the [`OutputSurface`] that owns the given `wl_surface`.
    fn surface_idx_for_wl(&self, surface: &wl_surface::WlSurface) -> Option<usize> {
        self.surfaces
            .iter()
            .position(|s| s.layer_surface.wl_surface() == surface)
    }

    /// Returns the effective (path, mode, interval) for a given output name,
    /// merging per-output overrides with the top-level fallback config.
    fn effective_config_for(&self, output_name: &str) -> (Option<PathBuf>, WallMode, u64) {
        if let Some(oc) = self.config.outputs.get(output_name) {
            let path = oc
                .path
                .clone()
                .or_else(|| self.config.path.clone());
            let mode = oc.mode.unwrap_or(self.config.mode);
            let interval = oc.interval.unwrap_or(self.config.interval);
            (path, mode, interval)
        } else {
            (self.config.path.clone(), self.config.mode, self.config.interval)
        }
    }

    /// Creates a new layer-shell background surface for the given output.
    ///
    /// Resolves per-output config, loads content, and anchors the surface to
    /// all edges with exclusive zone -1 (behind everything).
    fn create_output_surface(&mut self, qh: &QueueHandle<Self>, output: wl_output::WlOutput) {
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

        let (eff_path, eff_mode, eff_interval) = self.effective_config_for(&name);
        let content = load_content_for(eff_path.as_deref(), eff_interval);

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

        let pool = SlotPool::new(1920 * 1080 * 4, &self.shm)
            .expect("failed to create per-surface shm pool");

        let id = self.next_surface_id;
        self.next_surface_id += 1;

        self.surfaces.push(OutputSurface {
            id,
            output_name: name,
            output,
            layer_surface,
            pool,
            buffer: None,
            width: 0,
            height: 0,
            scale_factor: scale,
            configured: false,
            content,
            wall_mode: eff_mode,
            timer_token: None,
        });
    }

    /// Renders the wallpaper (or fallback color) into an shm buffer and commits it
    /// to the surface at the given index. Skips rendering if the surface has zero dimensions.
    fn draw(&mut self, idx: usize) {
        let surf = &mut self.surfaces[idx];
        if surf.width == 0 || surf.height == 0 {
            return;
        }

        let scale = surf.scale_factor.max(1) as u32;
        if surf.scale_factor < 1 {
            tracing::warn!(
                surface = idx,
                scale_factor = surf.scale_factor,
                "unexpected scale factor, clamping to 1"
            );
        }
        let buf_w = surf.width * scale;
        let buf_h = surf.height * scale;
        let stride = buf_w as i32 * 4;

        let has_content = surf.content.is_some();
        tracing::debug!(
            surface = idx,
            buf_w,
            buf_h,
            has_image = has_content,
            "drawing surface"
        );

        let (buffer, canvas) = match surf.pool.create_buffer(
            buf_w as i32,
            buf_h as i32,
            stride,
            wl_shm::Format::Argb8888,
        ) {
            Ok(bc) => bc,
            Err(e) => {
                tracing::error!(surface = idx, buf_w, buf_h, "failed to create buffer: {e}");
                return;
            }
        };

        fill_solid(canvas, 0x1e, 0x1e, 0x2e);

        let current_image: Option<&image::RgbaImage> = match &surf.content {
            Some(OutputContent::Static(img)) => Some(img),
            Some(OutputContent::Animated { frames, current_frame }) => {
                Some(&frames[*current_frame].image)
            }
            Some(OutputContent::Slideshow { current_image, .. }) => Some(current_image),
            None => None,
        };

        if let Some(img) = current_image {
            render_wallpaper(canvas, buf_w, buf_h, img, &surf.wall_mode);
        }

        surf.layer_surface
            .wl_surface()
            .attach(Some(buffer.wl_buffer()), 0, 0);
        surf.layer_surface
            .wl_surface()
            .damage_buffer(0, 0, buf_w as i32, buf_h as i32);
        surf.layer_surface.wl_surface().commit();
        surf.buffer = Some(buffer);
    }

    /// Schedules the next timer tick for an animated or slideshow output.
    fn schedule_tick(&mut self, idx: usize) {
        // Cancel existing timer for this surface.
        if let Some(token) = self.surfaces[idx].timer_token.take() {
            self.loop_handle.remove(token);
        }

        let delay = match &self.surfaces[idx].content {
            Some(OutputContent::Animated { frames, current_frame }) => frames[*current_frame].delay,
            Some(OutputContent::Slideshow { interval, .. }) => *interval,
            _ => return,
        };

        let surface_id = self.surfaces[idx].id;
        match self.loop_handle.insert_source(
            Timer::from_duration(delay),
            move |_, _, state: &mut WallState| {
                state.advance_content(surface_id);
                TimeoutAction::Drop
            },
        ) {
            Ok(token) => self.surfaces[idx].timer_token = Some(token),
            Err(e) => tracing::error!("failed to schedule timer: {e}"),
        }
    }

    /// Advances content for a surface (next animation frame or next slideshow image),
    /// redraws it, and schedules the next tick.
    fn advance_content(&mut self, surface_id: u64) {
        let Some(idx) = self.surfaces.iter().position(|s| s.id == surface_id) else {
            return;
        };

        if !self.surfaces[idx].configured {
            return;
        }

        let needs_redraw = match &mut self.surfaces[idx].content {
            Some(OutputContent::Animated { frames, current_frame }) => {
                *current_frame = (*current_frame + 1) % frames.len();
                true
            }
            Some(OutputContent::Slideshow {
                entries,
                current_index,
                current_image,
                ..
            }) => {
                *current_index = (*current_index + 1) % entries.len();
                if let Some(img) = load_image(&entries[*current_index]) {
                    *current_image = img;
                }
                true
            }
            _ => false,
        };

        if needs_redraw {
            self.draw(idx);
            self.schedule_tick(idx);
        }
    }

    /// Loads or reloads content for a surface at the given index based on effective config.
    fn reload_surface_content(&mut self, idx: usize) {
        let name = self.surfaces[idx].output_name.clone();
        let (eff_path, eff_mode, eff_interval) = self.effective_config_for(&name);
        let content = load_content_for(eff_path.as_deref(), eff_interval);
        self.surfaces[idx].content = content;
        self.surfaces[idx].wall_mode = eff_mode;
        if self.surfaces[idx].configured {
            self.draw(idx);
            self.schedule_tick(idx);
        }
    }

    /// Dispatches an [`IpcCommand`] received from the background IPC listener thread.
    fn handle_ipc(&mut self, cmd: IpcCommand) {
        match cmd {
            IpcCommand::SetWallpaper { path, output } => {
                let path = expand_tilde(Path::new(&path));
                match output {
                    Some(output_name) => {
                        if let Some(idx) =
                            self.surfaces.iter().position(|s| s.output_name == output_name)
                        {
                            tracing::info!(
                                "IPC: setting wallpaper for {output_name} to {}",
                                path.display()
                            );
                            let (_, _, eff_interval) = self.effective_config_for(&output_name);
                            let content = load_content_for(Some(&path), eff_interval);
                            self.surfaces[idx].content = content;
                            if self.surfaces[idx].configured {
                                self.draw(idx);
                                self.schedule_tick(idx);
                            }
                        } else {
                            tracing::warn!("IPC: unknown output {output_name}");
                        }
                    }
                    None => {
                        tracing::info!("IPC: setting all wallpapers to {}", path.display());
                        // Decode once, clone for each surface.
                        let content = load_content_for(Some(&path), self.config.interval);
                        for idx in 0..self.surfaces.len() {
                            self.surfaces[idx].content = content.as_ref().map(clone_content);
                            if self.surfaces[idx].configured {
                                self.draw(idx);
                                self.schedule_tick(idx);
                            }
                        }
                    }
                }
            }
            IpcCommand::ReloadConfig => {
                tracing::info!("reloading config");
                if let Ok(cfg) = config::load() {
                    self.config = cfg.wall;
                    for idx in 0..self.surfaces.len() {
                        self.reload_surface_content(idx);
                    }
                }
            }
        }
    }
}

/// Loads an image from disk and converts it to RGBA8. Returns `None` on failure.
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

/// Image file extensions recognized for slideshow mode.
const IMAGE_EXTENSIONS: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "webp", "bmp", "tiff", "tif",
];

/// Scans a directory for image files and returns them in sorted order.
fn scan_image_directory(dir: &Path) -> Vec<PathBuf> {
    let mut entries: Vec<PathBuf> = match std::fs::read_dir(dir) {
        Ok(rd) => rd
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| {
                p.extension()
                    .and_then(|e| e.to_str())
                    .is_some_and(|e| IMAGE_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
            })
            .collect(),
        Err(e) => {
            tracing::error!("failed to read slideshow directory {}: {e}", dir.display());
            return Vec::new();
        }
    };
    entries.sort();
    entries
}

/// Returns true if the file path has an extension associated with potentially animated formats.
fn is_potentially_animated(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase())
            .as_deref(),
        Some("gif" | "apng" | "webp" | "png")
    )
}

/// Attempts to decode an animated image file into frames using the `image` crate's
/// `AnimationDecoder` trait. Returns `None` if the file is not animated (single frame)
/// or cannot be decoded as animated.
fn load_animated(path: &Path) -> Option<Vec<AnimFrame>> {
    use image::AnimationDecoder;

    let data = std::fs::read(path).ok()?;
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())?;

    let raw_frames: Vec<image::Frame> = match ext.as_str() {
        "gif" => {
            let decoder = image::codecs::gif::GifDecoder::new(std::io::Cursor::new(&data)).ok()?;
            decoder.into_frames().collect_frames().ok()?
        }
        "webp" => {
            let decoder =
                image::codecs::webp::WebPDecoder::new(std::io::Cursor::new(&data)).ok()?;
            decoder.into_frames().collect_frames().ok()?
        }
        "png" | "apng" => {
            let decoder =
                image::codecs::png::PngDecoder::new(std::io::Cursor::new(&data)).ok()?;
            if !decoder.is_apng().ok()? {
                return None;
            }
            decoder.apng().ok()?.into_frames().collect_frames().ok()?
        }
        _ => return None,
    };

    if raw_frames.len() <= 1 {
        return None;
    }

    tracing::info!(
        "loaded animated wallpaper: {} ({} frames)",
        path.display(),
        raw_frames.len()
    );

    let frames = raw_frames
        .into_iter()
        .map(|f| {
            let (numer, denom) = f.delay().numer_denom_ms();
            let delay_ms = (numer as u64) / (denom as u64).max(1);
            AnimFrame {
                image: f.into_buffer(),
                delay: Duration::from_millis(delay_ms.max(16)),
            }
        })
        .collect();

    Some(frames)
}

/// Loads content for an output based on a path and interval.
/// If path is a directory, enters slideshow mode. If it's a potentially animated
/// image, tries animated decoding first, falling back to static.
fn load_content_for(path: Option<&Path>, interval: u64) -> Option<OutputContent> {
    let path = path?;

    if path.is_dir() {
        let entries = scan_image_directory(path);
        if entries.is_empty() {
            tracing::warn!("slideshow directory is empty: {}", path.display());
            return None;
        }
        let first_image = load_image(&entries[0])?;
        return Some(OutputContent::Slideshow {
            entries,
            current_index: 0,
            current_image: first_image,
            interval: Duration::from_secs(interval),
        });
    }

    if is_potentially_animated(path) && let Some(frames) = load_animated(path) {
        return Some(OutputContent::Animated {
            frames,
            current_frame: 0,
        });
    }

    load_image(path).map(OutputContent::Static)
}

/// Fills every pixel in `canvas` with a solid color in ARGB8888 format (fully opaque).
fn fill_solid(canvas: &mut [u8], r: u8, g: u8, b: u8) {
    for chunk in canvas.chunks_exact_mut(4) {
        chunk[0] = b;
        chunk[1] = g;
        chunk[2] = r;
        chunk[3] = 0xff;
    }
}

/// Writes a single pixel to `canvas` at the given byte offset, converting from
/// the `image` crate's RGBA order to the Wayland ARGB8888 byte order (B, G, R, A).
#[inline]
fn write_pixel(canvas: &mut [u8], offset: usize, pixel: &image::Rgba<u8>) {
    canvas[offset] = pixel[2];
    canvas[offset + 1] = pixel[1];
    canvas[offset + 2] = pixel[0];
    canvas[offset + 3] = pixel[3];
}

/// Blit `img` onto `canvas` at position (dst_x, dst_y), clipping to canvas bounds.
fn blit_at(
    canvas: &mut [u8],
    buf_w: u32,
    buf_h: u32,
    img: &image::RgbaImage,
    dst_x: u32,
    dst_y: u32,
) {
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

/// Renders `img` onto `canvas` using the specified [`WallMode`] scaling strategy.
fn render_wallpaper(
    canvas: &mut [u8],
    buf_w: u32,
    buf_h: u32,
    img: &image::RgbaImage,
    mode: &WallMode,
) {
    tracing::debug!(
        mode = ?mode,
        img_w = img.width(),
        img_h = img.height(),
        buf_w,
        buf_h,
        "rendering wallpaper"
    );
    match mode {
        WallMode::Fill => render_fill(canvas, buf_w, buf_h, img),
        WallMode::Fit => render_fit(canvas, buf_w, buf_h, img),
        WallMode::Stretch => render_stretch(canvas, buf_w, buf_h, img),
        WallMode::Center => render_center(canvas, buf_w, buf_h, img),
        WallMode::Tile => render_tile(canvas, buf_w, buf_h, img),
    }
}

/// Scales the image to cover the entire canvas, cropping overflow from the center.
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

/// Scales the image to fit within the canvas, centering it and letterboxing with the
/// fallback background color.
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

/// Stretches the image to fill the canvas exactly, ignoring aspect ratio.
fn render_stretch(canvas: &mut [u8], buf_w: u32, buf_h: u32, img: &image::RgbaImage) {
    let resized = image::imageops::resize(img, buf_w, buf_h, image::imageops::Lanczos3);
    blit_at(canvas, buf_w, buf_h, &resized, 0, 0);
}

/// Places the image at its original size, centered on the canvas without scaling.
fn render_center(canvas: &mut [u8], buf_w: u32, buf_h: u32, img: &image::RgbaImage) {
    let x_off = buf_w.saturating_sub(img.width()) / 2;
    let y_off = buf_h.saturating_sub(img.height()) / 2;
    blit_at(canvas, buf_w, buf_h, img, x_off, y_off);
}

/// Repeats the image at its original size across the entire canvas.
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

/// Spawns a background thread that connects to the psh-bar IPC hub and listens
/// for wallpaper-related messages. Automatically reconnects on disconnection.
/// Commands are forwarded to the calloop main loop via the `sender` channel.
fn spawn_ipc_listener(sender: Sender<IpcCommand>) {
    tracing::info!("spawning IPC listener thread");
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
                                Ok(psh_core::ipc::Message::SetWallpaper {
                                    path,
                                    output,
                                }) => {
                                    if sender
                                        .send(IpcCommand::SetWallpaper { path, output })
                                        .is_err()
                                    {
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

/// Handles compositor events: redraws surfaces when scale factor changes.
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
                surf.layer_surface.wl_surface().set_buffer_scale(new_factor);
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

/// Handles output (monitor) hotplug: creates surfaces for new outputs, updates scale
/// factors, and cleans up surfaces when outputs are removed.
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
        let Some(new_scale) = self.output.info(&output).map(|i| i.scale_factor) else {
            return;
        };
        let Some(idx) = self.surfaces.iter().position(|s| s.output == output) else {
            return;
        };
        if self.surfaces[idx].scale_factor != new_scale {
            tracing::info!("output scale updated to {new_scale}");
            self.surfaces[idx].scale_factor = new_scale;
            self.surfaces[idx]
                .layer_surface
                .wl_surface()
                .set_buffer_scale(new_scale);
            if self.surfaces[idx].configured {
                self.draw(idx);
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
        // Cancel any active timers before removing surfaces.
        for surf in &mut self.surfaces {
            if surf.output == output && let Some(token) = surf.timer_token.take() {
                self.loop_handle.remove(token);
            }
        }
        self.surfaces.retain(|s| s.output != output);
    }
}

/// Handles layer-shell events: draws wallpaper on configure and cleans up on close.
impl LayerShellHandler for WallState {
    fn closed(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, layer: &LayerSurface) {
        if let Some(idx) = self.surface_idx_for_layer(layer) {
            tracing::info!("layer surface closed, removing");
            if let Some(token) = self.surfaces[idx].timer_token.take() {
                self.loop_handle.remove(token);
            }
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
            self.schedule_tick(idx);
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

fn main() {
    psh_core::logging::init("psh_wall");
    tracing::info!("starting psh-wall");

    let cfg = config::load().expect("failed to load config");
    let wall_cfg = cfg.wall;
    tracing::debug!(path = ?wall_cfg.path, mode = ?wall_cfg.mode, "loaded config");

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

    // Set up calloop event loop
    let mut event_loop: EventLoop<WallState> =
        EventLoop::try_new().expect("failed to create event loop");
    let loop_handle = event_loop.handle();

    let mut state = WallState {
        registry: RegistryState::new(&globals),
        output: OutputState::new(&globals, &qh),
        compositor,
        layer_shell,
        shm,
        surfaces: Vec::new(),
        config: wall_cfg,
        loop_handle: loop_handle.clone(),
        next_surface_id: 0,
        running: true,
    };

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
    spawn_ipc_listener(ipc_sender.clone());

    // Spawn config file watcher
    let config_sender = ipc_sender;
    std::thread::spawn(move || {
        let config_path = config::config_path();
        if let Ok((_tx, _watcher)) = config::watch(config_path) {
            let mut rx = _tx.subscribe();
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("failed to build tokio runtime for config watcher");
            rt.block_on(async {
                while rx.recv().await.is_ok() {
                    let _ = config_sender.send(IpcCommand::ReloadConfig);
                }
            });
        }
    });

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

#[cfg(test)]
mod tests {
    use super::*;
    use psh_core::config::WallMode;

    /// Creates a solid-color RGBA test image.
    fn make_image(w: u32, h: u32, pixel: [u8; 4]) -> image::RgbaImage {
        image::RgbaImage::from_fn(w, h, |_, _| image::Rgba(pixel))
    }

    /// Allocates a canvas buffer of the given dimensions (4 bytes per pixel).
    fn make_canvas(w: u32, h: u32) -> Vec<u8> {
        vec![0u8; (w * h * 4) as usize]
    }

    /// Reads the BGRA bytes at the given canvas coordinate.
    fn read_pixel(canvas: &[u8], buf_w: u32, x: u32, y: u32) -> [u8; 4] {
        let offset = ((y * buf_w + x) * 4) as usize;
        [
            canvas[offset],
            canvas[offset + 1],
            canvas[offset + 2],
            canvas[offset + 3],
        ]
    }

    // ---- fill_solid ----

    #[test]
    fn test_fill_solid_sets_all_pixels() {
        let mut canvas = make_canvas(3, 2);
        fill_solid(&mut canvas, 0xAA, 0xBB, 0xCC);
        // ARGB8888 byte order: B, G, R, A
        for chunk in canvas.chunks_exact(4) {
            assert_eq!(chunk, [0xCC, 0xBB, 0xAA, 0xFF]);
        }
    }

    // ---- write_pixel ----

    #[test]
    fn test_write_pixel_argb_order() {
        let mut canvas = [0u8; 4];
        let pixel = image::Rgba([0x11, 0x22, 0x33, 0x44]); // R=0x11, G=0x22, B=0x33, A=0x44
        write_pixel(&mut canvas, 0, &pixel);
        // Expect BGRA order: B=0x33, G=0x22, R=0x11, A=0x44
        assert_eq!(canvas, [0x33, 0x22, 0x11, 0x44]);
    }

    // ---- blit_at ----

    #[test]
    fn test_blit_at_origin() {
        let img = make_image(2, 2, [0xFF, 0x00, 0x00, 0xFF]); // red
        let mut canvas = make_canvas(4, 4);
        fill_solid(&mut canvas, 0, 0, 0);
        blit_at(&mut canvas, 4, 4, &img, 0, 0);

        // Top-left 2x2 should be red (BGRA: 0x00, 0x00, 0xFF, 0xFF)
        assert_eq!(read_pixel(&canvas, 4, 0, 0), [0x00, 0x00, 0xFF, 0xFF]);
        assert_eq!(read_pixel(&canvas, 4, 1, 1), [0x00, 0x00, 0xFF, 0xFF]);
        // Outside the blit region should be black
        assert_eq!(read_pixel(&canvas, 4, 2, 0), [0x00, 0x00, 0x00, 0xFF]);
    }

    #[test]
    fn test_blit_at_offset() {
        let img = make_image(1, 1, [0x00, 0xFF, 0x00, 0xFF]); // green
        let mut canvas = make_canvas(4, 4);
        fill_solid(&mut canvas, 0, 0, 0);
        blit_at(&mut canvas, 4, 4, &img, 2, 3);

        // Only (2,3) should be green
        assert_eq!(read_pixel(&canvas, 4, 2, 3), [0x00, 0xFF, 0x00, 0xFF]);
        // Adjacent pixels should remain black
        assert_eq!(read_pixel(&canvas, 4, 1, 3), [0x00, 0x00, 0x00, 0xFF]);
        assert_eq!(read_pixel(&canvas, 4, 3, 3), [0x00, 0x00, 0x00, 0xFF]);
    }

    #[test]
    fn test_blit_at_clips_to_bounds() {
        // 3x3 image blitted at (2,2) on a 4x4 canvas — only a 2x2 region should be written
        let img = make_image(3, 3, [0x00, 0x00, 0xFF, 0xFF]); // blue
        let mut canvas = make_canvas(4, 4);
        fill_solid(&mut canvas, 0, 0, 0);
        blit_at(&mut canvas, 4, 4, &img, 2, 2);

        // (2,2) and (3,3) should be blue
        assert_eq!(read_pixel(&canvas, 4, 2, 2), [0xFF, 0x00, 0x00, 0xFF]);
        assert_eq!(read_pixel(&canvas, 4, 3, 3), [0xFF, 0x00, 0x00, 0xFF]);
        // (1,1) should still be black
        assert_eq!(read_pixel(&canvas, 4, 1, 1), [0x00, 0x00, 0x00, 0xFF]);
    }

    #[test]
    fn test_blit_at_zero_canvas() {
        let img = make_image(2, 2, [0xFF, 0x00, 0x00, 0xFF]);
        let mut canvas = make_canvas(0, 0);
        // Should not panic
        blit_at(&mut canvas, 0, 0, &img, 0, 0);
    }

    // ---- blit_region ----

    #[test]
    fn test_blit_region_extracts_subregion() {
        // 4x4 image, blit the bottom-right 2x2 onto a 2x2 canvas
        let mut img = image::RgbaImage::new(4, 4);
        // Set bottom-right quadrant to green
        for y in 2..4 {
            for x in 2..4 {
                img.put_pixel(x, y, image::Rgba([0x00, 0xFF, 0x00, 0xFF]));
            }
        }
        let mut canvas = make_canvas(2, 2);
        blit_region(&mut canvas, 2, 2, &img, 2, 2);

        assert_eq!(read_pixel(&canvas, 2, 0, 0), [0x00, 0xFF, 0x00, 0xFF]);
        assert_eq!(read_pixel(&canvas, 2, 1, 1), [0x00, 0xFF, 0x00, 0xFF]);
    }

    // ---- render_center ----

    #[test]
    fn test_render_center_places_image_in_middle() {
        let img = make_image(2, 2, [0xFF, 0x00, 0x00, 0xFF]); // red
        let mut canvas = make_canvas(6, 6);
        fill_solid(&mut canvas, 0, 0, 0);
        render_center(&mut canvas, 6, 6, &img);

        // Image should be at (2,2) to (3,3)
        assert_eq!(read_pixel(&canvas, 6, 2, 2), [0x00, 0x00, 0xFF, 0xFF]);
        assert_eq!(read_pixel(&canvas, 6, 3, 3), [0x00, 0x00, 0xFF, 0xFF]);
        // Corners should be black
        assert_eq!(read_pixel(&canvas, 6, 0, 0), [0x00, 0x00, 0x00, 0xFF]);
        assert_eq!(read_pixel(&canvas, 6, 5, 5), [0x00, 0x00, 0x00, 0xFF]);
    }

    // ---- render_tile ----

    #[test]
    fn test_render_tile_repeats() {
        let img = make_image(2, 2, [0xFF, 0x00, 0x00, 0xFF]); // red
        let mut canvas = make_canvas(4, 4);
        fill_solid(&mut canvas, 0, 0, 0);
        render_tile(&mut canvas, 4, 4, &img);

        // All pixels should be red since 2x2 tiles perfectly into 4x4
        for y in 0..4 {
            for x in 0..4 {
                assert_eq!(
                    read_pixel(&canvas, 4, x, y),
                    [0x00, 0x00, 0xFF, 0xFF],
                    "pixel ({x},{y}) should be red"
                );
            }
        }
    }

    #[test]
    fn test_render_tile_zero_size_image() {
        let img = make_image(0, 0, [0xFF, 0x00, 0x00, 0xFF]);
        let mut canvas = make_canvas(4, 4);
        // Should not panic or loop infinitely
        render_tile(&mut canvas, 4, 4, &img);
    }

    // ---- render_stretch ----

    #[test]
    fn test_render_stretch_covers_canvas() {
        let img = make_image(1, 1, [0xFF, 0x00, 0x00, 0xFF]); // red
        let mut canvas = make_canvas(4, 4);
        fill_solid(&mut canvas, 0, 0, 0);
        render_stretch(&mut canvas, 4, 4, &img);

        // Every pixel should have been written (no longer the fill color)
        for y in 0..4 {
            for x in 0..4 {
                let px = read_pixel(&canvas, 4, x, y);
                assert_ne!(
                    px,
                    [0x00, 0x00, 0x00, 0xFF],
                    "pixel ({x},{y}) should not be black after stretch"
                );
            }
        }
    }

    // ---- render_fill ----

    #[test]
    fn test_render_fill_covers_canvas() {
        // Wide image on a tall canvas — fill should cover everything
        let img = make_image(8, 2, [0x00, 0xFF, 0x00, 0xFF]); // green
        let mut canvas = make_canvas(4, 4);
        fill_solid(&mut canvas, 0, 0, 0);
        render_fill(&mut canvas, 4, 4, &img);

        // No pixel should remain the fill color
        for y in 0..4 {
            for x in 0..4 {
                let px = read_pixel(&canvas, 4, x, y);
                assert_ne!(
                    px,
                    [0x00, 0x00, 0x00, 0xFF],
                    "pixel ({x},{y}) should not be black after fill"
                );
            }
        }
    }

    // ---- render_fit ----

    #[test]
    fn test_render_fit_letterboxes() {
        // Wide image (8x4) fit into 4x8 — scales to 4x2, centered vertically with 3px
        // letterbox on top and bottom
        let img = make_image(8, 4, [0xFF, 0x00, 0x00, 0xFF]); // red
        let mut canvas = make_canvas(4, 8);
        fill_solid(&mut canvas, 0, 0, 0);
        render_fit(&mut canvas, 4, 8, &img);

        // Top row should still be the fill color (letterboxed)
        assert_eq!(
            read_pixel(&canvas, 4, 0, 0),
            [0x00, 0x00, 0x00, 0xFF],
            "top-left should be letterbox"
        );
        // Bottom row should still be fill color
        assert_eq!(
            read_pixel(&canvas, 4, 0, 7),
            [0x00, 0x00, 0x00, 0xFF],
            "bottom-left should be letterbox"
        );

        // The middle rows should have image content (not black)
        let mid = read_pixel(&canvas, 4, 2, 4);
        assert_ne!(
            mid,
            [0x00, 0x00, 0x00, 0xFF],
            "center should have image content"
        );
    }

    // ---- render_wallpaper dispatch ----

    #[test]
    fn test_render_wallpaper_dispatches_all_modes() {
        // Ensure render_wallpaper doesn't panic for any mode
        let img = make_image(4, 4, [0xFF, 0x00, 0x00, 0xFF]);
        for mode in [
            WallMode::Fill,
            WallMode::Fit,
            WallMode::Stretch,
            WallMode::Center,
            WallMode::Tile,
        ] {
            let mut canvas = make_canvas(8, 8);
            render_wallpaper(&mut canvas, 8, 8, &img, &mode);
        }
    }

    // ---- scan_image_directory ----

    #[test]
    fn test_scan_image_directory_filters_correctly() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.png"), b"").unwrap();
        std::fs::write(dir.path().join("b.jpg"), b"").unwrap();
        std::fs::write(dir.path().join("c.txt"), b"").unwrap();
        std::fs::write(dir.path().join("d.webp"), b"").unwrap();
        std::fs::write(dir.path().join("e.rs"), b"").unwrap();

        let entries = scan_image_directory(dir.path());
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].file_name().unwrap(), "a.png");
        assert_eq!(entries[1].file_name().unwrap(), "b.jpg");
        assert_eq!(entries[2].file_name().unwrap(), "d.webp");
    }

    #[test]
    fn test_scan_image_directory_empty() {
        let dir = tempfile::tempdir().unwrap();
        let entries = scan_image_directory(dir.path());
        assert!(entries.is_empty());
    }

    #[test]
    fn test_scan_image_directory_nonexistent() {
        let entries = scan_image_directory(Path::new("/nonexistent/dir"));
        assert!(entries.is_empty());
    }

    // ---- is_potentially_animated ----

    #[test]
    fn test_is_potentially_animated() {
        assert!(is_potentially_animated(Path::new("test.gif")));
        assert!(is_potentially_animated(Path::new("test.webp")));
        assert!(is_potentially_animated(Path::new("test.png")));
        assert!(is_potentially_animated(Path::new("test.apng")));
        assert!(!is_potentially_animated(Path::new("test.jpg")));
        assert!(!is_potentially_animated(Path::new("test.bmp")));
        assert!(!is_potentially_animated(Path::new("test.tiff")));
    }

    // ---- load_content_for ----

    #[test]
    fn test_load_content_for_none_path() {
        assert!(load_content_for(None, 300).is_none());
    }

    #[test]
    fn test_load_content_for_nonexistent_file() {
        assert!(load_content_for(Some(Path::new("/nonexistent/image.png")), 300).is_none());
    }

    #[test]
    fn test_load_content_for_empty_directory() {
        let dir = tempfile::tempdir().unwrap();
        assert!(load_content_for(Some(dir.path()), 300).is_none());
    }

    // ---- IMAGE_EXTENSIONS ----

    #[test]
    fn test_image_extensions() {
        assert!(IMAGE_EXTENSIONS.contains(&"png"));
        assert!(IMAGE_EXTENSIONS.contains(&"jpg"));
        assert!(IMAGE_EXTENSIONS.contains(&"jpeg"));
        assert!(IMAGE_EXTENSIONS.contains(&"gif"));
        assert!(IMAGE_EXTENSIONS.contains(&"webp"));
        assert!(IMAGE_EXTENSIONS.contains(&"bmp"));
        assert!(!IMAGE_EXTENSIONS.contains(&"txt"));
        assert!(!IMAGE_EXTENSIONS.contains(&"rs"));
    }
}
