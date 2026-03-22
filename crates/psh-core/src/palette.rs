//! Palette extraction from psh theme CSS and export to GTK/Qt system themes.
//!
//! Parses `@define-color psh-<name> #rrggbb;` lines from a psh theme CSS file
//! and generates GTK3/GTK4 CSS overrides and Qt5ct/Qt6ct color scheme files
//! that match the palette.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use tracing::info;

use crate::Result;

/// A parsed color from the theme CSS.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Color {
    /// Parse a `#rrggbb` hex string.
    pub fn from_hex(hex: &str) -> Option<Self> {
        let hex = hex.trim_start_matches('#');
        if hex.len() < 6 {
            return None;
        }
        Some(Self {
            r: u8::from_str_radix(&hex[0..2], 16).ok()?,
            g: u8::from_str_radix(&hex[2..4], 16).ok()?,
            b: u8::from_str_radix(&hex[4..6], 16).ok()?,
        })
    }

    /// Format as `#rrggbb`.
    pub fn to_hex(&self) -> String {
        format!("#{:02x}{:02x}{:02x}", self.r, self.g, self.b)
    }

    /// Format as `r, g, b` for Qt color scheme files.
    fn to_rgb_csv(&self) -> String {
        format!("{}, {}, {}", self.r, self.g, self.b)
    }

}

/// The extracted psh color palette.
///
/// Colors are stored by their short name (e.g. `"base"`, `"text"`, `"blue"`),
/// without the `psh-` prefix.
#[derive(Debug, Clone)]
pub struct Palette {
    pub colors: BTreeMap<String, Color>,
}

impl Palette {
    /// Parse a psh theme CSS string, extracting all `@define-color psh-<name> #hex;` lines.
    pub fn from_css(css: &str) -> Self {
        let mut colors = BTreeMap::new();
        for line in css.lines() {
            let line = line.trim();
            if let Some(rest) = line.strip_prefix("@define-color psh-") {
                // rest looks like: "base #1e1e2e;"
                let rest = rest.trim_end_matches(';').trim();
                if let Some((name, hex)) = rest.split_once(char::is_whitespace) {
                    let hex = hex.trim();
                    if let Some(color) = Color::from_hex(hex) {
                        colors.insert(name.to_owned(), color);
                    }
                }
            }
        }
        Self { colors }
    }

    /// Parse palette from a CSS file on disk.
    pub fn from_css_file(path: &Path) -> Result<Self> {
        let css = std::fs::read_to_string(path)?;
        Ok(Self::from_css(&css))
    }

    /// Parse palette from the bundled default theme.
    pub fn from_default() -> Self {
        Self::from_css(include_str!("../../../assets/themes/default.css"))
    }

    /// Load the palette for the named theme, falling back to the bundled default.
    pub fn load(theme_name: &str) -> Self {
        let search_paths = theme_search_paths(theme_name);
        for path in &search_paths {
            if path.exists() {
                match Self::from_css_file(path) {
                    Ok(p) if !p.colors.is_empty() => return p,
                    Ok(_) => continue,
                    Err(_) => continue,
                }
            }
        }
        Self::from_default()
    }

    /// Get a color by short name (e.g. `"base"`, `"text"`).
    pub fn get(&self, name: &str) -> Option<&Color> {
        self.colors.get(name)
    }

