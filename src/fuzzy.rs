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

use crate::{
    query::{
        ParsedQuery, record_boolean_score_with_text, record_matches_query_filters,
        record_matches_query_filters_with_signed_text,
    },
    scan::SortMode,
    search_index::{SearchIndex, SearchRecord, character_mask},
};

const CANCELLATION_CHECK_INTERVAL: usize = 1024;

const PROGRESS_INTERVAL: Duration = Duration::from_millis(75);

/*
 * While an Exact search is still running, publish only a bounded preview.
 *
 * The completed message contains every Exact match. This avoids repeatedly
 * cloning a potentially enormous result vector during one corpus traversal.
 */
const EXACT_PROGRESS_RESULT_LIMIT: usize = 500;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkerEntryFilter {
    All,

    FilesOnly,

    DirectoriesOnly,
}

impl WorkerEntryFilter {
    fn matches(self, is_directory: bool, is_symlink: bool) -> bool {
        match self {
            Self::All => true,

            /*
             * Symlinks remain file-like unless the indexed entry itself is a
             * real directory.
             */
            Self::FilesOnly => !is_directory || is_symlink,

            Self::DirectoriesOnly => is_directory,
        }
    }
}

#[derive(Debug)]
pub struct FuzzyWorkerResult {
    pub generation: u64,

    pub indices: Vec<usize>,

    pub examined: usize,

    pub total: usize,

    pub finished: bool,

    pub cancelled: bool,

    /*
     * True only when an Exact worker found more eligible matches than its
     * configured result limit could retain.
     *
     * Fuzzy workers always report false because their best-500 policy is an
     * established ranking policy rather than the Exact Tree safety cap.
     */
    pub limit_reached: bool,
}

/*
 * A larger RankedMatch is always a better result.
 *
 * Fuzzy results retain Scry's ordinary two-group listing structure:
 *
 * - directories appear first;
 * - files follow afterward.
 *
 * Relevance controls ordering independently inside each group. A weak directory
 * therefore cannot outrank a stronger directory, and a weak file cannot outrank
 * a stronger file, while the complete list remains visually predictable.
 *
 * The original entry index provides deterministic ordering for otherwise equal
 * results.
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

fn score_fuzzy_text(
    record: &crate::search_index::SearchRecord,
    value: &str,
    case_sensitive: bool,
) -> Option<i64> {
    if value.is_empty() {
        return None;
    }

    if case_sensitive {
        let original_name = record
            .original_path
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or(record.original_path.as_ref());

        score_candidate(original_name, &record.original_path, value)
    } else {
        score_candidate(&record.searchable_name, &record.searchable_path, value)
    }
}

fn record_matches_fuzzy_signed_text(
    record: &crate::search_index::SearchRecord,
    value: &str,
    case_sensitive: bool,
) -> bool {
    score_fuzzy_text(record, value, case_sensitive).is_some()
}

fn score_fuzzy_signed_term(
    record: &crate::search_index::SearchRecord,
    term: &crate::query::QueryHighlightTerm,
) -> Option<i64> {
    score_fuzzy_text(record, &term.value, term.case_sensitive)
}

/*
 * Search a stable shared index.
 *
 * The worker retains only the configured number of highest-ranked matches.
 * It never builds a vector containing every technically matching path.
 */
