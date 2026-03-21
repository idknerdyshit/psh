//! Lock screen rendering using tiny-skia and ab_glyph.
//!
//! Renders the password entry UI onto shared memory buffers that are then
//! attached to Wayland lock surfaces.

use ab_glyph::{Font, FontArc, ScaleFont};
use psh_core::config::LockConfig;
use tiny_skia::{Color, FillRule, Paint, PathBuilder, Pixmap, Transform};

use crate::state::AuthState;

/// Googled font search paths for common Linux distributions.
const FONT_SEARCH_PATHS: &[&str] = &[
    "/usr/share/fonts/TTF/DejaVuSans.ttf",
    "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
    "/usr/share/fonts/dejavu-sans-fonts/DejaVuSans.ttf",
    "/usr/share/fonts/noto/NotoSans-Regular.ttf",
    "/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf",
    "/usr/share/fonts/google-noto/NotoSans-Regular.ttf",
    "/usr/share/fonts/liberation-sans/LiberationSans-Regular.ttf",
    "/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf",
];

/// Embedded fallback font (Inter Regular subset) for when no system font is found.
/// Using DejaVu Sans as it's the most common across distros.
const FALLBACK_FONT: &[u8] = include_bytes!("fonts/Inter-Regular.ttf");

/// State needed for rendering a lock surface.
pub struct RenderState {
    pub font: FontArc,
}

impl RenderState {
    /// Create a new render state, loading the best available font.
    pub fn new() -> Self {
        let font = load_font();
        Self { font }
    }
}

/// Snapshot of lock state needed for rendering (avoids borrow issues).
pub struct RenderParams<'a> {
    pub config: &'a LockConfig,
    pub username: &'a str,
    pub password_len: usize,
    pub auth_state: &'a AuthState,
    pub time_text: &'a str,
    pub date_text: &'a str,
}

/// Render the lock screen UI into a pixmap.
///
/// Returns a `Pixmap` with the rendered content in premultiplied RGBA.
/// The caller must convert to the Wayland shm format (ARGB8888 / BGRA).
pub fn render_lock_surface(
    width: u32,
    height: u32,
    render: &RenderState,
    params: &RenderParams<'_>,
) -> Option<Pixmap> {
    let mut pixmap = Pixmap::new(width, height)?;

    // Background fill
    let bg = parse_hex_color(&params.config.background_color);
    pixmap.fill(bg);

    let cx = width as f32 / 2.0;
    let base_size = params.config.font_size;

    // Vertical layout from center
    let mut y = height as f32 * 0.30;

    // Clock
    if params.config.show_clock && !params.time_text.is_empty() {
        let clock_size = base_size * 3.0;
        let text_color = parse_hex_color(&params.config.password_dot_color);
        draw_text_centered(
            &mut pixmap,
            &render.font,
            clock_size,
            cx,
            y,
            params.time_text,
            text_color,
        );
        y += clock_size * 1.2;

        // Date
        if !params.date_text.is_empty() {
            let date_size = base_size * 0.8;
            draw_text_centered(
                &mut pixmap,
                &render.font,
                date_size,
                cx,
                y,
                params.date_text,
                text_color,
            );
            y += date_size * 2.0;
        }
    } else {
        y = height as f32 * 0.40;
    }

    // Username
    if params.config.show_username && !params.username.is_empty() {
        let name_size = base_size * 1.0;
        let text_color = parse_hex_color(&params.config.password_dot_color);
        draw_text_centered(
            &mut pixmap,
            &render.font,
            name_size,
            cx,
            y,
            params.username,
            text_color,
        );
        y += name_size * 2.5;
    }

    // Password dots
    let dot_color = parse_hex_color(&params.config.password_dot_color);
    draw_password_dots(&mut pixmap, cx, y, params.password_len, dot_color);
    y += 40.0;

    // Auth state indicator
    match params.auth_state {
        AuthState::Authenticating => {
            let text_color = parse_hex_color(&params.config.password_dot_color);
            draw_text_centered(
                &mut pixmap,
                &render.font,
                base_size * 0.7,
                cx,
                y + 20.0,
                "verifying...",
                text_color,
            );
        }
        AuthState::Failed(msg) => {
            let err_color = parse_hex_color(&params.config.error_color);
            draw_text_centered(
                &mut pixmap,
                &render.font,
                base_size * 0.8,
                cx,
                y + 20.0,
                msg,
                err_color,
            );
        }
        AuthState::Idle | AuthState::Unlocked => {}
    }

    Some(pixmap)
}

