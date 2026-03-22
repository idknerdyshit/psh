use std::path::{Path, PathBuf};

use gtk4::CssProvider;
use gtk4::gdk::Display;
use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use tokio::sync::broadcast;
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

/// Watch a named theme's CSS file for changes. Returns `None` if the theme
/// is bundled (no file to watch) or the directory doesn't exist.
///
/// The returned [`broadcast::Sender`] fires `()` whenever the CSS file is
/// modified. Callers should re-apply the theme on the GTK main thread.
/// The [`RecommendedWatcher`] must be kept alive for the watch to remain active.
pub fn watch(name: &str) -> Option<(broadcast::Sender<()>, RecommendedWatcher)> {
    let path = find_theme(name)?;
    let parent = path.parent().filter(|p| p.exists())?;

    let (tx, _) = broadcast::channel(4);
    let tx_clone = tx.clone();
    let path_clone = path.clone();

    let mut watcher = RecommendedWatcher::new(
        move |res: std::result::Result<notify::Event, notify::Error>| match res {
            Ok(event)
                if matches!(event.kind, EventKind::Modify(_) | EventKind::Create(_))
                    && event.paths.iter().any(|p| p == &path_clone) =>
            {
                info!("theme CSS changed: {}", path_clone.display());
                let _ = tx_clone.send(());
            }
            Ok(_) => {}
            Err(e) => warn!("theme watch error: {e}"),
        },
        notify::Config::default(),
    )
    .ok()?;

    watcher.watch(parent, RecursiveMode::NonRecursive).ok()?;

    info!("watching theme file: {}", path.display());
    Some((tx, watcher))
}