#[allow(clippy::too_many_arguments)]
pub fn start_fuzzy_worker(
    index: Arc<SearchIndex>,
    parsed_query: ParsedQuery,
    generation: u64,
    show_hidden: bool,
    hidden_only: bool,
    scope_prefix: Option<String>,
    entry_filter: WorkerEntryFilter,
    result_limit: usize,
    cancel_signal: Arc<AtomicBool>,
) -> Receiver<FuzzyWorkerResult> {
    let (sender, receiver) = mpsc::channel();

    thread::spawn(move || {
        /*
         * ParsedQuery already stores its ordinary text in lowercase.
         *
         * Structured terms remain in parsed_query and are evaluated directly
         * against SearchRecord inside this worker.
         */
        let folded_query = parsed_query.search_text().to_string();

        /*
         * Positive compact +terms participate in relevance ranking as well as
         * eligibility while Fuzzy mode is active.
         */
        let fuzzy_signed_terms = parsed_query.fuzzy_signed_highlight_terms();

        let query_mask = character_mask(&folded_query);

        let maximum_typo_distance = maximum_typo_distance(folded_query.len());

        let total = index.len();

        /*
         * Reverse keeps the worst retained result at the top of the heap.
         *
         * Once the limit has been reached, a new candidate replaces that worst
         * match only when it ranks higher.
         */
        let mut best_matches: BinaryHeap<Reverse<RankedMatch>> =
            BinaryHeap::with_capacity(result_limit.saturating_add(1));

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

                    limit_reached: false,
                });

                return;
            }

            /*
             * Scope is checked before every more expensive query operation.
             */
            if let Some(scope_prefix) = scope_prefix.as_deref()
                && !scope_prefix.is_empty()
                && record.searchable_path.as_ref() != scope_prefix
                && !record
                    .searchable_path
                    .strip_prefix(scope_prefix)
                    .is_some_and(|suffix| suffix.starts_with('/'))
            {
                continue;
            }

            /*
             * Hidden Only accepts entries whose searchable path contains at least one
             * hidden component. This includes every descendant beneath a dot-directory.
             */
            if hidden_only {
                if !record.contains_hidden_component {
                    continue;
                }
            } else if !show_hidden && record.contains_hidden_component {
                continue;
            }

            if !entry_filter.matches(record.is_directory, record.is_symlink) {
                continue;
            }

            /*
             * type:, ext:, positive terms, and negative terms are now evaluated
             * here rather than through a corpus-sized Boolean mask constructed
             * by App.
             */
            if !record_matches_query_filters_with_signed_text(
                record,
                &parsed_query,
                record_matches_fuzzy_signed_text,
            ) {
                continue;
            }

            /*
             * Boolean text operands participate in relevance ranking as well as
             * eligibility.
             *
             * OR keeps the strongest matching branch, AND combines required branches, and
             * NOT contributes no positive score.
             */
            let Some(boolean_score) =
                record_boolean_score_with_text(record, &parsed_query, score_fuzzy_text)
            else {
                continue;
            };

            /*
             * Every positive fuzzy +term is a required match.
             *
             * Add the quality of all those matches so an entry that strongly satisfies
             * every requirement ranks above one containing widely scattered accidental
             * matches.
             */
            let mut signed_terms_score = 0_i64;

            let mut signed_terms_rankable = true;

            for term in &fuzzy_signed_terms {
                let Some(score) = score_fuzzy_signed_term(record, term) else {
                    signed_terms_rankable = false;

                    break;
                };

                signed_terms_score = signed_terms_score.saturating_add(score);
            }

            if !signed_terms_rankable {
                continue;
            }

            /*
             * Modifier-only queries have no ordinary fuzzy text.
             *
             * Every structurally eligible record receives an equal base score;
             * directories retain their established priority.
             */
            if folded_query.is_empty() || folded_query == "." {
                retain_ranked_match(
                    &mut best_matches,
                    RankedMatch {
                        entry_index: record.entry_index,

                        is_directory: record.is_directory,

                        score: signed_terms_score.saturating_add(boolean_score),
                    },
                    result_limit,
                );
            } else {
                /*
                 * If the query is longer than the complete searchable path by
                 * more than the permitted typo distance, no scoring route can
                 * succeed.
                 */
                if folded_query.len() > record.path_length as usize + maximum_typo_distance {
                    continue;
                }

                /*
                 * Reject candidates missing too many query characters before
                 * invoking subsequence or edit-distance scoring.
                 */
                if query_mask != 0 {
                    let missing_characters =
                        (query_mask & !record.character_mask).count_ones() as usize;

                    if missing_characters > maximum_typo_distance {
                        continue;
                    }
                }

                let Some(base_score) = score_candidate(
                    &record.searchable_name,
                    &record.searchable_path,
                    &folded_query,
                ) else {
                    continue;
                };

                let score = base_score
                    .saturating_add(signed_terms_score)
                    .saturating_add(boolean_score);

                retain_ranked_match(
                    &mut best_matches,
                    RankedMatch {
                        entry_index: record.entry_index,

                        is_directory: record.is_directory,

                        score,
                    },
                    result_limit,
                );
            }

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

                        limit_reached: false,
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

                limit_reached: false,
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

            limit_reached: false,
        });
    });

    receiver
}

/*
 * Every argument describes an independent part of one Exact worker request.
 *
 * Keeping them explicit mirrors start_fuzzy_worker() and avoids introducing a
 * one-use configuration structure that would merely move these fields elsewhere.
 */