    // Convenience accessors with fallbacks for role-based mapping.
    fn base(&self) -> &Color {
        self.get("base").unwrap_or(&Color { r: 0x1e, g: 0x1e, b: 0x2e })
    }
    fn mantle(&self) -> &Color {
        self.get("mantle").unwrap_or(self.base())
    }
    fn crust(&self) -> &Color {
        self.get("crust").unwrap_or(self.base())
    }
    fn surface0(&self) -> &Color {
        self.get("surface0").unwrap_or(&Color { r: 0x31, g: 0x32, b: 0x44 })
    }
    fn surface1(&self) -> &Color {
        self.get("surface1").unwrap_or(self.surface0())
    }
    fn surface2(&self) -> &Color {
        self.get("surface2").unwrap_or(self.surface1())
    }
    fn text(&self) -> &Color {
        self.get("text").unwrap_or(&Color { r: 0xcd, g: 0xd6, b: 0xf4 })
    }
    fn subtext0(&self) -> &Color {
        self.get("subtext0").unwrap_or(self.text())
    }
    fn blue(&self) -> &Color {
        self.get("blue").unwrap_or(&Color { r: 0x89, g: 0xb4, b: 0xfa })
    }
    fn red(&self) -> &Color {
        self.get("red").unwrap_or(&Color { r: 0xf3, g: 0x8b, b: 0xa8 })
    }
    fn green(&self) -> &Color {
        self.get("green").unwrap_or(&Color { r: 0xa6, g: 0xe3, b: 0xa1 })
    }
    fn yellow(&self) -> &Color {
        self.get("yellow").unwrap_or(&Color { r: 0xf9, g: 0xe2, b: 0xaf })
    }
    fn lavender(&self) -> &Color {
        self.get("lavender").unwrap_or(self.blue())
    }

    /// Generate GTK3/GTK4 CSS color overrides.
    ///
    /// These layer on top of the existing GTK theme (e.g. Adwaita) and remap
    /// key color variables to match the psh palette.
    pub fn generate_gtk_css(&self) -> String {
        let base = self.base();
        let mantle = self.mantle();
        let crust = self.crust();
        let surface0 = self.surface0();
        let surface1 = self.surface1();
        let surface2 = self.surface2();
        let text = self.text();
        let subtext0 = self.subtext0();
        let red = self.red();
        let green = self.green();
        let yellow = self.yellow();
        let lavender = self.lavender();

        let mut css = String::with_capacity(2048);
        let _ = writeln!(css, "/* Generated by psh — do not edit manually */");
        let _ = writeln!(css, "/* Re-generate with: psh theme apply */");
        let _ = writeln!(css);

        // Adwaita/libadwaita named colors (GTK4)
        let _ = writeln!(css, "@define-color window_bg_color {};", base.to_hex());
        let _ = writeln!(css, "@define-color window_fg_color {};", text.to_hex());
        let _ = writeln!(css, "@define-color view_bg_color {};", mantle.to_hex());
        let _ = writeln!(css, "@define-color view_fg_color {};", text.to_hex());
        let _ = writeln!(css, "@define-color headerbar_bg_color {};", crust.to_hex());
        let _ = writeln!(css, "@define-color headerbar_fg_color {};", text.to_hex());
        let _ = writeln!(css, "@define-color headerbar_backdrop_color {};", mantle.to_hex());
        let _ = writeln!(css, "@define-color card_bg_color {};", surface0.to_hex());
        let _ = writeln!(css, "@define-color card_fg_color {};", text.to_hex());
        let _ = writeln!(css, "@define-color dialog_bg_color {};", surface0.to_hex());
        let _ = writeln!(css, "@define-color dialog_fg_color {};", text.to_hex());
        let _ = writeln!(css, "@define-color popover_bg_color {};", surface0.to_hex());
        let _ = writeln!(css, "@define-color popover_fg_color {};", text.to_hex());
        let _ = writeln!(css, "@define-color sidebar_bg_color {};", mantle.to_hex());
        let _ = writeln!(css, "@define-color sidebar_fg_color {};", text.to_hex());
        let _ = writeln!(css, "@define-color sidebar_backdrop_color {};", crust.to_hex());
        let _ = writeln!(css, "@define-color accent_bg_color {};", lavender.to_hex());
        let _ = writeln!(css, "@define-color accent_fg_color {};", crust.to_hex());
        let _ = writeln!(css, "@define-color accent_color {};", lavender.to_hex());
        let _ = writeln!(css, "@define-color destructive_bg_color {};", red.to_hex());
        let _ = writeln!(css, "@define-color destructive_fg_color {};", crust.to_hex());
        let _ = writeln!(css, "@define-color destructive_color {};", red.to_hex());
        let _ = writeln!(css, "@define-color success_bg_color {};", green.to_hex());
        let _ = writeln!(css, "@define-color success_fg_color {};", crust.to_hex());
        let _ = writeln!(css, "@define-color success_color {};", green.to_hex());
        let _ = writeln!(css, "@define-color warning_bg_color {};", yellow.to_hex());
        let _ = writeln!(css, "@define-color warning_fg_color {};", crust.to_hex());
        let _ = writeln!(css, "@define-color warning_color {};", yellow.to_hex());
        let _ = writeln!(css, "@define-color error_bg_color {};", red.to_hex());
        let _ = writeln!(css, "@define-color error_fg_color {};", crust.to_hex());
        let _ = writeln!(css, "@define-color error_color {};", red.to_hex());
        let _ = writeln!(css);

        // GTK3 theme_* colors
        let _ = writeln!(css, "@define-color theme_bg_color {};", base.to_hex());
        let _ = writeln!(css, "@define-color theme_fg_color {};", text.to_hex());
        let _ = writeln!(css, "@define-color theme_base_color {};", mantle.to_hex());
        let _ = writeln!(css, "@define-color theme_text_color {};", text.to_hex());
        let _ = writeln!(css, "@define-color theme_selected_bg_color {};", lavender.to_hex());
        let _ = writeln!(css, "@define-color theme_selected_fg_color {};", crust.to_hex());
        let _ = writeln!(css, "@define-color theme_unfocused_bg_color {};", base.to_hex());
        let _ = writeln!(css, "@define-color theme_unfocused_fg_color {};", subtext0.to_hex());
        let _ = writeln!(css, "@define-color insensitive_bg_color {};", surface0.to_hex());
        let _ = writeln!(css, "@define-color insensitive_fg_color {};", surface2.to_hex());
        let _ = writeln!(css, "@define-color borders {};", surface1.to_hex());
        let _ = writeln!(css, "@define-color unfocused_borders {};", surface0.to_hex());
        let _ = writeln!(css);

        // Targeted widget overrides
        let _ = writeln!(css, "tooltip {{");
        let _ = writeln!(css, "    background-color: {};", surface0.to_hex());
        let _ = writeln!(css, "    color: {};", text.to_hex());
        let _ = writeln!(css, "}}");

        css
    }

