// SPDX-License-Identifier: BSD-3-Clause

use std::cmp::{Ordering as ComparisonOrdering, Reverse};
use std::collections::BinaryHeap;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc::{self, Receiver},
};
use std::thread;
use std::time::{Duration, Instant};

use crate::search_index::{SearchIndex, character_mask};

const CANCELLATION_CHECK_INTERVAL: usize = 1024;

const FUZZY_RESULT_LIMIT: usize = 500;

const PROGRESS_INTERVAL: Duration = Duration::from_millis(75);

#[derive(Debug)]
pub struct FuzzyWorkerResult {
    pub generation: u64,

    pub indices: Vec<usize>,

    pub examined: usize,

    pub total: usize,

    pub finished: bool,

    pub cancelled: bool,
}

/*
 * A larger RankedMatch is always a better result.
 *
 * Directory status comes first so every retained directory remains above
 * ordinary files. Within each group, relevance controls ordering. The original
 * entry index gives deterministic ordering for equal scores.
 */
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RankedMatch {
    entry_index: usize,

    is_directory: bool,

    score: i64,
}

impl Ord for RankedMatch {
    fn cmp(&self, other: &Self) -> ComparisonOrdering {
        self.is_directory
            .cmp(&other.is_directory)
            .then_with(|| self.score.cmp(&other.score))
            .then_with(|| other.entry_index.cmp(&self.entry_index))
    }
}

impl PartialOrd for RankedMatch {
    fn partial_cmp(&self, other: &Self) -> Option<ComparisonOrdering> {
        Some(self.cmp(other))
    }
}

/*
 * Search a stable shared index.
 *
 * The worker retains only the best FUZZY_RESULT_LIMIT matches. It never builds
 * a vector containing every technically matching path.
 */
pub fn start_fuzzy_worker(
    index: Arc<SearchIndex>,
    query: String,
    generation: u64,
    show_hidden: bool,
    scope_prefix: Option<String>,
    cancel_signal: Arc<AtomicBool>,
) -> Receiver<FuzzyWorkerResult> {
    let (sender, receiver) = mpsc::channel();

    thread::spawn(move || {
        let folded_query = query.to_lowercase();

        let query_mask = character_mask(&folded_query);

        let maximum_typo_distance = maximum_typo_distance(folded_query.len());

        let total = index.len();

        /*
         * Reverse keeps the worst retained result at the top of the heap.
         *
         * Once 500 matches have been retained, a new candidate replaces that
         * worst match only when it ranks higher.
         */
        let mut best_matches: BinaryHeap<Reverse<RankedMatch>> =
            BinaryHeap::with_capacity(FUZZY_RESULT_LIMIT.saturating_add(1));

        let mut last_progress = Instant::now();

        for (position, record) in index.records().iter().enumerate() {
            if position % CANCELLATION_CHECK_INTERVAL == 0 && cancel_signal.load(Ordering::Relaxed)
            {
                let _ = sender.send(FuzzyWorkerResult {
                    generation,

                    indices: Vec::new(),

                    examined: position,

                    total,

                    finished: true,

                    cancelled: true,
                });

                return;
            }

            if !show_hidden && record.searchable_name.starts_with('.') {
                continue;
            }

            if let Some(scope_prefix) = scope_prefix.as_deref() {
                if !scope_prefix.is_empty()
                    && record.searchable_path.as_ref() != scope_prefix
                    && !record
                        .searchable_path
                        .strip_prefix(scope_prefix)
                        .is_some_and(|suffix| suffix.starts_with('/'))
                {
                    continue;
                }
            }

            /*
             * If the query is longer than the complete searchable path by more
             * than the permitted typo distance, neither subsequence matching nor
             * typo matching can succeed.
             */
            if folded_query.len() > record.path_length as usize + maximum_typo_distance {
                continue;
            }

            /*
             * Count query characters completely absent from the candidate path.
             *
             * Exact and subsequence matching permit none. Typo matching may
             * account for a small number through replacements or insertions.
             */
            if query_mask != 0 {
                let missing_characters =
                    (query_mask & !record.character_mask).count_ones() as usize;

                if missing_characters > maximum_typo_distance {
                    continue;
                }
            }

            let Some(score) = score_candidate(
                &record.searchable_name,
                &record.searchable_path,
                &folded_query,
            ) else {
                continue;
            };

            retain_ranked_match(
                &mut best_matches,
                RankedMatch {
                    entry_index: record.entry_index,

                    is_directory: record.is_directory,

                    score,
                },
            );

            if last_progress.elapsed() >= PROGRESS_INTERVAL {
                let indices = ranked_indices(&best_matches);

                if sender
                    .send(FuzzyWorkerResult {
                        generation,

                        indices,

                        examined: position.saturating_add(1),

                        total,

                        finished: false,

                        cancelled: false,
                    })
                    .is_err()
                {
                    return;
                }

                last_progress = Instant::now();
            }
        }

        if cancel_signal.load(Ordering::Relaxed) {
            let _ = sender.send(FuzzyWorkerResult {
                generation,

                indices: Vec::new(),

                examined: total,

                total,

                finished: true,

                cancelled: true,
            });

            return;
        }

        let indices = ranked_indices(&best_matches);

        let _ = sender.send(FuzzyWorkerResult {
            generation,

            indices,

            examined: total,

            total,

            finished: true,

            cancelled: false,
        });
    });

    receiver
}

