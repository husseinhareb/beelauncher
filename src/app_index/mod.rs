//! Desktop file indexing: parse, cache, and expose application entries.
//!
//! Cache format: plain text, first line is a magic/version tag.
//! If the tag doesn't match CACHE_VERSION the file is discarded and rebuilt.
//! Bump CACHE_VERSION whenever the per-entry field layout changes.

use std::{
    collections::HashSet,
    ffi::OsString,
    fs,
    io::{self, BufRead, Write},
    path::{Path, PathBuf},
    time::SystemTime,
};

use thiserror::Error;

#[derive(Debug, Error)]
#[allow(dead_code)]
pub enum IndexError {
    #[error("I/O: {0}")]
    Io(#[from] io::Error),
}

/// Minimal representation of a .desktop application.
#[derive(Debug, Clone)]
pub struct AppEntry {
    pub name:    String,
    /// The command to execute (may contain field codes).
    pub exec:    String,
    /// Lowercase name used for matching — precomputed once.
    pub name_lc: String,
    /// Icon name or absolute path from the Icon= field, if present.
    pub icon:    Option<String>,
}

/// Loaded index of all known applications.
pub struct AppIndex {
    pub entries: Vec<AppEntry>,
}

impl AppIndex {
    /// Load from cache if fresh, otherwise rebuild from .desktop files.
    pub fn load() -> Self {
        let cache_path = cache_path();
        let desktop_dirs = desktop_dirs();

        // Check if cache is newer than any desktop dir mtime.
        if let Ok(entries) = try_load_cache(&cache_path, &desktop_dirs) {
            return Self { entries };
        }

        let entries = build_index(&desktop_dirs);
        let _ = write_cache(&cache_path, &entries);
        Self { entries }
    }

    #[allow(dead_code)]
    pub fn len(&self) -> usize {
        self.entries.len()
    }
}

// ── Cache ────────────────────────────────────────────────────────────────────

/// Increment this string whenever the per-entry line layout changes.
/// Any cache file whose first line doesn't match is silently discarded.
const CACHE_VERSION: &str = "beelauncher-cache-v3";

fn cache_path() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("beelauncher")
        .join("index.cache")
}

fn desktop_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(home) = dirs::data_local_dir() {
        dirs.push(home.join("applications"));
    }

    let data_dirs = std::env::var("XDG_DATA_DIRS").unwrap_or_default();
    if !data_dirs.is_empty() {
        for p in data_dirs.split(':') {
            if !p.is_empty() {
                let dir = PathBuf::from(p).join("applications");
                if !dirs.contains(&dir) {
                    dirs.push(dir);
                }
            }
        }
    } else {
        let local_share = PathBuf::from("/usr/local/share/applications");
        if !dirs.contains(&local_share) {
            dirs.push(local_share);
        }
        let usr_share = PathBuf::from("/usr/share/applications");
        if !dirs.contains(&usr_share) {
            dirs.push(usr_share);
        }
    }
    dirs
}

/// Returns the most recent mtime across all watched desktop directories.
///
/// We stat the directories themselves, not individual files inside them.
/// When any file is added, removed, or renamed in a directory, the kernel
/// updates that directory's mtime — regardless of the file's own timestamp.
/// This means a freshly installed package is always detected, even when the
/// package manager preserves the upstream build date on the .desktop file.
fn max_mtime(dirs: &[PathBuf]) -> SystemTime {
    dirs.iter()
        .filter_map(|dir| fs::metadata(dir).and_then(|m| m.modified()).ok())
        .max()
        .unwrap_or(SystemTime::UNIX_EPOCH)
}

fn try_load_cache(cache: &Path, dirs: &[PathBuf]) -> Result<Vec<AppEntry>, ()> {
    let cache_meta = fs::metadata(cache).map_err(|_| ())?;
    let cache_mtime = cache_meta.modified().map_err(|_| ())?;

    if max_mtime(dirs) > cache_mtime {
        return Err(());
    }

    let file = fs::File::open(cache).map_err(|_| ())?;
    let mut entries = Vec::with_capacity(256);
    let reader = io::BufReader::new(file);
    let mut lines = reader.lines();

    // First line must be the version tag — reject any cache with a different layout.
    match lines.next() {
        Some(Ok(ref tag)) if tag == CACHE_VERSION => {}
        _ => return Err(()),
    }

    while let Some(Ok(name)) = lines.next() {
        let exec = lines.next().and_then(|l| l.ok()).unwrap_or_default();
        let icon_line = lines.next().and_then(|l| l.ok()).unwrap_or_default();
        let icon = if icon_line.is_empty() { None } else { Some(icon_line) };
        let name_lc = name.to_lowercase();
        entries.push(AppEntry { name, exec, name_lc, icon });
    }

    Ok(entries)
}

fn write_cache(cache: &Path, entries: &[AppEntry]) -> Result<(), io::Error> {
    if let Some(parent) = cache.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut file = fs::File::create(cache)?;
    writeln!(file, "{CACHE_VERSION}")?;
    for e in entries {
        writeln!(file, "{}", e.name)?;
        writeln!(file, "{}", e.exec)?;
        writeln!(file, "{}", e.icon.as_deref().unwrap_or(""))?;
    }
    Ok(())
}