    /// Generate a Qt5ct/Qt6ct color scheme `.conf` file.
    ///
    /// Maps the psh palette to the QPalette color roles for both Active and
    /// Inactive groups. The Disabled group uses dimmed variants.
    pub fn generate_qt_color_scheme(&self) -> String {
        let base = self.base();
        let mantle = self.mantle();
        let crust = self.crust();
        let surface0 = self.surface0();
        let surface1 = self.surface1();
        let surface2 = self.surface2();
        let text = self.text();
        let subtext0 = self.subtext0();
        let blue = self.blue();
        let red = self.red();
        let lavender = self.lavender();

        let mut conf = String::with_capacity(2048);
        let _ = writeln!(conf, "# Generated by psh — do not edit manually");
        let _ = writeln!(conf, "# Re-generate with: psh theme apply");
        let _ = writeln!(conf, "[ColorScheme]");
        let _ = writeln!(conf, "active_colors={}", [
            text.to_rgb_csv(),           // WindowText
            base.to_rgb_csv(),           // Button
            surface1.to_rgb_csv(),       // Light
            surface0.to_rgb_csv(),       // Midlight
            crust.to_rgb_csv(),          // Dark
            surface1.to_rgb_csv(),       // Mid
            text.to_rgb_csv(),           // Text
            text.to_rgb_csv(),           // BrightText
            text.to_rgb_csv(),           // ButtonText
            mantle.to_rgb_csv(),         // Base
            base.to_rgb_csv(),           // Window
            surface1.to_rgb_csv(),       // Shadow
            lavender.to_rgb_csv(),       // Highlight
            crust.to_rgb_csv(),          // HighlightedText
            blue.to_rgb_csv(),           // Link
            red.to_rgb_csv(),            // LinkVisited
            mantle.to_rgb_csv(),         // AlternateBase
            base.to_rgb_csv(),           // ToolTipBase (default)
            text.to_rgb_csv(),           // ToolTipText (default)
            text.to_rgb_csv(),           // PlaceholderText
        ].join(", "));

        let _ = writeln!(conf, "inactive_colors={}", [
            text.to_rgb_csv(),
            base.to_rgb_csv(),
            surface1.to_rgb_csv(),
            surface0.to_rgb_csv(),
            crust.to_rgb_csv(),
            surface1.to_rgb_csv(),
            text.to_rgb_csv(),
            text.to_rgb_csv(),
            text.to_rgb_csv(),
            mantle.to_rgb_csv(),
            base.to_rgb_csv(),
            surface1.to_rgb_csv(),
            lavender.to_rgb_csv(),
            crust.to_rgb_csv(),
            blue.to_rgb_csv(),
            red.to_rgb_csv(),
            mantle.to_rgb_csv(),
            base.to_rgb_csv(),
            text.to_rgb_csv(),
            text.to_rgb_csv(),
        ].join(", "));

        let disabled_text = surface2;
        let _ = writeln!(conf, "disabled_colors={}", [
            disabled_text.to_rgb_csv(),  // WindowText
            base.to_rgb_csv(),           // Button
            surface1.to_rgb_csv(),       // Light
            surface0.to_rgb_csv(),       // Midlight
            crust.to_rgb_csv(),          // Dark
            surface1.to_rgb_csv(),       // Mid
            disabled_text.to_rgb_csv(),  // Text
            disabled_text.to_rgb_csv(),  // BrightText
            disabled_text.to_rgb_csv(),  // ButtonText
            base.to_rgb_csv(),           // Base
            base.to_rgb_csv(),           // Window
            surface1.to_rgb_csv(),       // Shadow
            surface2.to_rgb_csv(),       // Highlight
            subtext0.to_rgb_csv(),       // HighlightedText
            surface2.to_rgb_csv(),       // Link
            surface2.to_rgb_csv(),       // LinkVisited
            base.to_rgb_csv(),           // AlternateBase
            base.to_rgb_csv(),           // ToolTipBase
            disabled_text.to_rgb_csv(),  // ToolTipText
            disabled_text.to_rgb_csv(),  // PlaceholderText
        ].join(", "));

        conf
    }

