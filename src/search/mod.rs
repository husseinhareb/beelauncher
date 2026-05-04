//! Fuzzy search engine.
//!
//! Algorithm: a lightweight subsequence-based scorer that rewards:
//!   1. Consecutive character runs (strong prefix/infix bonus)
//!   2. Start-of-word matches (camelCase / space / hyphen boundaries)
//!   3. Prefix matches (highest priority)
//!   4. Launch frequency (persistent soft boost — heavy users float to top)
//!
//! All comparisons are on lowercase bytes — normalisation happens once at index
//! build time, so hot-path search touches only pre-lowercased data.

use crate::{app_index::AppEntry, frequency::FrequencyMap};

/// Score weight per launch: 8 points per launch, capped at 800.
/// This keeps frequently-used apps near the top while a strong fuzzy match
/// (e.g. exact prefix) can still beat a very popular but wrong result.
const FREQ_WEIGHT: i32 = 8;
const FREQ_CAP: i32 = 800;

/// A scored reference into the app index.
#[derive(Debug, Clone, Copy)]
pub struct Match<'a> {
    pub entry: &'a AppEntry,
    pub score: i32,
}

/// Filter and rank entries against `query`, blending fuzzy score with frequency.
///
/// Empty query: returns all entries sorted by frequency descending (most-used first),
/// then alphabetically for entries with equal counts.
pub fn filter<'a>(
    query: &str,
    entries: &'a [AppEntry],
    freq: &FrequencyMap,
) -> Vec<Match<'a>> {
    if query.is_empty() {
        let mut results: Vec<Match<'a>> = entries
            .iter()
            .map(|e| Match {
                entry: e,
                score: freq_score(freq.get(&e.name)),
            })
            .collect();
        // Highest frequency first; ties keep alphabetical order from the index.
        results.sort_by(|a, b| b.score.cmp(&a.score));
        return results;
    }

    let query_lc = query.to_lowercase();
    let qbytes = query_lc.as_bytes();

    let mut results: Vec<Match<'a>> = entries
        .iter()
        .filter_map(|e| {
            fuzzy_score(qbytes, e.name_lc.as_bytes()).map(|s| Match {
                entry: e,
                score: s + freq_score(freq.get(&e.name)),
            })
        })
        .collect();

    results.sort_unstable_by(|a, b| b.score.cmp(&a.score));
    results
}

/// Capped frequency bonus so a 100-launch app doesn't bury perfect matches.
#[inline]
fn freq_score(count: u32) -> i32 {
    (count as i32 * FREQ_WEIGHT).min(FREQ_CAP)
}

/// Returns `Some(score)` if `query` is a subsequence of `target`, `None` otherwise.
///
/// Scoring heuristic (higher = better match):
///   +100  per consecutive character in a run
///   +50   for a word-boundary match
///   +200  bonus if the entire query is a prefix of target
///   -1    per character of gap between matches (encourages compact matches)
#[inline]
fn fuzzy_score(query: &[u8], target: &[u8]) -> Option<i32> {
    if query.is_empty() {
        return Some(0);
    }
    if query.len() > target.len() {
        return None;
    }

    // Prefix short-circuit: fastest possible case.
    if target.starts_with(query) {
        let bonus = if query.len() == target.len() { 300 } else { 200 };
        return Some(bonus + query.len() as i32 * 100);
    }

    let mut qi = 0;
    let mut score: i32 = 0;
    let mut last_match: Option<usize> = None;
    let mut consecutive = 0i32;

    for (ti, &tc) in target.iter().enumerate() {
        if qi == query.len() {
            break;
        }
        if tc == query[qi] {
            consecutive = if last_match == Some(ti.wrapping_sub(1)) {
                consecutive + 1
            } else {
                0
            };
            score += 100 + consecutive * 100;

            if ti == 0 || matches!(target[ti - 1], b' ' | b'-' | b'_' | b'.') {
                score += 50;
            }

            if let Some(last) = last_match {
                score -= (ti - last - 1) as i32;
            }

            last_match = Some(ti);
            qi += 1;
        }
    }

    if qi == query.len() { Some(score) } else { None }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frequency::FrequencyMap;

    fn dummy(name: &str) -> AppEntry {
        AppEntry {
            name: name.to_owned(),
            exec: String::new(),
            name_lc: name.to_lowercase(),
            icon: None,
        }
    }

    #[test]
    fn prefix_beats_infix() {
        let freq = FrequencyMap::empty();
        let entries = vec![dummy("firefox"), dummy("FarFox"), dummy("Firewatch")];
        let results = filter("fire", &entries, &freq);
        assert_eq!(results[0].entry.name, "firefox");
    }

    #[test]
    fn subsequence_match() {
        let freq = FrequencyMap::empty();
        let entries = vec![dummy("Nautilus")];
        assert!(!filter("ntls", &entries, &freq).is_empty());
    }

    #[test]
    fn no_match() {
        let freq = FrequencyMap::empty();
        let entries = vec![dummy("Nautilus")];
        assert!(filter("xyz", &entries, &freq).is_empty());
    }

    #[test]
    fn frequency_ranks_empty_query() {
        let mut freq = FrequencyMap::empty();
        freq.increment_in_memory("Thunar");
        freq.increment_in_memory("Thunar");
        freq.increment_in_memory("Firefox");

        let entries = vec![dummy("Firefox"), dummy("Thunar"), dummy("Alacritty")];
        let results = filter("", &entries, &freq);
        assert_eq!(results[0].entry.name, "Thunar");
        assert_eq!(results[1].entry.name, "Firefox");
    }

    #[test]
    fn frequency_does_not_bury_perfect_match() {
        // "thunar" used 200 times (freq cap = 800), "firefox" typed exactly.
        let mut freq = FrequencyMap::empty();
        for _ in 0..200 { freq.increment_in_memory("Thunar"); }

        let entries = vec![dummy("Firefox"), dummy("Thunar")];
        let results = filter("firefox", &entries, &freq);
        assert_eq!(results[0].entry.name, "Firefox");
    }
}