/// Draw password indicator dots centered horizontally at the given y position.
fn draw_password_dots(pixmap: &mut Pixmap, cx: f32, cy: f32, count: usize, color: Color) {
    if count == 0 {
        return;
    }

    let dot_radius = 6.0_f32;
    let dot_spacing = 20.0_f32;
    let max_dots = 32; // Don't draw more than 32 dots
    let n = count.min(max_dots);

    let total_width = (n as f32 - 1.0) * dot_spacing;
    let start_x = cx - total_width / 2.0;

    let mut paint = Paint::default();
    paint.set_color(color);
    paint.anti_alias = true;

    for i in 0..n {
        let x = start_x + i as f32 * dot_spacing;
        if let Some(path) = circle_path(x, cy, dot_radius) {
            pixmap.fill_path(&path, &paint, FillRule::Winding, Transform::identity(), None);
        }
    }
}

/// Create a tiny-skia path for a filled circle.
fn circle_path(cx: f32, cy: f32, r: f32) -> Option<tiny_skia::Path> {
    // Approximate circle with 4 cubic bezier curves.
    let k = 0.552_284_8_f32; // Magic constant for cubic bezier circle approximation
    let mut pb = PathBuilder::new();
    pb.move_to(cx, cy - r);
    pb.cubic_to(cx + r * k, cy - r, cx + r, cy - r * k, cx + r, cy);
    pb.cubic_to(cx + r, cy + r * k, cx + r * k, cy + r, cx, cy + r);
    pb.cubic_to(cx - r * k, cy + r, cx - r, cy + r * k, cx - r, cy);
    pb.cubic_to(cx - r, cy - r * k, cx - r * k, cy - r, cx, cy - r);
    pb.close();
    pb.finish()
}

/// Draw text centered horizontally at the given position using ab_glyph.
fn draw_text_centered(
    pixmap: &mut Pixmap,
    font: &FontArc,
    size: f32,
    cx: f32,
    y: f32,
    text: &str,
    color: Color,
) {
    let scaled = font.as_scaled(size);

    // Measure total width for centering.
    let mut total_width = 0.0_f32;
    let mut last_glyph_id = None;
    for ch in text.chars() {
        let glyph_id = scaled.glyph_id(ch);
        if let Some(last) = last_glyph_id {
            total_width += scaled.kern(last, glyph_id);
        }
        total_width += scaled.h_advance(glyph_id);
        last_glyph_id = Some(glyph_id);
    }

    let start_x = cx - total_width / 2.0;
    let mut cursor_x = start_x;
    let ascent = scaled.ascent();

    let r = (color.red() * 255.0) as u8;
    let g = (color.green() * 255.0) as u8;
    let b = (color.blue() * 255.0) as u8;

    let pw = pixmap.width() as i32;
    let ph = pixmap.height() as i32;

    last_glyph_id = None;
    for ch in text.chars() {
        let glyph_id = scaled.glyph_id(ch);
        if let Some(last) = last_glyph_id {
            cursor_x += scaled.kern(last, glyph_id);
        }

        let glyph = glyph_id.with_scale_and_position(size, ab_glyph::point(cursor_x, y + ascent));

        if let Some(outlined) = font.outline_glyph(glyph) {
            let bounds = outlined.px_bounds();
            outlined.draw(|gx, gy, coverage| {
                let px = bounds.min.x as i32 + gx as i32;
                let py = bounds.min.y as i32 + gy as i32;
                if px >= 0 && py >= 0 && px < pw && py < ph {
                    let alpha = (coverage * 255.0) as u8;
                    if alpha > 0 {
                        let idx = (py as usize * pw as usize + px as usize) * 4;
                        let data = pixmap.data_mut();
                        // Premultiplied alpha blending onto existing pixel.
                        let src_a = alpha as u32;
                        let inv_a = 255 - src_a;
                        data[idx] =
                            ((r as u32 * src_a + data[idx] as u32 * inv_a) / 255) as u8;
                        data[idx + 1] =
                            ((g as u32 * src_a + data[idx + 1] as u32 * inv_a) / 255) as u8;
                        data[idx + 2] =
                            ((b as u32 * src_a + data[idx + 2] as u32 * inv_a) / 255) as u8;
                        data[idx + 3] =
                            ((src_a + data[idx + 3] as u32 * inv_a / 255).min(255)) as u8;
                    }
                }
            });
        }

        cursor_x += scaled.h_advance(glyph_id);
        last_glyph_id = Some(glyph_id);
    }
}

/// Parse a hex color string like "#rrggbb" into a tiny-skia Color.
pub fn parse_hex_color(hex: &str) -> Color {
    let hex = hex.trim_start_matches('#');
    if hex.len() >= 6 {
        let r = u8::from_str_radix(&hex[0..2], 16).unwrap_or(0);
        let g = u8::from_str_radix(&hex[2..4], 16).unwrap_or(0);
        let b = u8::from_str_radix(&hex[4..6], 16).unwrap_or(0);
        Color::from_rgba8(r, g, b, 255)
    } else {
        Color::from_rgba8(0x1e, 0x1e, 0x2e, 0xff) // Fallback to catppuccin base
    }
}

