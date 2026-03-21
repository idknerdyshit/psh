use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct DesktopEntry {
    pub name: String,
    pub exec: String,
    pub comment: Option<String>,
    pub icon: Option<String>,
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
    entries.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    entries.dedup_by(|a, b| a.exec == b.exec);
    entries
}

fn application_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();

    if let Some(data_home) = directories::BaseDirs::new() {
        dirs.push(data_home.data_dir().join("applications"));
    }

    dirs.push(PathBuf::from("/usr/share/applications"));
    dirs.push(PathBuf::from("/usr/local/share/applications"));
    dirs
}

fn parse_desktop_file(path: &PathBuf) -> Option<DesktopEntry> {
    let content = fs::read_to_string(path).ok()?;

    let mut name = None;
    let mut exec = None;
    let mut comment = None;
    let mut icon = None;
    let mut terminal = false;
    let mut no_display = false;
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
            // Strip field codes like %f, %u, %U, etc.
            let cleaned = val
                .replace("%f", "")
                .replace("%F", "")
                .replace("%u", "")
                .replace("%U", "")
                .trim()
                .to_string();
            exec = Some(cleaned);
        } else if let Some(val) = line.strip_prefix("Comment=") {
            comment = Some(val.to_string());
        } else if let Some(val) = line.strip_prefix("Icon=") {
            icon = Some(val.to_string());
        } else if let Some(val) = line.strip_prefix("Terminal=") {
            terminal = val == "true";
        } else if let Some(val) = line.strip_prefix("NoDisplay=") {
            no_display = val == "true";
        }
    }

    if no_display {
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