fn retain_ranked_match(matches: &mut BinaryHeap<Reverse<RankedMatch>>, candidate: RankedMatch) {
    if matches.len() < FUZZY_RESULT_LIMIT {
        matches.push(Reverse(candidate));

        return;
    }

    let should_replace = matches
        .peek()
        .is_some_and(|Reverse(worst)| candidate > *worst);

    if should_replace {
        matches.pop();

        matches.push(Reverse(candidate));
    }
}

fn ranked_indices(matches: &BinaryHeap<Reverse<RankedMatch>>) -> Vec<usize> {
    let mut ranked: Vec<RankedMatch> = matches
        .iter()
        .map(|Reverse(candidate)| *candidate)
        .collect();

    ranked.sort_unstable_by(|left, right| right.cmp(left));

    ranked
        .into_iter()
        .map(|candidate| candidate.entry_index)
        .collect()
}

fn maximum_typo_distance(query_length: usize) -> usize {
    match query_length {
        0..=2 => 0,

        3 => 1,

        4..=8 => 2,

        _ => 3,
    }
}

/*
 * Search filenames first, followed by complete individual path components.
 *
 * Characters are never allowed to scatter across unrelated directory names.
 */
fn score_candidate(name: &str, path: &str, query: &str) -> Option<i64> {
    if query.is_empty() {
        return Some(0);
    }

    let mut best_score = score_component(name, query).map(|score| score + 1_000);

    for component in path.split(['/', '\\']) {
        if component.is_empty() || component == name {
            continue;
        }

        if let Some(score) = score_component(component, query) {
            let component_score = score + 400;

            best_score = Some(
                best_score
                    .map(|current| current.max(component_score))
                    .unwrap_or(component_score),
            );
        }
    }

    best_score
}

fn score_component(candidate: &str, query: &str) -> Option<i64> {
    if candidate.is_empty() {
        return None;
    }

    if candidate == query {
        return Some(10_000);
    }

    if candidate.starts_with(query) {
        return Some(8_000 - candidate.len().saturating_sub(query.len()) as i64);
    }

    if let Some(position) = candidate.find(query) {
        return Some(6_000 - position as i64);
    }

    let subsequence_score = compact_subsequence_score(candidate.as_bytes(), query.as_bytes());

    let typo_score = typo_score(candidate.as_bytes(), query.as_bytes());

    match (subsequence_score, typo_score) {
        (Some(left), Some(right)) => Some(left.max(right)),

        (Some(score), None) | (None, Some(score)) => Some(score),

        (None, None) => None,
    }
}

/*
 * Ordered abbreviation matching:
 *
 *     nct  -> noct
 *     cpuf -> cpuforge
 *
 * Reject matches whose characters are scattered too widely.
 */
