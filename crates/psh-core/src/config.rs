use std::collections::HashMap;
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
    /// Claude.ai session key for the claude bar module.
    /// Falls back to `CLAUDE_SESSION_KEY` env var if not set.
    pub claude_session_key: Option<String>,
    /// Claude module display format: "percent" (default) or "both".
    pub claude_display: Option<String>,
    /// Claude module poll interval in seconds (default: 120).
    pub claude_poll_interval: Option<u64>,
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

/// Per-output wallpaper override. Unset fields inherit from the top-level `[wall]` section.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct OutputWallConfig {
    /// Image path (or directory for slideshow) for this specific output.
    pub path: Option<PathBuf>,
    /// Wallpaper scaling mode for this output.
    pub mode: Option<WallMode>,
    /// Slideshow interval in seconds for this output (only when path is a directory).
    pub interval: Option<u64>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct WallConfig {
    /// Default wallpaper image path, or directory path for slideshow mode.
    pub path: Option<PathBuf>,
    /// Default wallpaper scaling mode.
    pub mode: WallMode,
    /// Slideshow interval in seconds (default: 300). Only applies when path is a directory.
    pub interval: u64,
    /// Per-output wallpaper overrides, keyed by Wayland output name (e.g. "DP-1").
    pub outputs: HashMap<String, OutputWallConfig>,
}

impl Default for WallConfig {
    fn default() -> Self {
        Self {
            path: None,
            mode: WallMode::Fill,
            interval: 300,
            outputs: HashMap::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, Deserialize)]
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
    /// Base font size in pixels.
    pub font_size: f32,
    /// Color for password indicator dots as a hex string.
    pub password_dot_color: String,
    /// Color for error messages as a hex string.
    pub error_color: String,
    /// Optional path to a background image.
    pub background_image: Option<String>,
    /// Inactivity timeout in seconds — clears password and shows blank screen (0 = disabled).
    pub blank_timeout_secs: u64,
    /// Inactivity timeout in seconds — powers off monitors via DPMS (0 = disabled).
    /// Must be >= blank_timeout_secs. Only takes effect if blank_timeout_secs > 0.
    pub dpms_timeout_secs: u64,
    /// Apply gaussian blur to the background image.
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
            font_size: 24.0,
            password_dot_color: "#cdd6f4".into(),
            error_color: "#f38ba8".into(),
            background_image: None,
            blank_timeout_secs: 0,
            dpms_timeout_secs: 0,
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
/// Returns the path unchanged if it doesn't start with `~` or if home dir cannot be determined.
pub fn expand_tilde(path: &Path) -> PathBuf {
    if let (Ok(stripped), Some(dirs)) = (path.strip_prefix("~"), directories::BaseDirs::new()) {
        return dirs.home_dir().join(stripped);
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
    let contents = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            info!("no config file at {}, using defaults", path.display());
            return Ok(PshConfig::default());
        }
        Err(e) => return Err(PshError::Io(e)),
    };
    let deserializer = toml::Deserializer::new(&contents);
    let mut unknown_keys = Vec::new();
    let mut config: PshConfig = serde_ignored::deserialize(deserializer, |path| {
        unknown_keys.push(path.to_string());
    })
    .map_err(|source| PshError::ConfigParse {
        path: path.to_owned(),
        source,
    })?;
    for key in &unknown_keys {
        warn!("unknown config key: {key}");
    }

    // Expand tilde in paths that users are likely to write with ~/
    if let Some(ref p) = config.wall.path {
        config.wall.path = Some(expand_tilde(p));
    }
    for output_cfg in config.wall.outputs.values_mut() {
        if let Some(ref p) = output_cfg.path {
            output_cfg.path = Some(expand_tilde(p));
        }
    }
    Ok(config)
}