#[allow(clippy::too_many_arguments)]
pub fn start_exact_worker(
    index: Arc<SearchIndex>,
    parsed_query: ParsedQuery,
    generation: u64,
    show_hidden: bool,
    hidden_only: bool,
    scope_prefix: Option<String>,
    entry_filter: WorkerEntryFilter,
    result_limit: Option<usize>,
    sort_mode: SortMode,
    sort_descending: bool,
    cancel_signal: Arc<AtomicBool>,
) -> Receiver<FuzzyWorkerResult> {
    let (sender, receiver) = mpsc::channel();

    thread::spawn(move || {
        let exact_text = parsed_query.search_text();

        let total = index.len();

        let mut matching_indices = Vec::new();

        /*
         * This becomes true only after the worker encounters an eligible match that
         * cannot be retained because the caller's Exact result limit is already full.
         */
        let mut limit_reached = false;

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

                    limit_reached,
                });

                return;
            }

            /*
             * Restrict a host-wide persistent index to the currently selected
             * recursive root.
             */
            if let Some(scope_prefix) = scope_prefix.as_deref()
                && !scope_prefix.is_empty()
                && record.searchable_path.as_ref() != scope_prefix
                && !record
                    .searchable_path
                    .strip_prefix(scope_prefix)
                    .is_some_and(|suffix| suffix.starts_with('/'))
            {
                continue;
            }

            if hidden_only {
                if !record.contains_hidden_component {
                    continue;
                }
            } else if !show_hidden && record.contains_hidden_component {
                continue;
            }

            if !entry_filter.matches(record.is_directory, record.is_symlink) {
                continue;
            }

            if !record_matches_query_filters(record, &parsed_query) {
                continue;
            }

            /*
             * Exact mode preserves its ordinary substring semantics.
             *
             * Modifier-only queries have no unsigned text, so every entry that
             * passed the structured filters is a match.
             */
            if !exact_text.is_empty()
                && exact_text != "."
                && !record.searchable_path.contains(exact_text)
            {
                continue;
            }

            /*
             * Exact List mode passes None and therefore retains every match.
             *
             * Exact Tree mode passes a bounded limit because constructing a contextual
             * hierarchy from hundreds of thousands of matches is not useful and can block
             * the terminal event thread for several seconds.
             *
             * Continue scanning after the limit has been filled so progress, cancellation,
             * and the examined count remain accurate.
             */
            match result_limit {
                Some(limit) if matching_indices.len() >= limit => {
                    /*
                     * Continue scanning the corpus so progress and cancellation remain
                     * accurate, but remember that at least one additional match existed.
                     */
                    limit_reached = true;
                }

                _ => {
                    matching_indices.push(record.entry_index);
                }
            }

            /*
             * Publish a bounded preview throughout the complete traversal.
             *
             * A broad Exact query may exceed the preview limit almost immediately.
             * Continuing to publish the first bounded result page prevents the previous
             * query's results from remaining onscreen until a million-entry scan finishes.
             *
             * Only at most EXACT_PROGRESS_RESULT_LIMIT indices are cloned per update.
             */
            if last_progress.elapsed() >= PROGRESS_INTERVAL {
                let preview_length = matching_indices.len().min(EXACT_PROGRESS_RESULT_LIMIT);

                let preview_indices = matching_indices[..preview_length].to_vec();

                if sender
                    .send(FuzzyWorkerResult {
                        generation,

                        indices: preview_indices,

                        examined: position.saturating_add(1),

                        total,

                        finished: false,

                        cancelled: false,

                        limit_reached,
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

                limit_reached,
            });

            return;
        }

        /*
         * The backing index remains in stable scanner order.
         *
         * Sort only the matching entry indices, here on the worker thread. Changing
         * Exact/Fuzzy mode or Reverse therefore never needs to rearrange the complete
         * recursive FileEntry corpus or rebuild SearchIndex on the UI thread.
         */
        matching_indices.sort_unstable_by(|left_index, right_index| {
            let Some(left) = index.records().get(*left_index) else {
                return ComparisonOrdering::Equal;
            };

            let Some(right) = index.records().get(*right_index) else {
                return ComparisonOrdering::Equal;
            };

            compare_exact_records(left, right, sort_mode, sort_descending)
        });

        let _ = sender.send(FuzzyWorkerResult {
            generation,

            indices: matching_indices,

            examined: total,

            total,

            finished: true,

            cancelled: false,

            limit_reached,
        });
    });

    receiver
}