fn compact_subsequence_score(candidate: &[u8], query: &[u8]) -> Option<i64> {
    if query.is_empty() {
        return Some(0);
    }

    let mut query_position = 0_usize;

    let mut first_match = None;

    let mut previous_match = None;

    let mut consecutive_pairs = 0_usize;

    for (candidate_position, character) in candidate.iter().copied().enumerate() {
        if query_position == query.len() {
            break;
        }

        if character != query[query_position] {
            continue;
        }

        first_match.get_or_insert(candidate_position);

        if previous_match.is_some_and(|previous| previous + 1 == candidate_position) {
            consecutive_pairs += 1;
        }

        previous_match = Some(candidate_position);

        query_position += 1;
    }

    if query_position != query.len() {
        return None;
    }

    let first = first_match?;

    let last = previous_match?;

    let span = last.saturating_sub(first).saturating_add(1);

    /*
     * Four query characters spread across a forty-character component are
     * accidental noise, not a useful fuzzy result.
     */
    let maximum_span = query.len().saturating_mul(3).saturating_add(2);

    if span > maximum_span {
        return None;
    }

    let gap_count = span.saturating_sub(query.len());

    Some(
        4_000 + consecutive_pairs as i64 * 120
            - gap_count as i64 * 80
            - first as i64 * 10
            - candidate.len().saturating_sub(query.len()) as i64,
    )
}

/*
 * Typo-aware matching with adjacent transposition support.
 *
 * This handles:
 *
 *     hlpe -> help
 *     hlep -> help
 *     hepl -> help
 *     halp -> help
 */
fn typo_score(candidate: &[u8], query: &[u8]) -> Option<i64> {
    if query.len() < 3 {
        return None;
    }

    /*
     * Do not run edit distance against wildly different component lengths.
     */
    let length_difference = candidate.len().abs_diff(query.len());

    let maximum_distance = maximum_typo_distance(query.len());

    if length_difference > maximum_distance {
        return None;
    }

    let distance = bounded_damerau_levenshtein(candidate, query, maximum_distance)?;

    Some(5_000 - distance as i64 * 700 - candidate.len().abs_diff(query.len()) as i64 * 40)
}

/*
 * Restricted Damerau-Levenshtein distance.
 *
 * Insertions, deletions, replacements, and adjacent swaps each cost one.
 * Returning early when a row cannot beat max_distance keeps typo matching
 * bounded for large result sets.
 */
fn bounded_damerau_levenshtein(left: &[u8], right: &[u8], max_distance: usize) -> Option<usize> {
    if left.len().abs_diff(right.len()) > max_distance {
        return None;
    }

    let mut previous_previous = vec![0_usize; right.len() + 1];

    let mut previous: Vec<usize> = (0..=right.len()).collect();

    let mut current = vec![0_usize; right.len() + 1];

    for left_index in 1..=left.len() {
        current[0] = left_index;

        let mut row_minimum = current[0];

        for right_index in 1..=right.len() {
            let substitution_cost = usize::from(left[left_index - 1] != right[right_index - 1]);

            current[right_index] = (previous[right_index] + 1)
                .min(current[right_index - 1] + 1)
                .min(previous[right_index - 1] + substitution_cost);

            if left_index > 1
                && right_index > 1
                && left[left_index - 1] == right[right_index - 2]
                && left[left_index - 2] == right[right_index - 1]
            {
                current[right_index] =
                    current[right_index].min(previous_previous[right_index - 2] + 1);
            }

            row_minimum = row_minimum.min(current[right_index]);
        }

        if row_minimum > max_distance {
            return None;
        }

        std::mem::swap(&mut previous_previous, &mut previous);

        std::mem::swap(&mut previous, &mut current);
    }

    let distance = previous[right.len()];

    (distance <= max_distance).then_some(distance)
}

/*
 * Return character positions to highlight inside a displayed relative path.
 *
 * This deliberately runs only for visible UI rows. Storing positions for
 * every worker result would consume enormous amounts of memory on multi-
 * million-entry searches.
 *
 * Exact substring and compact-subsequence matches highlight their contributing
 * characters. Typo matches highlight the complete component because inserted,
 * removed, replaced, or transposed letters do not have a single exact
 * character-to-character representation.
 */