// ── Parser ────────────────────────────────────────────────────────────────────

fn build_index(dirs: &[PathBuf]) -> Vec<AppEntry> {
    let mut entries = Vec::with_capacity(256);
    let mut seen_ids: HashSet<OsString> = HashSet::new();

    for dir in dirs {
        let Ok(rd) = fs::read_dir(dir) else { continue };
        for item in rd.flatten() {
            let path = item.path();
            if path.extension().map(|e| e == "desktop").unwrap_or(false) {
                let file_name = item.file_name();
                if !seen_ids.insert(file_name) {
                    continue;
                }
                if let Some(entry) = parse_desktop_file(&path) {
                    entries.push(entry);
                }
            }
        }
    }
    entries.sort_unstable_by(|a, b| a.name.cmp(&b.name));
    entries
}

/// Minimal, fast .desktop parser.  We only need [Desktop Entry] Name and Exec.
/// Avoids allocations where possible by scanning bytes directly.
fn parse_desktop_file(path: &Path) -> Option<AppEntry> {
    let content = fs::read_to_string(path).ok()?;
    let mut in_desktop_section = false;
    let mut name: Option<String> = None;
    let mut exec: Option<String> = None;
    let mut icon: Option<String> = None;

    for line in content.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_desktop_section = line == "[Desktop Entry]";
            continue;
        }
        if !in_desktop_section {
            continue;
        }

        if let Some(rest) = line.strip_prefix("Name=") {
            if name.is_none() {
                name = Some(rest.to_owned());
            }
        } else if let Some(rest) = line.strip_prefix("Exec=") {
            if exec.is_none() {
                exec = Some(rest.to_owned());
            }
        } else if let Some(rest) = line.strip_prefix("Icon=") {
            if icon.is_none() && !rest.is_empty() {
                icon = Some(rest.to_owned());
            }
        } else if let Some(rest) = line.strip_prefix("NoDisplay=") {
            if rest.eq_ignore_ascii_case("true") {
                return None;
            }
        } else if let Some(rest) = line.strip_prefix("Type=") {
            if !rest.eq_ignore_ascii_case("Application") && exec.is_none() && name.is_none() {
                return None;
            }
        }
    }

    let name = name?;
    let exec = exec?;
    let name_lc = name.to_lowercase();
    Some(AppEntry { name, exec, name_lc, icon })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;

    #[test]
    fn test_deduplication_overrides_lower_priority() {
        let temp_dir = std::env::temp_dir().join(format!("beelauncher_test_{}", std::process::id()));
        let user_apps = temp_dir.join("user_apps");
        let sys_apps = temp_dir.join("sys_apps");

        fs::create_dir_all(&user_apps).unwrap();
        fs::create_dir_all(&sys_apps).unwrap();

        let user_claude = user_apps.join("claude.desktop");
        let sys_claude = sys_apps.join("claude.desktop");

        let mut f1 = File::create(&user_claude).unwrap();
        writeln!(f1, "[Desktop Entry]\nName=Claude User\nExec=claude-user\nType=Application").unwrap();

        let mut f2 = File::create(&sys_claude).unwrap();
        writeln!(f2, "[Desktop Entry]\nName=Claude Sys\nExec=claude-sys\nType=Application").unwrap();

        let entries = build_index(&[user_apps.clone(), sys_apps.clone()]);
        let _ = fs::remove_dir_all(&temp_dir);

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "Claude User");
        assert_eq!(entries[0].exec, "claude-user");
    }

    #[test]
    fn test_nodisplay_shadows_lower_priority() {
        let temp_dir = std::env::temp_dir().join(format!("beelauncher_test_nodisplay_{}", std::process::id()));
        let user_apps = temp_dir.join("user_apps");
        let sys_apps = temp_dir.join("sys_apps");

        fs::create_dir_all(&user_apps).unwrap();
        fs::create_dir_all(&sys_apps).unwrap();

        let user_app = user_apps.join("app.desktop");
        let sys_app = sys_apps.join("app.desktop");

        let mut f1 = File::create(&user_app).unwrap();
        writeln!(f1, "[Desktop Entry]\nName=App\nExec=app\nNoDisplay=true\nType=Application").unwrap();

        let mut f2 = File::create(&sys_app).unwrap();
        writeln!(f2, "[Desktop Entry]\nName=App\nExec=app\nType=Application").unwrap();

        let entries = build_index(&[user_apps.clone(), sys_apps.clone()]);
        let _ = fs::remove_dir_all(&temp_dir);

        assert_eq!(entries.len(), 0);
    }

    #[test]
    fn test_live_index_no_duplicate_claude() {
        let index = AppIndex::load();
        let claude_count = index.entries.iter().filter(|e| e.name == "Claude").count();
        assert_eq!(claude_count, 1);
    }
}
