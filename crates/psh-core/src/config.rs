use std::path::{Path, PathBuf};

use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use serde::Deserialize;
use tokio::sync::broadcast;
use tracing::{info, warn};

use crate::{PshError, Result};

/// Top-level psh configuration.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct PshConfig {
    pub theme: ThemeConfig,
    pub bar: BarConfig,
    pub notify: NotifyConfig,
    pub polkit: PolkitConfig,
    pub launch: LaunchConfig,
    pub wall: WallConfig,
    pub lock: LockConfig,
    pub idle: IdleConfig,
    pub clip: ClipConfig,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct ThemeConfig {
    pub name: String,
}

impl Default for ThemeConfig {
    fn default() -> Self {
        Self {
            name: "default".into(),
        }
    }
}

/// Configuration for the status bar.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct BarConfig {
    /// Bar position: top or bottom of the screen.
    pub position: BarPosition,
    /// Bar height in pixels (default: 32).
    pub height: Option<u32>,
    /// Module names for the left section.
    pub modules_left: Vec<String>,
    /// Module names for the center section.
    pub modules_center: Vec<String>,
    /// Module names for the right section.
    pub modules_right: Vec<String>,
    /// Show workspaces from all outputs, not just the current one.
    pub show_all_workspaces: bool,
    /// Maximum window title length before truncation (default: 50).
    pub max_title_length: Option<usize>,
    /// Volume adjustment step per scroll event in percent (default: 5).
    pub volume_step: Option<u32>,
    /// Sysfs battery device name (default: "BAT0").
    pub battery_device: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BarPosition {
    #[default]
    Top,
    Bottom,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct NotifyConfig {
    pub max_visible: u32,
    pub default_timeout_ms: u64,
    pub width: u32,
    pub gap: u32,
    pub icon_size: u32,
}

impl Default for NotifyConfig {
    fn default() -> Self {
        Self {
            max_visible: 5,
            default_timeout_ms: 5000,
            width: 380,
            gap: 10,
            icon_size: 48,
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct PolkitConfig {}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct LaunchConfig {
    pub terminal: Option<String>,
    pub max_results: Option<usize>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct WallConfig {
    pub path: Option<PathBuf>,
    pub mode: WallMode,
}

impl Default for WallConfig {
    fn default() -> Self {
        Self {
            path: None,
            mode: WallMode::Fill,
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WallMode {
    #[default]
    Fill,
    Fit,
    Center,
    Stretch,
    Tile,
}

/// Configuration for the screen locker.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct LockConfig {
    /// Show a clock on the lock screen.
    pub show_clock: bool,
    /// Clock format string (strftime syntax).
    pub clock_format: String,
    /// Date format string (strftime syntax).
    pub date_format: String,
    /// Show the current username on the lock screen.
    pub show_username: bool,
    /// Background color as a hex string (e.g. "#1e1e2e").
    pub background_color: String,
    /// Optional path to a background image.
    pub background_image: Option<String>,
    /// Base font size in pixels.
    pub font_size: f32,
    /// Color for password indicator dots as a hex string.
    pub password_dot_color: String,
    /// Color for error messages as a hex string.
    pub error_color: String,
    /// Auto-cancel timeout in seconds (0 = disabled).
    pub timeout_secs: u64,
    /// Placeholder for future blur support.
    pub blur_background: bool,
}

impl Default for LockConfig {
    fn default() -> Self {
        Self {
            show_clock: true,
            clock_format: "%H:%M".into(),
            date_format: "%A, %B %d".into(),
            show_username: true,
            background_color: "#1e1e2e".into(),
            background_image: None,
            font_size: 24.0,
            password_dot_color: "#cdd6f4".into(),
            error_color: "#f38ba8".into(),
            timeout_secs: 0,
            blur_background: false,
        }
    }
}

/// Configuration for the idle monitor daemon.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct IdleConfig {
    /// Idle timeout in seconds before locking (0 = disabled).
    pub idle_timeout_secs: u64,
    /// Lock the screen on system sleep/suspend.
    pub lock_on_sleep: bool,
    /// Command to run to lock the screen.
    pub lock_command: String,
}

impl Default for IdleConfig {
    fn default() -> Self {
        Self {
            idle_timeout_secs: 300,
            lock_on_sleep: true,
            lock_command: "psh-lock".into(),
        }
    }
}

/// Configuration for the clipboard manager.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct ClipConfig {
    pub max_history: usize,
    pub persist: bool,
    pub image_support: bool,
    pub max_image_bytes: usize,
}

impl Default for ClipConfig {
    fn default() -> Self {
        Self {
            max_history: 100,
            persist: true,
            image_support: true,
            max_image_bytes: 10_000_000,
        }
    }
}

/// Expands a leading `~` in a path to the user's home directory.
/// Returns the path unchanged if it doesn't start with `~` or if `$HOME` is not set.
pub fn expand_tilde(path: &Path) -> PathBuf {
    if let (Ok(stripped), Ok(home)) = (path.strip_prefix("~"), std::env::var("HOME")) {
        return PathBuf::from(home).join(stripped);
    }
    path.to_owned()
}

/// Returns the path to `$XDG_CONFIG_HOME/psh/psh.toml`.
pub fn config_path() -> PathBuf {
    directories::BaseDirs::new()
        .map(|dirs| dirs.config_dir().join("psh").join("psh.toml"))
        .unwrap_or_else(|| PathBuf::from("/etc/psh/psh.toml"))
}

/// Load config from the default path. Returns defaults if file doesn't exist.
pub fn load() -> Result<PshConfig> {
    load_from(&config_path())
}

/// Load config from a specific path. Returns defaults if file doesn't exist.
pub fn load_from(path: &Path) -> Result<PshConfig> {
    if !path.exists() {
        info!("no config file at {}, using defaults", path.display());
        return Ok(PshConfig::default());
    }

    let contents = std::fs::read_to_string(path).map_err(PshError::Io)?;
    let mut config: PshConfig =
        toml::from_str(&contents).map_err(|source| PshError::ConfigParse {
            path: path.to_owned(),
            source,
        })?;

    // Expand tilde in paths that users are likely to write with ~/
    if let Some(ref p) = config.wall.path {
        config.wall.path = Some(expand_tilde(p));
    }

    Ok(config)
}

/// Watch the config file for changes, sending notifications on the returned channel.
pub fn watch(path: PathBuf) -> Result<(broadcast::Sender<PshConfig>, RecommendedWatcher)> {
    let (tx, _) = broadcast::channel(4);
    let tx_clone = tx.clone();
    let path_clone = path.clone();

    let mut watcher =
        RecommendedWatcher::new(
            move |res: std::result::Result<notify::Event, notify::Error>| match res {
                Ok(event)
                    if matches!(
                        event.kind,
                        EventKind::Modify(_) | EventKind::Create(_)
                    ) =>
                {
                    match load_from(&path_clone) {
                        Ok(config) => {
                            info!("config reloaded");
                            let _ = tx_clone.send(config);
                        }
                        Err(e) => warn!("failed to reload config: {e}"),
                    }
                }
                Ok(_) => {}
                Err(e) => warn!("config watch error: {e}"),
            },
            notify::Config::default(),
        )?;

    if let Some(parent) = path.parent().filter(|p| p.exists()) {
        watcher.watch(parent, RecursiveMode::NonRecursive)?;
    }

    Ok((tx, watcher))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_parses() {
        let config: PshConfig = toml::from_str("").unwrap();
        assert_eq!(config.theme.name, "default");
        assert_eq!(config.notify.max_visible, 5);
    }

    #[test]
    fn partial_config_parses() {
        let toml = r#"
            [wall]
            path = "/home/user/wallpaper.png"
            mode = "fit"

            [bar]
            position = "bottom"
        "#;
        let config: PshConfig = toml::from_str(toml).unwrap();
        assert!(config.wall.path.is_some());
        assert!(matches!(config.wall.mode, WallMode::Fit));
        assert!(matches!(config.bar.position, BarPosition::Bottom));
    }

    #[test]
    fn expand_tilde_expands_home() {
        let home = std::env::var("HOME").unwrap();
        let result = expand_tilde(Path::new("~/wallpaper.png"));
        assert_eq!(result, PathBuf::from(home).join("wallpaper.png"));
    }

    #[test]
    fn expand_tilde_leaves_absolute_unchanged() {
        let path = Path::new("/usr/share/wallpaper.png");
        assert_eq!(expand_tilde(path), path);
    }

    #[test]
    fn missing_file_returns_defaults() {
        let config = load_from(Path::new("/nonexistent/psh.toml")).unwrap();
        assert_eq!(config.theme.name, "default");
    }

    #[test]
    fn bar_config_new_fields_default() {
        let config: PshConfig = toml::from_str("").unwrap();
        assert!(!config.bar.show_all_workspaces);
        assert_eq!(config.bar.max_title_length, None);
        assert_eq!(config.bar.volume_step, None);
        assert_eq!(config.bar.battery_device, None);
    }

    #[test]
    fn bar_config_new_fields_parse() {
        let toml = r#"
            [bar]
            show_all_workspaces = true
            max_title_length = 30
            volume_step = 10
            battery_device = "BAT1"
            modules_left = ["workspaces"]
            modules_center = ["clock"]
            modules_right = ["battery", "volume"]
        "#;
        let config: PshConfig = toml::from_str(toml).unwrap();
        assert!(config.bar.show_all_workspaces);
        assert_eq!(config.bar.max_title_length, Some(30));
        assert_eq!(config.bar.volume_step, Some(10));
        assert_eq!(config.bar.battery_device.as_deref(), Some("BAT1"));
        assert_eq!(config.bar.modules_left, vec!["workspaces"]);
        assert_eq!(config.bar.modules_center, vec!["clock"]);
        assert_eq!(config.bar.modules_right, vec!["battery", "volume"]);
    }

    #[test]
    fn lock_config_defaults() {
        let config: PshConfig = toml::from_str("").unwrap();
        assert!(config.lock.show_clock);
        assert_eq!(config.lock.clock_format, "%H:%M");
        assert_eq!(config.lock.date_format, "%A, %B %d");
        assert!(config.lock.show_username);
        assert_eq!(config.lock.background_color, "#1e1e2e");
        assert!(config.lock.background_image.is_none());
        assert_eq!(config.lock.font_size, 24.0);
        assert_eq!(config.lock.password_dot_color, "#cdd6f4");
        assert_eq!(config.lock.error_color, "#f38ba8");
        assert_eq!(config.lock.timeout_secs, 0);
        assert!(!config.lock.blur_background);
    }

    #[test]
    fn lock_config_parses() {
        let toml = r##"
            [lock]
            show_clock = false
            clock_format = "%I:%M %p"
            background_color = "#000000"
            font_size = 32.0
            timeout_secs = 30
        "##;
        let config: PshConfig = toml::from_str(toml).unwrap();
        assert!(!config.lock.show_clock);
        assert_eq!(config.lock.clock_format, "%I:%M %p");
        assert_eq!(config.lock.background_color, "#000000");
        assert_eq!(config.lock.font_size, 32.0);
        assert_eq!(config.lock.timeout_secs, 30);
    }

    #[test]
    fn idle_config_defaults() {
        let config: PshConfig = toml::from_str("").unwrap();
        assert_eq!(config.idle.idle_timeout_secs, 300);
        assert!(config.idle.lock_on_sleep);
        assert_eq!(config.idle.lock_command, "psh-lock");
    }

    #[test]
    fn idle_config_parses() {
        let toml = r#"
            [idle]
            idle_timeout_secs = 600
            lock_on_sleep = false
            lock_command = "swaylock"
        "#;
        let config: PshConfig = toml::from_str(toml).unwrap();
        assert_eq!(config.idle.idle_timeout_secs, 600);
        assert!(!config.idle.lock_on_sleep);
        assert_eq!(config.idle.lock_command, "swaylock");
    }
}
