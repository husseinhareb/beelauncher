//! Launch-frequency tracking.
//!
//! Stored at ~/.local/share/beelauncher/frequency.dat as a plain tab-separated
//! file: one `name\tcount` pair per line.  Written atomically (write to tmp,
//! then rename) so a crash during save never corrupts existing data.

use std::{
    collections::HashMap,
    fs,
    io::{self, BufRead, Write},
    path::{Path, PathBuf},
};

pub struct FrequencyMap {
    counts: HashMap<String, u32>,
    path:   PathBuf,
}

impl FrequencyMap {
    pub fn load() -> Self {
        let path = data_path();
        let counts = read_counts(&path).unwrap_or_default();
        Self { counts, path }
    }

    /// In-memory only — used in unit tests where no filesystem path is needed.
    #[cfg(test)]
    pub fn empty() -> Self {
        Self {
            counts: HashMap::new(),
            path:   PathBuf::new(),
        }
    }

    /// How many times `name` has been launched.
    #[inline]
    pub fn get(&self, name: &str) -> u32 {
        self.counts.get(name).copied().unwrap_or(0)
    }

    /// Increment the count for `name` and flush to disk.
    pub fn increment(&mut self, name: &str) {
        self.increment_in_memory(name);
        if let Err(e) = write_counts(&self.path, &self.counts) {
            log::warn!("frequency: save failed: {e}");
        }
    }

    /// Increment without flushing — used in tests.
    pub fn increment_in_memory(&mut self, name: &str) {
        *self.counts.entry(name.to_owned()).or_insert(0) += 1;
    }
}

// ── File I/O ─────────────────────────────────────────────────────────────────

fn data_path() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("~/.local/share"))
        .join("beelauncher")
        .join("frequency.dat")
}

fn read_counts(path: &Path) -> Option<HashMap<String, u32>> {
    let file = fs::File::open(path).ok()?;
    let mut map = HashMap::new();
    for line in io::BufReader::new(file).lines().map_while(Result::ok) {
        let mut parts = line.splitn(2, '\t');
        let name  = parts.next()?;
        let count: u32 = parts.next()?.parse().ok()?;
        map.insert(name.to_owned(), count);
    }
    Some(map)
}

/// Atomic write: temp file beside the target, then rename.
fn write_counts(path: &Path, counts: &HashMap<String, u32>) -> Result<(), io::Error> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    // Sort by name for stable, human-readable diffs.
    let mut pairs: Vec<(&String, &u32)> = counts.iter().collect();
    pairs.sort_unstable_by_key(|(name, _)| name.as_str());

    let tmp = path.with_extension("dat.tmp");
    {
        let mut f = fs::File::create(&tmp)?;
        for (name, count) in &pairs {
            writeln!(f, "{name}\t{count}")?;
        }
        f.flush()?;
    }
    fs::rename(&tmp, path)?;
    Ok(())
}
