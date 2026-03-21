//! Desktop entry parsing for `.desktop` files per the freedesktop.org Desktop Entry Specification.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

/// A parsed `.desktop` file entry representing a launchable application.
#[derive(Debug, Clone)]
pub struct DesktopEntry {
    /// Human-readable application name (from `Name=`).
    pub name: String,
    /// Command to execute (from `Exec=`, with field codes stripped).
    pub exec: String,
    /// Optional description (from `Comment=`).
    pub comment: Option<String>,
    /// Optional icon name for GTK icon theme lookup (from `Icon=`).
    pub icon: Option<String>,
    /// Whether the application should be launched in a terminal (from `Terminal=`).
    pub terminal: bool,
}

/// Load .desktop files from standard XDG data directories.
pub fn load_desktop_entries() -> Vec<DesktopEntry> {
    let mut entries = Vec::new();
    for dir in application_dirs() {
        if let Ok(read_dir) = fs::read_dir(&dir) {
            for file in read_dir.flatten() {
                let path = file.path();
                if path.extension().is_some_and(|ext| ext == "desktop") {
                    entries.extend(parse_desktop_file(&path));
                }
            }
        }
    }
    // Deduplicate by exec command before sorting.
    let mut seen = HashSet::new();
    entries.retain(|e| seen.insert(e.exec.clone()));
    entries.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    entries
}

/// Returns application directories per the XDG Base Directory Specification.
/// Searches `$XDG_DATA_HOME/applications` first, then each directory in
/// `$XDG_DATA_DIRS` (defaulting to `/usr/local/share:/usr/share`).
fn application_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();

    if let Some(base) = directories::BaseDirs::new() {
        dirs.push(base.data_dir().join("applications"));
    }

    let data_dirs = std::env::var("XDG_DATA_DIRS")
        .unwrap_or_else(|_| "/usr/local/share:/usr/share".to_string());
    for dir in data_dirs.split(':') {
        if !dir.is_empty() {
            dirs.push(PathBuf::from(dir).join("applications"));
        }
    }

    dirs
}

/// Parse a single `.desktop` file into a `DesktopEntry`.
///
/// Returns `None` if the file cannot be read, lacks required fields (`Name`, `Exec`),
/// has `NoDisplay=true`, or has a `Type` other than `Application`.
fn parse_desktop_file(path: &Path) -> Option<DesktopEntry> {
    let content = fs::read_to_string(path).ok()?;

    let mut name = None;
    let mut exec = None;
    let mut comment = None;
    let mut icon = None;
    let mut terminal = false;
    let mut no_display = false;
    let mut entry_type = None;
    let mut in_entry = false;

    for line in content.lines() {
        let line = line.trim();
        if line == "[Desktop Entry]" {
            in_entry = true;
            continue;
        }
        if line.starts_with('[') {
            if in_entry {
                break; // Next section, stop parsing
            }
            continue;
        }
        if !in_entry {
            continue;
        }

        if let Some(val) = line.strip_prefix("Name=") {
            name = Some(val.to_string());
        } else if let Some(val) = line.strip_prefix("Exec=") {
            exec = Some(strip_field_codes(val));
        } else if let Some(val) = line.strip_prefix("Comment=") {
            comment = Some(val.to_string());
        } else if let Some(val) = line.strip_prefix("Icon=") {
            icon = Some(val.to_string());
        } else if let Some(val) = line.strip_prefix("Terminal=") {
            terminal = val == "true";
        } else if let Some(val) = line.strip_prefix("NoDisplay=") {
            no_display = val == "true";
        } else if let Some(val) = line.strip_prefix("Type=") {
            entry_type = Some(val.to_string());
        }
    }

    if no_display {
        return None;
    }

    // Only include Application entries (or entries with no Type, for tolerance).
    if entry_type.as_deref().is_some_and(|t| t != "Application") {
        return None;
    }

    Some(DesktopEntry {
        name: name?,
        exec: exec?,
        comment,
        icon,
        terminal,
    })
}