/// Watch the config file for changes, sending notifications on the returned channel.
pub fn watch(path: PathBuf) -> Result<(broadcast::Sender<PshConfig>, RecommendedWatcher)> {
    let (tx, _) = broadcast::channel(4);
    let tx_clone = tx.clone();
    let path_clone = path.clone();

    let mut watcher = RecommendedWatcher::new(
        move |res: std::result::Result<notify::Event, notify::Error>| match res {
            Ok(event)
                if matches!(event.kind, EventKind::Modify(_) | EventKind::Create(_))
                    && event.paths.iter().any(|p| p.ends_with("psh.toml")) =>
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

    match path.parent().filter(|p| p.exists()) {
        Some(parent) => {
            watcher.watch(parent, RecursiveMode::NonRecursive)?;
        }
        None => {
            warn!(
                "config directory {} does not exist, config hot-reload disabled",
                path.parent()
                    .map(|p| p.display().to_string())
                    .unwrap_or_default()
            );
        }
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
        let home = directories::BaseDirs::new().unwrap().home_dir().to_owned();
        let result = expand_tilde(Path::new("~/wallpaper.png"));
        assert_eq!(result, home.join("wallpaper.png"));
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
        assert_eq!(config.lock.font_size, 24.0);
        assert_eq!(config.lock.password_dot_color, "#cdd6f4");
        assert_eq!(config.lock.error_color, "#f38ba8");
    }

    #[test]
    fn lock_config_parses() {
        let toml = r##"
            [lock]
            show_clock = false
            clock_format = "%I:%M %p"
            background_color = "#000000"
            font_size = 32.0
        "##;
        let config: PshConfig = toml::from_str(toml).unwrap();
        assert!(!config.lock.show_clock);
        assert_eq!(config.lock.clock_format, "%I:%M %p");
        assert_eq!(config.lock.background_color, "#000000");
        assert_eq!(config.lock.font_size, 32.0);
    }

    #[test]
    fn idle_config_defaults() {
        let config: PshConfig = toml::from_str("").unwrap();
        assert_eq!(config.idle.idle_timeout_secs, 300);
        assert!(config.idle.lock_on_sleep);
        assert_eq!(config.idle.lock_command, "psh-lock");
    }

    #[test]
    fn unknown_top_level_key_still_parses() {
        let toml = r#"
            bogus_key = "hello"
            [bar]
            position = "top"
        "#;
        let config: PshConfig = toml::from_str(toml).unwrap();
        assert!(matches!(config.bar.position, BarPosition::Top));
    }

    #[test]
    fn unknown_nested_key_still_parses() {
        let toml = r#"
            [notify]
            max_visible = 3
            nonexistent_field = true
        "#;
        let config: PshConfig = toml::from_str(toml).unwrap();
        assert_eq!(config.notify.max_visible, 3);
    }

    #[test]
    fn serde_ignored_detects_unknown_keys() {
        let input = r#"
            bogus = "hi"
            [notify]
            max_visible = 3
            fake_field = true
        "#;
        let deserializer = toml::Deserializer::new(input);
        let mut ignored = Vec::new();
        let _config: PshConfig = serde_ignored::deserialize(deserializer, |path| {
            ignored.push(path.to_string());
        })
        .unwrap();
        assert!(ignored.contains(&"bogus".to_string()));
        assert!(ignored.contains(&"notify.fake_field".to_string()));
    }

    #[test]
    fn wall_config_per_output_parses() {
        let toml = r#"
            [wall]
            path = "/default.png"
            mode = "fill"
            interval = 600

            [wall.outputs."DP-1"]
            path = "/left.png"
            mode = "center"

            [wall.outputs."HDMI-A-1"]
            path = "/right/"
            interval = 120
        "#;
        let config: PshConfig = toml::from_str(toml).unwrap();
        assert_eq!(config.wall.interval, 600);
        assert_eq!(config.wall.outputs.len(), 2);
        assert_eq!(
            config.wall.outputs["DP-1"].path.as_deref(),
            Some(std::path::Path::new("/left.png"))
        );
        assert!(matches!(
            config.wall.outputs["DP-1"].mode,
            Some(WallMode::Center)
        ));
        assert_eq!(config.wall.outputs["HDMI-A-1"].interval, Some(120));
        // HDMI-A-1 inherits mode from top-level (None means fallback)
        assert!(config.wall.outputs["HDMI-A-1"].mode.is_none());
    }

    #[test]
    fn wall_config_defaults_backward_compat() {
        let toml = r#"
            [wall]
            path = "/wallpaper.png"
            mode = "fit"
        "#;
        let config: PshConfig = toml::from_str(toml).unwrap();
        assert!(config.wall.outputs.is_empty());
        assert_eq!(config.wall.interval, 300);
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