fn compare_exact_records(
    left: &SearchRecord,
    right: &SearchRecord,
    sort_mode: SortMode,
    descending: bool,
) -> ComparisonOrdering {
    /*
     * Directories always remain above ordinary files, regardless of direction.
     */
    match (left.is_directory, right.is_directory) {
        (true, false) => {
            return ComparisonOrdering::Less;
        }

        (false, true) => {
            return ComparisonOrdering::Greater;
        }

        _ => {}
    }

    /*
     * Match scan::sort_entries():
     *
     * directories use their paths for every sort mode, while files use the
     * selected metadata and fall back to path for deterministic ordering.
     */
    let ordering = if left.is_directory && right.is_directory {
        left.searchable_path.cmp(&right.searchable_path)
    } else {
        let primary_ordering = match sort_mode {
            SortMode::Name => left.searchable_path.cmp(&right.searchable_path),

            SortMode::Size => left.size_bytes.cmp(&right.size_bytes),

            SortMode::Modified => left.modified_time.cmp(&right.modified_time),

            SortMode::Type => left.class.label().cmp(right.class.label()),
        };

        primary_ordering.then_with(|| left.searchable_path.cmp(&right.searchable_path))
    };

    if descending {
        ordering.reverse()
    } else {
        ordering
    }
}