/// Strip desktop entry field codes (`%f`, `%U`, etc.) from an Exec value.
/// Removes any `%` followed by an ASCII letter per the Desktop Entry Specification.
fn strip_field_codes(val: &str) -> String {
    let mut result = String::with_capacity(val.len());
    let mut chars = val.chars();
    while let Some(c) = chars.next() {
        if c == '%' {
            if let Some(next) = chars.next() {
                if !next.is_ascii_alphabetic() {
                    result.push(c);
                    result.push(next);
                }
            } else {
                result.push(c);
            }
        } else {
            result.push(c);
        }
    }
    result.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_desktop_file(content: &str) -> tempfile::NamedTempFile {
        let mut f = tempfile::NamedTempFile::with_suffix(".desktop").unwrap();
        f.write_all(content.as_bytes()).unwrap();
        f
    }

    #[test]
    fn parse_basic_entry() {
        let f = write_desktop_file(
            "[Desktop Entry]\nType=Application\nName=Firefox\nExec=firefox\nComment=Web Browser\nIcon=firefox\n",
        );
        let entry = parse_desktop_file(f.path()).unwrap();
        assert_eq!(entry.name, "Firefox");
        assert_eq!(entry.exec, "firefox");
        assert_eq!(entry.comment.as_deref(), Some("Web Browser"));
        assert_eq!(entry.icon.as_deref(), Some("firefox"));
        assert!(!entry.terminal);
    }

    #[test]
    fn parse_terminal_entry() {
        let f = write_desktop_file(
            "[Desktop Entry]\nType=Application\nName=htop\nExec=htop\nTerminal=true\n",
        );
        let entry = parse_desktop_file(f.path()).unwrap();
        assert!(entry.terminal);
    }

    #[test]
    fn skip_no_display() {
        let f = write_desktop_file(
            "[Desktop Entry]\nType=Application\nName=Hidden\nExec=hidden\nNoDisplay=true\n",
        );
        assert!(parse_desktop_file(f.path()).is_none());
    }

    #[test]
    fn skip_non_application_type() {
        let f = write_desktop_file(
            "[Desktop Entry]\nType=Link\nName=Docs\nExec=xdg-open http://example.com\n",
        );
        assert!(parse_desktop_file(f.path()).is_none());
    }

    #[test]
    fn missing_type_is_tolerated() {
        let f = write_desktop_file("[Desktop Entry]\nName=App\nExec=app\n");
        assert!(parse_desktop_file(f.path()).is_some());
    }

    #[test]
    fn missing_name_returns_none() {
        let f = write_desktop_file("[Desktop Entry]\nType=Application\nExec=app\n");
        assert!(parse_desktop_file(f.path()).is_none());
    }

    #[test]
    fn missing_exec_returns_none() {
        let f = write_desktop_file("[Desktop Entry]\nType=Application\nName=App\n");
        assert!(parse_desktop_file(f.path()).is_none());
    }

    #[test]
    fn strip_common_field_codes() {
        assert_eq!(strip_field_codes("firefox %u"), "firefox");
        assert_eq!(strip_field_codes("vim %F"), "vim");
        assert_eq!(strip_field_codes("app %k --flag"), "app  --flag");
    }

    #[test]
    fn strip_preserves_percent_literals() {
        assert_eq!(strip_field_codes("echo 100%"), "echo 100%");
        assert_eq!(strip_field_codes("echo %1"), "echo %1");
    }

    #[test]
    fn strip_double_percent() {
        // `%%` in desktop files means a literal `%` — first `%` pairs with second `%`,
        // which is not alphabetic, so both are kept.
        assert_eq!(strip_field_codes("echo %%"), "echo %%");
    }

    #[test]
    fn ignores_keys_outside_desktop_entry_section() {
        let f = write_desktop_file(
            "[Some Other Section]\nName=Wrong\nExec=wrong\n\n[Desktop Entry]\nType=Application\nName=Right\nExec=right\n",
        );
        let entry = parse_desktop_file(f.path()).unwrap();
        assert_eq!(entry.name, "Right");
        assert_eq!(entry.exec, "right");
    }

    #[test]
    fn stops_parsing_at_next_section() {
        let f = write_desktop_file(
            "[Desktop Entry]\nType=Application\nName=App\nExec=app\n\n[Desktop Action New]\nName=Overridden\n",
        );
        let entry = parse_desktop_file(f.path()).unwrap();
        assert_eq!(entry.name, "App");
    }
}