/// Load a font, trying system paths first then falling back to embedded font.
fn load_font() -> FontArc {
    // Try system fonts.
    for path in FONT_SEARCH_PATHS {
        if let Ok(font) = std::fs::read(path).and_then(|data| {
            FontArc::try_from_vec(data).map_err(std::io::Error::other)
        }) {
            tracing::info!("loaded font from {path}");
            return font;
        }
    }

    // Fallback to embedded font.
    tracing::info!("using embedded fallback font");
    FontArc::try_from_slice(FALLBACK_FONT).expect("embedded font is valid")
}

/// Convert a tiny-skia RGBA premultiplied pixmap to BGRA (Wayland ARGB8888 byte order).
///
/// Wayland's `argb8888` format is stored as B, G, R, A in memory on little-endian.
pub fn rgba_to_bgra(data: &mut [u8]) {
    for chunk in data.chunks_exact_mut(4) {
        chunk.swap(0, 2); // R <-> B
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_hex_colors() {
        let c = parse_hex_color("#ff0000");
        assert_eq!(c.red(), 1.0);
        assert_eq!(c.green(), 0.0);
        assert_eq!(c.blue(), 0.0);

        let c = parse_hex_color("#00ff00");
        assert_eq!(c.green(), 1.0);

        let c = parse_hex_color("1e1e2e");
        assert_eq!(c.red(), 0x1e as f32 / 255.0);
    }

    #[test]
    fn parse_invalid_hex_returns_fallback() {
        let c = parse_hex_color("bad");
        assert_eq!(c.red(), 0x1e as f32 / 255.0); // catppuccin base
    }

    #[test]
    fn circle_path_creates_valid_path() {
        let path = circle_path(100.0, 100.0, 10.0);
        assert!(path.is_some());
    }

    #[test]
    fn rgba_to_bgra_swaps_channels() {
        let mut data = vec![0xFF, 0x00, 0x00, 0xFF]; // RGBA red
        rgba_to_bgra(&mut data);
        assert_eq!(data, vec![0x00, 0x00, 0xFF, 0xFF]); // BGRA red
    }

    #[test]
    fn render_produces_pixmap() {
        let render = RenderState::new();
        let params = RenderParams {
            config: &LockConfig::default(),
            username: "testuser",
            password_len: 5,
            auth_state: &AuthState::Idle,
            time_text: "12:34",
            date_text: "Monday, March 21",
        };
        let pixmap = render_lock_surface(800, 600, &render, &params);
        assert!(pixmap.is_some());
    }

    #[test]
    fn dot_layout_zero_dots() {
        // Just make sure it doesn't panic with 0 dots.
        let mut pixmap = Pixmap::new(100, 100).unwrap();
        draw_password_dots(&mut pixmap, 50.0, 50.0, 0, Color::WHITE);
    }

    #[test]
    fn dot_layout_many_dots() {
        // 50 dots should be clamped to max_dots (32).
        let mut pixmap = Pixmap::new(800, 100).unwrap();
        draw_password_dots(&mut pixmap, 400.0, 50.0, 50, Color::WHITE);
    }

    #[test]
    fn render_with_auth_failed() {
        let render = RenderState::new();
        let params = RenderParams {
            config: &LockConfig::default(),
            username: "testuser",
            password_len: 0,
            auth_state: &AuthState::Failed("wrong password".into()),
            time_text: "12:34",
            date_text: "Monday, March 21",
        };
        let pixmap = render_lock_surface(800, 600, &render, &params);
        assert!(pixmap.is_some());
    }

    #[test]
    fn render_with_authenticating() {
        let render = RenderState::new();
        let params = RenderParams {
            config: &LockConfig::default(),
            username: "",
            password_len: 8,
            auth_state: &AuthState::Authenticating,
            time_text: "",
            date_text: "",
        };
        let pixmap = render_lock_surface(800, 600, &render, &params);
        assert!(pixmap.is_some());
    }

    #[test]
    fn render_no_clock() {
        let mut config = LockConfig::default();
        config.show_clock = false;
        config.show_username = false;
        let render = RenderState::new();
        let params = RenderParams {
            config: &config,
            username: "",
            password_len: 3,
            auth_state: &AuthState::Idle,
            time_text: "",
            date_text: "",
        };
        let pixmap = render_lock_surface(640, 480, &render, &params);
        assert!(pixmap.is_some());
    }

    #[test]
    fn render_zero_size_returns_none() {
        let render = RenderState::new();
        let params = RenderParams {
            config: &LockConfig::default(),
            username: "user",
            password_len: 0,
            auth_state: &AuthState::Idle,
            time_text: "",
            date_text: "",
        };
        assert!(render_lock_surface(0, 0, &render, &params).is_none());
    }
}