fn retain_ranked_match(
    matches: &mut BinaryHeap<Reverse<RankedMatch>>,
    candidate: RankedMatch,
    result_limit: usize,
) {
    if matches.len() < result_limit {
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

    /*
     * Prefer filename matches, but do not let location outweigh substantially
     * better textual similarity.
     *
     * An obvious typo correction in a path component, such as:
     *
     *     hlep -> help
     *
     * should rank above a loosely scattered filename match such as:
     *
     *     shlex.py
     */
    let mut best_score = score_component(name, query).map(|score| score + 600);

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

/*
 * Find a contiguous candidate substring produced by correcting one adjacent
 * transposition in the query.
 *
 * Examples:
 *
 *     coed  -> code
 *     cdoe  -> code
 *     hlep  -> help
 *     hlelo -> hello
 *
 * Unlike general subsequence matching, every corrected character must remain
 * contiguous inside one path component. This allows typo-corrected words to
 * match larger names such as `pycodestyle` without admitting widely scattered
 * character combinations.
 */
fn adjacent_transposition_substring_position(candidate: &[u8], query: &[u8]) -> Option<usize> {
    if query.len() < 2 || candidate.len() < query.len() {
        return None;
    }

    let mut best_position = None;

    for swap_index in 0..query.len().saturating_sub(1) {
        /*
         * Swapping identical adjacent characters would reproduce the original
         * query and therefore adds no new fuzzy interpretation.
         */
        if query[swap_index] == query[swap_index + 1] {
            continue;
        }

        for (position, window) in candidate.windows(query.len()).enumerate() {
            let matches = window.iter().enumerate().all(|(index, character)| {
                let expected = if index == swap_index {
                    query[swap_index + 1]
                } else if index == swap_index + 1 {
                    query[swap_index]
                } else {
                    query[index]
                };

                *character == expected
            });

            if matches {
                best_position = Some(
                    best_position
                        .map(|current: usize| current.min(position))
                        .unwrap_or(position),
                );
            }
        }
    }

    best_position
}

/*
 * Find a contiguous candidate substring produced by inserting exactly one
 * missing character into the query.
 *
 * Examples:
 *
 *     REDME -> README     -> README.md
 *     helo  -> hello      -> hello_world.txt
 *     sorce -> source     -> source-code.rs
 *
 * Embedded replacements are deliberately not accepted here. Allowing any
 * same-length substring one edit away would reintroduce accidental matches such
 * as:
 *
 *     hlep  -> hlex       -> shlex.py
 *     hlelo -> hlelf      -> shlelf_nto.xwe
 *
 * Equal-length transpositions are already handled by the dedicated adjacent
 * transposition matcher, while whole-component replacements remain available
 * through typo_score().
 *
 * The returned values are:
 *
 *     byte position
 *     matched window length
 *     edit distance
 */
/*
 * Test whether removing exactly one byte from a candidate window makes the
 * remaining bytes equal the query, optionally after correcting one adjacent
 * transposition in the query.
 *
 * No allocation is performed. Every remaining candidate byte is compared
 * directly with its expected query byte.
 */
fn matches_after_one_removal(
    window: &[u8],
    query: &[u8],
    removed_index: usize,
    swap_index: Option<usize>,
) -> bool {
    if window.len() != query.len().saturating_add(1) || removed_index >= window.len() {
        return false;
    }

    for query_index in 0..query.len() {
        let window_index = if query_index < removed_index {
            query_index
        } else {
            query_index.saturating_add(1)
        };

        let expected = match swap_index {
            Some(swap_index) if query_index == swap_index => query[swap_index + 1],

            Some(swap_index) if query_index == swap_index + 1 => query[swap_index],

            _ => query[query_index],
        };

        if window[window_index] != expected {
            return false;
        }
    }

    true
}

/*
 * Find a corrected contiguous candidate word containing exactly one character
 * omitted from the query.
 *
 * Two narrowly defined forms are accepted:
 *
 *     REDME -> README
 *     REDEM -> REDME -> README
 *
 * The second form combines one missing character with one adjacent
 * transposition. General distance-two replacements remain excluded so noisy
 * matches such as arbitrary altered substrings are not reintroduced.
 *
 * The returned values are:
 *
 *     byte position
 *     matched window length
 *     edit cost: 1 for omission only, 2 for omission plus transposition
 */
fn missing_character_substring_match(
    candidate: &[u8],
    query: &[u8],
) -> Option<(usize, usize, usize)> {
    if query.len() < 3 {
        return None;
    }

    let window_length = query.len().saturating_add(1);

    if candidate.len() < window_length {
        return None;
    }

    for (position, window) in candidate.windows(window_length).enumerate() {
        /*
         * First accept an ordinary one-character omission.
         */
        for removed_index in 0..window.len() {
            if matches_after_one_removal(window, query, removed_index, None) {
                return Some((position, window_length, 1));
            }
        }

        /*
         * Then accept one omission combined with exactly one adjacent swap.
         *
         * Swapping identical bytes changes nothing and is therefore skipped.
         */
        for swap_index in 0..query.len().saturating_sub(1) {
            if query[swap_index] == query[swap_index + 1] {
                continue;
            }

            for removed_index in 0..window.len() {
                if matches_after_one_removal(window, query, removed_index, Some(swap_index)) {
                    return Some((position, window_length, 2));
                }
            }
        }
    }

    None
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

    /*
     * A corrected adjacent transposition may occur inside a larger component:
     *
     *     coed -> code -> pycodestyle
     *
     * Keep this below an ordinary contiguous substring but above general
     * whole-component edit-distance matching.
     */
    if let Some(position) =
        adjacent_transposition_substring_position(candidate.as_bytes(), query.as_bytes())
    {
        return Some(5_500 - position as i64);
    }

    /*
     * Permit one corrected contiguous word inside a larger component:
     *
     *     REDME -> README -> README.md
     *
     * Keep this below the specialized adjacent-transposition route, but above the
     * ordinary whole-component typo fallback.
     */
    if let Some((position, window_length, distance)) =
        missing_character_substring_match(candidate.as_bytes(), query.as_bytes())
    {
        return Some(
            5_300
                - distance as i64 * 300
                - window_length.abs_diff(query.len()) as i64 * 40
                - position as i64 * 10,
        );
    }

    typo_score(candidate.as_bytes(), query.as_bytes())
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

    if let Some(position) =
        adjacent_transposition_substring_position(candidate.as_bytes(), query.as_bytes())
    {
        /*
         * Highlight the complete corrected substring.
         *
         * The returned position is a byte offset. This is safe for the ASCII query
         * spellings handled by the byte-based fuzzy scorer.
         */
        return (position..position.saturating_add(query.len())).collect();
    }

    if let Some((position, window_length, _distance)) =
        missing_character_substring_match(candidate.as_bytes(), query.as_bytes())
    {
        /*
         * Highlight the complete corrected contiguous word.
         *
         * For REDME matched against README.md, this paints README rather than the
         * entire filename or only the five typed characters.
         */
        return (position..position.saturating_add(window_length)).collect();
    }

    if typo_score(candidate.as_bytes(), query.as_bytes()).is_some() {
        /*
         * For help matched by hlpe, hlep, or hepl, highlight the complete component.
         *
         * Inserted, removed, replaced, and transposed characters do not have one
         * reliable character-for-character highlight mapping.
         */
        return (0..candidate.chars().count()).collect();
    }

    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::{RankedMatch, score_component};

    #[test]
    fn directories_form_the_first_fuzzy_result_group() {
        let strong_file = RankedMatch {
            entry_index: 1,

            is_directory: false,

            score: 10_000,
        };

        let weak_directory = RankedMatch {
            entry_index: 2,

            is_directory: true,

            score: 4_000,
        };

        assert!(weak_directory > strong_file);
    }

    #[test]
    fn relevance_orders_results_inside_the_same_group() {
        let weaker_directory = RankedMatch {
            entry_index: 1,

            is_directory: true,

            score: 4_000,
        };

        let stronger_directory = RankedMatch {
            entry_index: 2,

            is_directory: true,

            score: 8_000,
        };

        let weaker_file = RankedMatch {
            entry_index: 3,

            is_directory: false,

            score: 4_000,
        };

        let stronger_file = RankedMatch {
            entry_index: 4,

            is_directory: false,

            score: 8_000,
        };

        assert!(stronger_directory > weaker_directory);

        assert!(stronger_file > weaker_file);
    }

    #[test]
    fn transposed_query_matches_corrected_word_inside_larger_component() {
        assert!(score_component("pycodestyle", "coed").is_some());

        assert!(score_component("sourcecodelookup", "cdoe").is_some());

        assert!(score_component("secure_code", "coed").is_some());
    }

    #[test]
    fn multiple_adjacent_transposition_forms_find_the_same_word() {
        assert!(score_component("code", "coed").is_some());

        assert!(score_component("code", "cdoe").is_some());

        assert!(score_component("help", "hlep").is_some());

        assert!(score_component("hello", "hlelo").is_some());
    }

    #[test]
    fn transposed_substring_matching_does_not_restore_scattered_results() {
        assert!(score_component("shlelf_nto.xwe", "hlelo").is_none());

        assert!(score_component("hook-google-cloud-bigquery.py", "hlelo").is_none());
    }

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
    fn scattered_filename_characters_are_rejected() {
        assert!(score_component("shlex.py", "hlep").is_none());

        assert!(score_component("shlelf_nto.xwe", "hlelo").is_none());
    }

    #[test]
    fn unrelated_long_component_is_rejected() {
        assert!(score_component("hook-google-cloud-bigquery.py", "hlelo").is_none());
    }

    #[test]
    fn replacement_typo_matches() {
        assert!(score_component("help", "halp").is_some());
    }

    #[test]
    fn widely_scattered_match_is_rejected() {
        assert!(score_component("columnexperimentwithfont", "cpuf").is_none());
    }

    #[test]
    fn unrelated_component_is_rejected() {
        assert!(score_component("deleteaction", "tstf").is_none());
    }

    #[test]
    fn inserted_character_typo_matches() {
        assert!(score_component("hello", "hlelo").is_some());
    }

    #[test]
    fn complete_prefix_remains_a_match() {
        assert!(score_component("cpuforge", "cpuf").is_some());
    }

    #[test]
    fn missing_character_typo_matches_word_inside_larger_component() {
        assert!(score_component("README.md", "REDME").is_some());

        assert!(score_component("README.txt", "REDME").is_some());

        assert!(score_component("README.rst", "REDME").is_some());

        assert!(score_component("README-old.md", "REDME").is_some());
    }

    #[test]
    fn missing_query_character_matches_word_inside_larger_component() {
        assert!(score_component("hello_world.txt", "helo").is_some());

        assert!(score_component("source-code.rs", "sorce").is_some());

        assert!(score_component("README.md", "REDME").is_some());
    }

    #[test]
    fn sensitive_typo_matching_does_not_ignore_case() {
        assert!(score_component("README.md", "REDME").is_some());

        assert!(score_component("readme.md", "REDME").is_none());
    }

    #[test]
    fn missing_character_typo_matches_readme_family() {
        for candidate in ["readme", "readme.md", "readme.txt", "project-readme.html"] {
            assert!(
                score_component(candidate, "redme").is_some(),
                "simple omission failed for {candidate}",
            );
        }
    }

    #[test]
    fn missing_character_plus_transposition_matches_readme_family() {
        for candidate in ["readme", "readme.md", "readme.txt", "project-readme.html"] {
            assert!(
                score_component(candidate, "redem").is_some(),
                "compound README typo failed for {candidate}",
            );
        }
    }

    #[test]
    fn compound_typo_support_does_not_admit_arbitrary_two_edit_substrings() {
        for candidate in ["shlex.py", "shlelf_nto.xwe", "random-redox.md"] {
            assert!(
                score_component(candidate, "redem").is_none(),
                "unrelated two-edit substring was accepted: {candidate}",
            );
        }
    }

    #[test]
    fn single_edit_substring_matching_does_not_restore_scattered_noise() {
        assert!(score_component("shlelf_nto.xwe", "hlelo").is_none());

        assert!(score_component("hook-google-cloud-bigquery.py", "hlelo",).is_none());
    }
}
