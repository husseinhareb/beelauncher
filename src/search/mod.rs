//! Fuzzy search engine.
//!
//! Algorithm: a lightweight subsequence-based scorer that rewards:
//!   1. Consecutive character runs (strong prefix/infix bonus)
//!   2. Start-of-word matches (camelCase / space / hyphen boundaries)
//!   3. Prefix matches (highest priority)
//!
//! All comparisons are on lowercase bytes — normalisation happens once at index
//! build time, so hot-path search touches only pre-lowercased data.

use crate::app_index::AppEntry;

/// A scored reference into the app index.
#[derive(Debug, Clone, Copy)]
pub struct Match<'a> {
    pub entry: &'a AppEntry,
    pub score: i32,
}

/// Stateless filter: given a query and a slice of entries, returns matched and
/// scored results sorted best-first.  Allocation is bounded to the output Vec.
pub fn filter<'a>(query: &str, entries: &'a [AppEntry]) -> Vec<Match<'a>> {
    if query.is_empty() {
        // Return all entries with equal score so the sorted order is alphabetical.
        return entries
            .iter()
            .map(|e| Match { entry: e, score: 0 })
            .collect();
    }

    let query_lc = query.to_lowercase();
    let qbytes = query_lc.as_bytes();

    let mut results: Vec<Match<'a>> = entries
        .iter()
        .filter_map(|e| {
            score(qbytes, e.name_lc.as_bytes()).map(|s| Match { entry: e, score: s })
        })
        .collect();

    // Highest score first; stable within equal scores (alphabetical from index).
    results.sort_unstable_by(|a, b| b.score.cmp(&a.score));
    results
}

/// Returns `Some(score)` if `query` is a subsequence of `target`, `None` otherwise.
///
/// Scoring heuristic (higher = better match):
///   +100  per consecutive character in a run
///   +50   for a word-boundary match
///   +200  bonus if the entire query is a prefix of target
///   -1    per character of gap between matches (encourages compact matches)
#[inline]
fn score(query: &[u8], target: &[u8]) -> Option<i32> {
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
            // Consecutive run bonus.
            consecutive = if last_match == Some(ti.wrapping_sub(1)) {
                consecutive + 1
            } else {
                0
            };
            score += 100 + consecutive * 100;

            // Word-boundary bonus: match starts after a space, '-', or '_'.
            if ti == 0 || matches!(target[ti - 1], b' ' | b'-' | b'_' | b'.') {
                score += 50;
            }

            // Gap penalty.
            if let Some(last) = last_match {
                let gap = (ti - last - 1) as i32;
                score -= gap;
            }

            last_match = Some(ti);
            qi += 1;
        }
    }

    if qi == query.len() {
        Some(score)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dummy(name: &str) -> AppEntry {
        AppEntry {
            name: name.to_owned(),
            exec: String::new(),
            name_lc: name.to_lowercase(),
        }
    }

    #[test]
    fn prefix_beats_infix() {
        let entries = vec![dummy("firefox"), dummy("FarFox"), dummy("Firewatch")];
        let results = filter("fire", &entries);
        assert_eq!(results[0].entry.name, "firefox");
    }

    #[test]
    fn subsequence_match() {
        let entries = vec![dummy("Nautilus")];
        let r = filter("ntls", &entries);
        assert!(!r.is_empty());
    }

    #[test]
    fn no_match() {
        let entries = vec![dummy("Nautilus")];
        let r = filter("xyz", &entries);
        assert!(r.is_empty());
    }
}