    /// Write GTK3, GTK4, and Qt color scheme files to their XDG config locations.
    ///
    /// Creates directories as needed. Returns the list of paths written.
    pub fn apply(&self) -> Result<Vec<PathBuf>> {
        let dirs = directories::BaseDirs::new()
            .ok_or_else(|| crate::PshError::Config("cannot determine home directory".into()))?;
        let config = dirs.config_dir();

        let mut written = Vec::new();

        // GTK4: ~/.config/gtk-4.0/gtk.css
        let gtk4_dir = config.join("gtk-4.0");
        std::fs::create_dir_all(&gtk4_dir)?;
        let gtk4_path = gtk4_dir.join("gtk.css");
        std::fs::write(&gtk4_path, self.generate_gtk_css())?;
        info!("wrote GTK4 overrides: {}", gtk4_path.display());
        written.push(gtk4_path);

        // GTK3: ~/.config/gtk-3.0/gtk.css
        let gtk3_dir = config.join("gtk-3.0");
        std::fs::create_dir_all(&gtk3_dir)?;
        let gtk3_path = gtk3_dir.join("gtk.css");
        std::fs::write(&gtk3_path, self.generate_gtk_css())?;
        info!("wrote GTK3 overrides: {}", gtk3_path.display());
        written.push(gtk3_path);

        // Qt5ct: ~/.config/qt5ct/colors/psh.conf
        let qt5_dir = config.join("qt5ct").join("colors");
        std::fs::create_dir_all(&qt5_dir)?;
        let qt5_path = qt5_dir.join("psh.conf");
        std::fs::write(&qt5_path, self.generate_qt_color_scheme())?;
        info!("wrote Qt5ct color scheme: {}", qt5_path.display());
        written.push(qt5_path);

        // Qt6ct: ~/.config/qt6ct/colors/psh.conf
        let qt6_dir = config.join("qt6ct").join("colors");
        std::fs::create_dir_all(&qt6_dir)?;
        let qt6_path = qt6_dir.join("psh.conf");
        std::fs::write(&qt6_path, self.generate_qt_color_scheme())?;
        info!("wrote Qt6ct color scheme: {}", qt6_path.display());
        written.push(qt6_path);

        Ok(written)
    }
}