pub fn fuzzy_highlight_positions(display_path: &str, query: &str) -> Vec<usize> {
    if query.is_empty() {
        return Vec::new();
    }

    let folded_path = display_path.to_lowercase();

    let folded_query = query.to_lowercase();

    let component_count = folded_path
        .split(['/', '\\'])
        .filter(|component| !component.is_empty())
        .count();

    let mut best_score = None;

    let mut best_positions = Vec::new();

    let mut component_character_start = 0_usize;

    let mut component_number = 0_usize;

    for component in folded_path.split(['/', '\\']) {
        let component_length = component.chars().count();

        if component.is_empty() {
            /*
             * Account for the separator before the next component.
             */
            component_character_start = component_character_start.saturating_add(1);

            continue;
        }

        component_number += 1;

        let filename_component = component_number == component_count;

        let Some(component_score) = score_component(component, &folded_query) else {
            component_character_start = component_character_start
                .saturating_add(component_length)
                .saturating_add(1);

            continue;
        };

        let total_score = component_score + if filename_component { 1_000 } else { 400 };

        if best_score.is_some_and(|current| current >= total_score) {
            component_character_start = component_character_start
                .saturating_add(component_length)
                .saturating_add(1);

            continue;
        }

        let local_positions = component_highlight_positions(component, &folded_query);

        best_positions = local_positions
            .into_iter()
            .map(|position| component_character_start + position)
            .collect();

        best_score = Some(total_score);

        component_character_start = component_character_start
            .saturating_add(component_length)
            .saturating_add(1);
    }

    best_positions
}

fn component_highlight_positions(candidate: &str, query: &str) -> Vec<usize> {
    if candidate == query {
        return (0..candidate.chars().count()).collect();
    }

    if let Some(byte_start) = candidate.find(query) {
        let character_start = candidate[..byte_start].chars().count();

        return (character_start..character_start + query.chars().count()).collect();
    }

    let subsequence_score = compact_subsequence_score(candidate.as_bytes(), query.as_bytes());

    let typo_match = typo_score(candidate.as_bytes(), query.as_bytes());

    /*
     * Follow the same winning strategy as score_component().
     */
    if subsequence_score.is_some() && subsequence_score >= typo_match {
        return compact_subsequence_positions(candidate, query).unwrap_or_default();
    }

    if typo_match.is_some() {
        /*
         * For help matched by hlpe, hlep, or hepl, highlight "help" as the
         * component that satisfied the typo-aware match.
         */
        return (0..candidate.chars().count()).collect();
    }

    Vec::new()
}

fn compact_subsequence_positions(candidate: &str, query: &str) -> Option<Vec<usize>> {
    let query_characters: Vec<char> = query.chars().collect();

    if query_characters.is_empty() {
        return Some(Vec::new());
    }

    let mut query_position = 0_usize;

    let mut positions = Vec::with_capacity(query_characters.len());

    for (candidate_position, candidate_character) in candidate.chars().enumerate() {
        if candidate_character != query_characters[query_position] {
            continue;
        }

        positions.push(candidate_position);

        query_position += 1;

        if query_position == query_characters.len() {
            return Some(positions);
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::score_component;

    #[test]
    fn exact_match_is_strongest() {
        assert!(
            score_component("help", "help").unwrap() > score_component("helper", "help").unwrap()
        );
    }

    #[test]
    fn adjacent_transposition_matches() {
        assert!(score_component("help", "hlpe").is_some());
        assert!(score_component("help", "hlep").is_some());
        assert!(score_component("help", "hepl").is_some());
    }

    #[test]
    fn replacement_typo_matches() {
        assert!(score_component("help", "halp").is_some());
    }

    #[test]
    fn compact_abbreviation_matches() {
        assert!(score_component("cpuforge", "cpuf").is_some());
    }

    #[test]
    fn widely_scattered_match_is_rejected() {
        assert!(score_component("columnexperimentwithfont", "cpuf").is_none());
    }

    #[test]
    fn unrelated_component_is_rejected() {
        assert!(score_component("deleteaction", "tstf").is_none());
    }
}
