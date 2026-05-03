//! Desktop file indexing: parse, cache, and expose application entries.
//!
//! Cache format: a simple binary file (bincode-style manual serialization avoided;
//! we use a flat text cache instead for zero-dependency, forward-compatible reads).

use std::{
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

fn cache_path() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("beelauncher")
        .join("index.cache")
}

fn desktop_dirs() -> Vec<PathBuf> {
    let mut dirs = vec![PathBuf::from("/usr/share/applications")];
    if let Some(home) = dirs::data_local_dir() {
        dirs.push(home.join("applications"));
    }
    dirs
}

/// Returns max mtime across all .desktop files in all dirs.
fn max_mtime(dirs: &[PathBuf]) -> SystemTime {
    let mut max = SystemTime::UNIX_EPOCH;
    for dir in dirs {
        let Ok(rd) = fs::read_dir(dir) else { continue };
        for entry in rd.flatten() {
            let p = entry.path();
            if p.extension().map(|e| e == "desktop").unwrap_or(false) {
                if let Ok(m) = entry.metadata().and_then(|m| m.modified()) {
                    if m > max {
                        max = m;
                    }
                }
            }
        }
    }
    max
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

    while let Some(Ok(name)) = lines.next() {
        let exec = lines.next().and_then(|l| l.ok()).unwrap_or_default();
        let name_lc = name.to_lowercase();
        entries.push(AppEntry { name, exec, name_lc });
    }

    Ok(entries)
}

fn write_cache(cache: &Path, entries: &[AppEntry]) -> Result<(), io::Error> {
    if let Some(parent) = cache.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut file = fs::File::create(cache)?;
    for e in entries {
        writeln!(file, "{}", e.name)?;
        writeln!(file, "{}", e.exec)?;
    }
    Ok(())
}

// ── Parser ────────────────────────────────────────────────────────────────────

fn build_index(dirs: &[PathBuf]) -> Vec<AppEntry> {
    let mut entries = Vec::with_capacity(256);
    for dir in dirs {
        let Ok(rd) = fs::read_dir(dir) else { continue };
        for item in rd.flatten() {
            let path = item.path();
            if path.extension().map(|e| e == "desktop").unwrap_or(false) {
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

    for line in content.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_desktop_section = line == "[Desktop Entry]";
            continue;
        }
        if !in_desktop_section {
            continue;
        }

        // Fast key extraction without splitting the whole line.
        if let Some(rest) = line.strip_prefix("Name=") {
            if name.is_none() {
                name = Some(rest.to_owned());
            }
        } else if let Some(rest) = line.strip_prefix("Exec=") {
            if exec.is_none() {
                exec = Some(rest.to_owned());
            }
        } else if let Some(rest) = line.strip_prefix("NoDisplay=") {
            if rest.eq_ignore_ascii_case("true") {
                return None;
            }
        } else if let Some(rest) = line.strip_prefix("Type=") {
            // Only include Application entries.
            if !rest.eq_ignore_ascii_case("Application") && exec.is_none() && name.is_none() {
                return None;
            }
        }
    }

    let name = name?;
    let exec = exec?;
    let name_lc = name.to_lowercase();
    Some(AppEntry { name, exec, name_lc })
}
