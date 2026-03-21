use std::path::{Path, PathBuf};

use gtk4::gdk::Display;
use gtk4::CssProvider;
use tracing::{info, warn};

/// Search paths for theme CSS files, in priority order.
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

/// Find a theme CSS file by name.
pub fn find_theme(name: &str) -> Option<PathBuf> {
    theme_search_paths(name).into_iter().find(|p| p.exists())
}

/// Load and apply a CSS theme file to the default GDK display.
pub fn apply_css(path: &Path) {
    let provider = CssProvider::new();
    let Some(path_str) = path.to_str() else {
        warn!("theme path contains invalid UTF-8: {}", path.display());
        return;
    };
    provider.load_from_path(path_str);

    let Some(display) = Display::default() else {
        warn!("no default display, cannot apply CSS theme");
        return;
    };

    gtk4::style_context_add_provider_for_display(
        &display,
        &provider,
        gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
    info!("applied theme from {}", path.display());
}

/// Load and apply a named theme. Falls back to bundled default CSS string.
pub fn apply_theme(name: &str) {
    if let Some(path) = find_theme(name) {
        apply_css(&path);
    } else {
        info!("theme '{name}' not found on disk, applying bundled default");
        apply_default_css();
    }
}

/// Apply the bundled default CSS.
pub fn apply_default_css() {
    let provider = CssProvider::new();
    provider.load_from_data(include_str!("../../../assets/themes/default.css"));

    let Some(display) = Display::default() else {
        warn!("no default display, cannot apply CSS theme");
        return;
    };

    gtk4::style_context_add_provider_for_display(
        &display,
        &provider,
        gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
}