/// Theme CSS search paths (duplicated from theme.rs to avoid gtk feature gate).
fn theme_search_paths(name: &str) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(dirs) = directories::BaseDirs::new() {
        paths.push(
            dirs.config_dir()
                .join("psh")
                .join("themes")
                .join(format!("{name}.css")),
        );
    }
    paths.push(PathBuf::from(format!("/usr/share/psh/themes/{name}.css")));
    paths.push(PathBuf::from(format!(
        "/usr/local/share/psh/themes/{name}.css"
    )));
    paths
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_default_palette() {
        let palette = Palette::from_default();
        assert_eq!(palette.colors.len(), 18);
        assert_eq!(palette.get("base"), Some(&Color { r: 0x1e, g: 0x1e, b: 0x2e }));
        assert_eq!(palette.get("text"), Some(&Color { r: 0xcd, g: 0xd6, b: 0xf4 }));
        assert_eq!(palette.get("blue"), Some(&Color { r: 0x89, g: 0xb4, b: 0xfa }));
    }

    #[test]
    fn parse_minimal_css() {
        let css = r#"
            @define-color psh-bg #112233;
            @define-color psh-fg #aabbcc;
            .some-class { color: red; }
        "#;
        let palette = Palette::from_css(css);
        assert_eq!(palette.colors.len(), 2);
        assert_eq!(palette.get("bg"), Some(&Color { r: 0x11, g: 0x22, b: 0x33 }));
        assert_eq!(palette.get("fg"), Some(&Color { r: 0xaa, g: 0xbb, b: 0xcc }));
    }

    #[test]
    fn color_hex_roundtrip() {
        let color = Color::from_hex("#89b4fa").unwrap();
        assert_eq!(color.to_hex(), "#89b4fa");
    }

    #[test]
    fn color_from_hex_no_hash() {
        let color = Color::from_hex("1e1e2e").unwrap();
        assert_eq!(color, Color { r: 0x1e, g: 0x1e, b: 0x2e });
    }

    #[test]
    fn color_from_hex_invalid() {
        assert!(Color::from_hex("#abc").is_none());
        assert!(Color::from_hex("").is_none());
        assert!(Color::from_hex("#zzzzzz").is_none());
    }

    #[test]
    fn gtk_css_contains_key_variables() {
        let palette = Palette::from_default();
        let css = palette.generate_gtk_css();
        assert!(css.contains("@define-color window_bg_color #1e1e2e;"));
        assert!(css.contains("@define-color window_fg_color #cdd6f4;"));
        assert!(css.contains("@define-color accent_color #b4befe;"));
        assert!(css.contains("@define-color theme_bg_color #1e1e2e;"));
        assert!(css.contains("@define-color theme_selected_bg_color #b4befe;"));
        assert!(css.contains("Generated by psh"));
    }

    #[test]
    fn qt_color_scheme_format() {
        let palette = Palette::from_default();
        let conf = palette.generate_qt_color_scheme();
        assert!(conf.contains("[ColorScheme]"));
        assert!(conf.contains("active_colors="));
        assert!(conf.contains("inactive_colors="));
        assert!(conf.contains("disabled_colors="));
        // Verify text color (205, 214, 244) appears in active colors
        assert!(conf.contains("205, 214, 244"));
    }

    #[test]
    fn empty_css_gives_empty_palette() {
        let palette = Palette::from_css("");
        assert!(palette.colors.is_empty());
    }

    #[test]
    fn non_psh_define_colors_ignored() {
        let css = "@define-color accent_bg_color #ff0000;\n@define-color psh-test #112233;";
        let palette = Palette::from_css(css);
        assert_eq!(palette.colors.len(), 1);
        assert!(palette.get("test").is_some());
    }
}
