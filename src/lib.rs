#![warn(clippy::pedantic)]
#![allow(clippy::too_many_lines, clippy::cast_precision_loss)]
mod builder;
mod matches;
mod replacer;
pub mod structs;
#[cfg(test)]
mod tests;

pub use builder::FuzzyAhoCorasickBuilder;
pub use replacer::FuzzyReplacer;
use std::borrow::Cow;
use std::collections::HashMap;
use unicode_segmentation::UnicodeSegmentation;
pub type PatternIndex = usize;
pub use structs::*;

#[allow(unused_macros)]
#[cfg(test)]
macro_rules! trace {
    ($($arg:tt)*) => { println!($($arg)*); };
}
#[allow(unused_macros)]
#[cfg(not(test))]
macro_rules! trace {
    ($($arg:tt)*) => {};
}
/// Fuzzy Aho—Corasick engine
impl FuzzyAhoCorasick {
    /// Get the per-node limits if this node corresponds to a pattern that has
    /// its own `FuzzyLimits`.
    #[inline]
    fn get_node_limits(&self, node: usize) -> Option<&FuzzyLimits> {
        self.nodes[node]
            .pattern_index
            .and_then(|i| self.patterns.get(i).and_then(|p| p.limits.as_ref()))
    }

    /// Fast path similarity lookup with inline handling of common cases.
    #[inline(always)]
    fn get_similarity(&self, a: char, b: char) -> f32 {
        // Fast path: exact match
        if a == b {
            return 1.0;
        }
        // Look up in similarity table
        *self.similarity.get(&(a, b)).unwrap_or(&0.0)
    }

    /// Check ahead whether an insertion would stay within the allowed limits.
    /// Considers both the node-specific limits and the global fallback `self.limits`.
    #[inline]
    fn within_limits_insertion_ahead(
        &self,
        limits: Option<&FuzzyLimits>,
        edits: NumEdits,
        insertions: NumEdits,
    ) -> bool {
        if let Some(max) = limits.or(self.limits.as_ref()) {
            max.edits.is_none_or(|max| edits < max)
                && max.insertions.is_none_or(|max| insertions < max)
        } else {
            false
        }
    }

    /// Check ahead whether a deletion would stay within the allowed limits.
    #[inline]
    fn within_limits_deletion_ahead(
        &self,
        limits: Option<&FuzzyLimits>,
        edits: NumEdits,
        deletions: NumEdits,
    ) -> bool {
        if let Some(max) = limits.or(self.limits.as_ref()) {
            max.edits.is_none_or(|max| edits < max)
                && max.deletions.is_none_or(|max| deletions < max)
        } else {
            false
        }
    }

    /// Check ahead whether a swap (transposition) would stay within the allowed limits.
    #[inline]
    fn within_limits_swap_ahead(
        &self,
        limits: Option<&FuzzyLimits>,
        edits: NumEdits,
        swaps: NumEdits,
    ) -> bool {
        if let Some(max) = limits.or(self.limits.as_ref()) {
            /*println!(
                "within_limits_swap_ahead() -- max: {max:?} edits: {edits:?} swaps: {swaps:?}\
                \nresult = {:?}\n"
            , max.edits.is_none_or(|max| edits < max) && max.swaps.is_none_or(|max| swaps < max))*/
            max.edits.is_none_or(|max| edits < max) && max.swaps.is_none_or(|max| swaps < max)
        } else {
            false
        }
    }

    /// Check ahead whether a substitution would stay within the allowed limits.
    #[inline]
    fn within_limits_subst(
        &self,
        limits: Option<&FuzzyLimits>,
        edits: NumEdits,
        substitutions: NumEdits,
    ) -> bool {
        if let Some(max) = limits.or(self.limits.as_ref()) {
            /*println!(
                "within_limits_subst_ahead() -- max: {max:?} edits: {edits:?} substitutions: {substitutions:?}\
                \nresult = {result:?}\n"
            );*/
            max.edits.is_none_or(|max| edits <= max)
                && max.substitutions.is_none_or(|max| substitutions <= max)
        } else {
            edits == 0 && substitutions == 0
        }
    }

    /// General limits check: given all edit counts, returns whether they are
    /// acceptable under either the node-specific limits or the global default.
    #[inline]
    fn within_limits(
        &self,
        limits: Option<&FuzzyLimits>,
        edits: NumEdits,
        insertions: NumEdits,
        deletions: NumEdits,
        substitutions: NumEdits,
        swaps: NumEdits,
    ) -> bool {
        if let Some(max) = limits.or(self.limits.as_ref()) {
            /*println!(
                "within_limits() -- max: {max:?} edits: {edits:?} insertions: {insertions:?} deletions: {deletions:?} substitutions: {substitutions:?} swaps: {swaps:?}\
                \nresult = {result:?}\n"
            );*/
            max.edits.is_none_or(|max| edits <= max)
                && max.insertions.is_none_or(|max| insertions <= max)
                && max.deletions.is_none_or(|max| deletions <= max)
                && max.substitutions.is_none_or(|max| substitutions <= max)
                && max.swaps.is_none_or(|max| swaps <= max)
        } else {
            edits == 0 && insertions == 0 && deletions == 0 && substitutions == 0 && swaps == 0
        }
    }

    /// Returns the list of patterns the automaton was built with.
    #[must_use]
    pub fn patterns(&self) -> &[Pattern] {
        &self.patterns
    }

    /// Core fuzzy search over the haystack producing raw matches without any
    /// global ordering applied. This explores all possible state transitions
    /// (substitutions, swaps, insertions, deletions) starting at each grapheme
    /// position, accumulating penalties and enforcing per-pattern limits. Keeps the
    /// best match for each unique (`start_byte`, `end_byte`, `pattern_index`) key by
    /// highest similarity, but does **not** sort the results; the returned
    /// `FuzzyMatches.inner` is effectively unsorted.
    ///
    /// Similarity is computed as `(total_graphemes - penalties) / total_graphemes * weight`.
    /// Matches below `similarity_threshold` are discarded early.
    ///
    /// # Parameters
    /// - `haystack`: the input text to search in.
    /// - `similarity_threshold`: minimum similarity a candidate must have to be kept.
    ///
    /// # Returns
    /// A `FuzzyMatches` containing the best per-span matches meeting the threshold.
    #[inline]
    #[must_use]
    pub fn search_unsorted<'a>(
        &'a self,
        haystack: &'a str,
        similarity_threshold: f32,
    ) -> FuzzyMatches<'a> {
        let grapheme_idx: Vec<(usize, &str)> = haystack.grapheme_indices(true).collect();
        if grapheme_idx.is_empty() {
            return FuzzyMatches {
                haystack,
                inner: vec![],
            };
        }
        let text_chars: Vec<Cow<str>> = grapheme_idx
            .iter()
            .map(|(_, g)| {
                if self.case_insensitive {
                    Cow::Owned(g.to_lowercase())
                } else {
                    Cow::Borrowed(*g)
                }
            })
            .collect();

        // Use HashMap for O(1) lookup instead of BTreeMap's O(log n)
        let mut best: HashMap<(usize, usize, usize), FuzzyMatch> =
            HashMap::with_capacity(self.patterns.len() * 4);

        // Pre-allocate queue - size based on beam width or a small default
        let mut queue: Vec<State> = Vec::with_capacity(self.beam_width.unwrap_or(64));

        trace!(
            "=== fuzzy_search on {haystack:?} (similarity_threshold {similarity_threshold:.2}) ===",
        );
        for start in 0..text_chars.len() {
            trace!(
                "=== new window at grapheme #{start} ({:?}) ===",
                text_chars[start]
            );

            queue.clear();
            queue.push(State {
                node: 0,
                j: start,
                matched_start: start,
                matched_end: start,
                penalties: 0.,
                edits: 0,
                insertions: 0,
                deletions: 0,
                substitutions: 0,
                swaps: 0,
                #[cfg(debug_assertions)]
                notes: vec![],
            });

            let mut q_idx = 0;
            while q_idx < queue.len() {
                // Beam pruning: if queue grows too large, keep only best candidates
                if let Some(bw) = self.beam_width {
                    let remaining = queue.len() - q_idx;
                    if remaining > bw * 2 {
                        // Sort remaining items by penalties (lowest first = best candidates)
                        queue[q_idx..].sort_by(|a, b| a.penalties.total_cmp(&b.penalties));
                        // Keep only beam_width items from q_idx onward
                        queue.truncate(q_idx + bw);
                    }
                }
                let State {
                    node,
                    j,
                    matched_start,
                    matched_end,
                    penalties,
                    edits,
                    insertions,
                    deletions,
                    substitutions,
                    swaps,
                    ..
                } = queue[q_idx];
                #[cfg(debug_assertions)]
                let notes = queue[q_idx].notes.clone();
                q_idx += 1;

                // Early pruning: check if this state can possibly produce a match
                // above the threshold. Use the maximum pattern length as the best case.
                let max_possible_similarity = (self.max_pattern_grapheme_len as f32 - penalties)
                    / self.max_pattern_grapheme_len as f32;
                if max_possible_similarity < similarity_threshold {
                    continue;
                }

                let Node {
                    output,
                    transitions,
                    ..
                } = &self.nodes[node];

                if !output.is_empty() {
                    for &pattern_index in output {
                        if !self.within_limits(
                            self.patterns[pattern_index].limits.as_ref(),
                            edits,
                            insertions,
                            deletions,
                            substitutions,
                            swaps,
                        ) {
                            continue;
                        }
                        let start_byte = grapheme_idx.get(matched_start).map_or(0, |&(b, _)| b);
                        let end_byte = grapheme_idx
                            .get(matched_end)
                            .map_or_else(|| haystack.len(), |&(b, _)| b);
                        let key = (start_byte, end_byte, pattern_index);

                        let total = self.patterns[pattern_index].grapheme_len as f32;

                        let similarity =
                            (total - penalties) / total * self.patterns[pattern_index].weight;

                        if similarity < similarity_threshold {
                            continue;
                        }

                        best.entry(key)
                            .and_modify(|entry| {
                                if similarity > entry.similarity {
                                    *entry = FuzzyMatch {
                                        insertions,
                                        deletions,
                                        substitutions,
                                        edits,
                                        swaps,
                                        pattern_index,
                                        start: start_byte,
                                        end: end_byte,
                                        pattern: &self.patterns[pattern_index],
                                        similarity,
                                        text: &haystack[start_byte..end_byte],
                                        #[cfg(debug_assertions)]
                                        notes: notes.clone(),
                                    };
                                }
                            })
                            .or_insert_with(|| FuzzyMatch {
                                insertions,
                                deletions,
                                substitutions,
                                edits,
                                swaps,
                                pattern_index,
                                start: start_byte,
                                end: end_byte,
                                pattern: &self.patterns[pattern_index],
                                similarity,
                                text: &haystack[start_byte..end_byte],
                                #[cfg(debug_assertions)]
                                notes: notes.clone(),
                            });
                    }
                }

                //
                // 1) Same or similar symbol — только внутри текста
                //
                if j < text_chars.len() {
                    let current_grapheme = text_chars[j].as_ref();
                    let current_ch = current_grapheme.chars().next().unwrap_or('\0');

                    for (edge_g, &next_node) in transitions {
                        #[cfg(debug_assertions)]
                        let notes = notes.clone();

                        let g_ch = edge_g.chars().next().unwrap_or('\0');
                        if edge_g == current_grapheme {
                            // exact match
                            trace!("  match   {:>8} ─ok→ node={}  sim=1.00", edge_g, next_node);
                            queue.push(State {
                                node: next_node,
                                j: j + 1,
                                matched_start: if matched_end == matched_start {
                                    j
                                } else {
                                    matched_start
                                },
                                matched_end: j + 1,
                                penalties,
                                edits,
                                insertions,
                                deletions,
                                substitutions,
                                swaps,
                                #[cfg(debug_assertions)]
                                notes,
                            });
                        } else if self.within_limits_subst(
                            self.get_node_limits(node),
                            edits,
                            substitutions,
                        ) {
                            // substitution
                            let sim = self.get_similarity(g_ch, current_ch);
                            let penalty = self.penalties.substitution * (1.0 - sim);

                            trace!(
                                "  subst {:>8?} ─sub→ {current_grapheme:?} \
                                 node={}  sim={:.2} pen={:.2} edits->{}",
                                edge_g,
                                next_node,
                                sim,
                                penalty,
                                edits + 1
                            );
                            #[cfg(debug_assertions)]
                            let mut notes = notes.clone();
                            #[cfg(debug_assertions)]
                            notes.push(format!("sub {edge_g:?} -> {current_grapheme:?} (sim={sim:.2}, pen={penalty:.2}) (subst->{}, edits->{})", substitutions + 1, edits + 1));

                            queue.push(State {
                                node: next_node,
                                j: j + 1,
                                matched_start: if matched_end == matched_start {
                                    j
                                } else {
                                    matched_start
                                },
                                matched_end: j + 1,
                                penalties: penalties + penalty,
                                edits: edits + 1,
                                insertions,
                                deletions,
                                substitutions: substitutions + 1,
                                swaps,
                                #[cfg(debug_assertions)]
                                notes,
                            });
                        }
                    }

                    //
                    // 2) Swap (transposition of two neighboring graphemes)
                    //
                    if j + 1 < text_chars.len() {
                        let a = &text_chars[j];
                        let b = &text_chars[j + 1];
                        if let Some(&node2) = transitions
                            .get(b.as_ref())
                            .and_then(|&x| self.nodes[x].transitions.get(a.as_ref()))
                        {
                            if self.within_limits_swap_ahead(
                                self.get_node_limits(node2),
                                edits,
                                swaps,
                            ) {
                                #[cfg(debug_assertions)]
                                let mut notes = notes.clone();
                                #[cfg(debug_assertions)]
                                notes.push(format!(
                                    "swap a:{a:?} b:{b:?} (swaps->{}, edits->{})",
                                    swaps + 1,
                                    edits + 1
                                ));
                                queue.push(State {
                                    node: node2,
                                    j: j + 2,
                                    matched_start,
                                    matched_end: j + 2,
                                    penalties: penalties + self.penalties.swap,
                                    edits: edits + 1,
                                    insertions,
                                    deletions,
                                    substitutions,
                                    swaps: swaps + 1,
                                    #[cfg(debug_assertions)]
                                    notes,
                                });
                            }
                        }
                    }

                    //
                    // 3a) Insertion (skip a haystack character)
                    //
                    if (matched_start != matched_end || matched_start != j)
                        && self.within_limits_insertion_ahead(
                            self.get_node_limits(node),
                            edits,
                            insertions,
                        )
                    {
                        #[cfg(debug_assertions)]
                        let mut notes = notes.clone();
                        #[cfg(debug_assertions)]
                        notes.push(format!(
                            "ins {:?} (ins->{} , edits->{})",
                            text_chars[j],
                            insertions + 1,
                            edits + 1
                        ));
                        queue.push(State {
                            node,
                            j: j + 1,
                            matched_start,
                            matched_end,
                            penalties: penalties + self.penalties.insertion,
                            edits: edits + 1,
                            insertions: insertions + 1,
                            deletions,
                            substitutions,
                            swaps,
                            #[cfg(debug_assertions)]
                            notes,
                        });
                    }
                }

                //
                // 3b) Deletion (skip a pattern character) — always, even if j == len
                //
                if self.within_limits_deletion_ahead(self.get_node_limits(node), edits, deletions) {
                    #[allow(unused_variables)]
                    for (edge_g2, &next_node2) in transitions {
                        trace!(
                            "  delete to node={next_node2} penalty={:.2}",
                            self.penalties.deletion
                        );
                        #[cfg(debug_assertions)]
                        let mut notes = notes.clone();
                        #[cfg(debug_assertions)]
                        notes.push(format!("edge_g2 {edge_g2:?} (del->{:?})", deletions + 1));
                        queue.push(State {
                            node: next_node2,
                            j,
                            matched_start,
                            matched_end,
                            penalties: penalties + self.penalties.deletion,
                            edits: edits + 1,
                            insertions,
                            deletions: deletions + 1,
                            substitutions,
                            swaps,
                            #[cfg(debug_assertions)]
                            notes,
                        });
                    }
                }
            }
        }
        FuzzyMatches {
            haystack,
            inner: best
                .into_values()
                .map(|mut m| {
                    m.text = &haystack[m.start..m.end];
                    m
                })
                .collect(),
        }
    }

    /// Convenience wrapper over `search_unsorted` that applies the default sorting
    /// order to the matches (via `default_sort()`).
    ///
    /// # Parameters
    /// - `haystack`: the input text to search in.
    /// - `similarity_threshold`: minimum similarity threshold for candidates.
    ///
    /// # Returns
    /// `FuzzyMatches` with matches sorted according to the default ranking.
    #[inline]
    #[must_use]
    pub fn search<'a>(&'a self, haystack: &'a str, similarity_threshold: f32) -> FuzzyMatches<'a> {
        let mut matches = self.search_unsorted(haystack, similarity_threshold);
        matches.default_sort();
        matches
    }

    /// Convenience wrapper over `search_unsorted` that applies a greedy sort (via `greedy_sort()`),
    ///
    /// # Parameters
    /// - `haystack`: the input text to search in.
    /// - `similarity_threshold`: minimum similarity threshold for candidates.
    ///
    /// # Returns
    /// `FuzzyMatches` with matches sorted by the greedy heuristic.
    #[inline]
    #[must_use]
    pub fn search_greedy<'a>(
        &'a self,
        haystack: &'a str,
        similarity_threshold: f32,
    ) -> FuzzyMatches<'a> {
        let mut matches = self.search_unsorted(haystack, similarity_threshold);
        matches.greedy_sort();
        matches
    }

    /// Search that returns non-overlapping matches by delegating to
    /// `non_overlapping()` on the fully sorted (default) results. This will
    /// greedily select a set of matches such that their spans do not overlap,
    /// according to whatever strategy `non_overlapping` encapsulates.
    ///
    /// # Parameters
    /// - `haystack`: the input text to search in.
    /// - `similarity_threshold`: minimum similarity threshold for candidates.
    ///
    /// # Returns
    /// `FuzzyMatches` containing a non-overlapping subset of matches.
    #[must_use]
    pub fn search_non_overlapping<'a>(
        &'a self,
        haystack: &'a str,
        similarity_threshold: f32,
    ) -> FuzzyMatches<'a> {
        let mut matches = self.search(haystack, similarity_threshold);
        matches.non_overlapping();
        matches
    }

    /// Variation of `search_non_overlapping` that additionally enforces uniqueness
    /// of patterns: each pattern (identified by `custom_unique_id` if present or by
    /// its index) may only contribute one accepted match. Delegates to
    /// `non_overlapping_unique()` after obtaining the base sorted matches.
    ///
    /// # Parameters
    /// - `haystack`: the input text to search in.
    /// - `similarity_threshold`: minimum similarity threshold for candidates.
    ///
    /// # Returns
    /// `FuzzyMatches` containing a non-overlapping, pattern-unique subset of matches.
    #[must_use]
    pub fn search_non_overlapping_unique<'a>(
        &'a self,
        haystack: &'a str,
        similarity_threshold: f32,
    ) -> FuzzyMatches<'a> {
        let mut matches = self.search(haystack, similarity_threshold);
        matches.non_overlapping_unique();
        matches
    }

    /// Perform replacements on `text` by finding non-overlapping fuzzy matches above
    /// `threshold` and invoking `callback` for each. Matches are resolved via
    /// `search_non_overlapping`, so they won’t overlap; the first chosen set is
    /// used in left-to-right order.
    ///
    /// The `callback` is called with each `FuzzyMatch`. If it returns `Some(repl)`,
    /// the matched span is replaced with `repl`. If it returns `None`, the original
    /// substring from `text` is preserved.
    ///
    /// # Parameters
    /// - `text`: input string to perform replacements on.
    /// - `callback`: mapping from a `FuzzyMatch` to an optional replacement string.
    /// - `threshold`: minimum similarity for a match to be considered.
    ///
    /// # Returns
    /// A new `String` with the selected fuzzy matches replaced per `callback`.
    ///
    /// # Example
    /// ```rust
    /// use fuzzy_aho_corasick::FuzzyAhoCorasickBuilder;
    /// let automaton = FuzzyAhoCorasickBuilder::new().build(["FOO", "BAR", "BAZ"]);
    /// let result = automaton.replace("FOO BAR BAZ", |m| {
    ///     (m.pattern.pattern == "BAR").then_some("###")
    /// }, 0.8);
    /// assert_eq!(result, "FOO ### BAZ");
    /// ```
    #[must_use]
    pub fn replace<'a, F, S: Into<Cow<'a, str>>>(
        &'a self,
        text: &'a str,
        callback: F,
        threshold: f32,
    ) -> String
    where
        F: Fn(&FuzzyMatch<'a>) -> Option<S>,
        S: Into<Cow<'a, str>>,
    {
        self.search_non_overlapping(text, threshold)
            .replace(callback)
    }

    /// Strip any leading fuzzy‐matched prefix from `haystack` using the given
    /// similarity `threshold`, and return the remainder of the string.
    ///
    /// # Behavior
    ///
    /// - All initial [`Segment::Matched`] variants are skipped.
    /// - Any unmatched segments consisting solely of whitespace are also skipped.
    /// - The first non‐whitespace [`Segment::Unmatched`]:
    ///   - Has its leading whitespace trimmed before appending.
    ///   - Disables skipping so that all subsequent segments are included.
    /// - After that point, both `Matched` and `Unmatched` segments are appended
    ///   in full (without further trimming).
    ///
    /// # Parameters
    ///
    /// - `haystack`: The text to strip a fuzzy‐matched prefix from.
    /// - `threshold`: A float from `0.0` to `1.0` indicating the minimum
    ///   similarity score required for a match.
    ///
    /// # Returns
    ///
    /// A `String` containing the remainder of `haystack` after removing the
    /// leading fuzzy‐matched portion and any leading whitespace.
    ///
    /// # Examples
    ///
    /// ```
    /// use fuzzy_aho_corasick::{FuzzyAhoCorasickBuilder, FuzzyLimits};
    /// let f = FuzzyAhoCorasickBuilder::new()
    ///     .fuzzy(FuzzyLimits::new().edits(1))
    ///     .case_insensitive(true)
    ///     .build(["LOREM", "IPSUM"]);
    ///
    /// // "LROEM" fuzzy‐matches "LOREM", "PISUM" matches "IPSUM",
    /// // so both are stripped, and leading space before "ZZZ" is trimmed:
    /// let result = f.strip_prefix("LrEM ISuM Lorm ZZZ", 0.8);
    /// assert_eq!(result, "ZZZ");
    /// ```
    #[must_use]
    pub fn strip_prefix<'a>(&'a self, haystack: &'a str, threshold: f32) -> String {
        self.search_non_overlapping(haystack, threshold)
            .strip_prefix()
    }

    /// Perform a non‐overlapping fuzzy search over `haystack` with the given
    /// similarity `threshold`, then strip any trailing fuzzy‐matched suffix
    /// from the end of the string and return the leading portion.
    ///
    /// # Behavior
    ///
    /// - Conducts a non‐overlapping fuzzy search (via [`search_non_overlapping`]).
    /// - Skips all trailing [`Segment::Matched`] variants.
    /// - Skips any trailing [`Segment::Unmatched`] variants consisting solely of whitespace.
    /// - The last non‐whitespace [`Segment::Unmatched`]:
    ///   - Has its trailing whitespace trimmed before inclusion.
    ///   - Marks the cutoff point—everything after it is dropped.
    ///
    /// # Parameters
    ///
    /// - `haystack`: The text to strip a fuzzy‐matched suffix from.
    /// - `threshold`: A float in `0.0..=1.0` indicating the minimum similarity
    ///   score required for a match to count as part of the suffix.
    ///
    /// # Returns
    ///
    /// A `String` containing the beginning of `haystack` with any trailing
    /// fuzzy‐matched portion (and trailing whitespace) removed.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use fuzzy_aho_corasick::{FuzzyAhoCorasickBuilder, FuzzyLimits};
    ///
    /// let f = FuzzyAhoCorasickBuilder::new()
    ///     .fuzzy(FuzzyLimits::new().edits(1))
    ///     .case_insensitive(true)
    ///     .build(["LOREM", "IPSUM"]);
    ///
    /// // The suffix " LrEM ISuM" fuzzily matches " LOREM IPSUM" at ≥0.8,
    /// // so it's stripped from the end, leaving only "ZZZ".
    /// let result = f.strip_postfix("ZZZ LrEM ISuM", 0.8);
    /// assert_eq!(result, "ZZZ");
    /// ```
    #[must_use]
    pub fn strip_postfix<'a>(&'a self, haystack: &'a str, threshold: f32) -> String {
        self.search_non_overlapping(haystack, threshold)
            .strip_postfix()
    }

    /// Split `haystack` into unmatched substrings by treating each fuzzy match
    /// (above the given `threshold`) as a separator.
    ///
    /// # Behavior
    ///
    /// - Performs a non-overlapping fuzzy search over `haystack` using
    ///   [`search_non_overlapping`].
    /// - Delegates to the `split()` method on the resulting `FuzzyMatches`.
    /// - Produces one `String` per unmatched segment in order, including empty
    ///   segments if matches occur at the very start or end.
    ///
    /// # Parameters
    ///
    /// - `haystack`: The input text to split on fuzzy matches.
    /// - `threshold`: A similarity cutoff (`0.0..=1.0`); only matches with
    ///   a score ≥ `threshold` are treated as separators.
    ///
    /// # Returns
    ///
    /// A `Vec<String>` containing the parts of `haystack` between each fuzzy match.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use fuzzy_aho_corasick::{FuzzyAhoCorasickBuilder, FuzzyLimits};
    ///
    /// let engine = FuzzyAhoCorasickBuilder::new()
    ///     .fuzzy(FuzzyLimits::new().edits(1))
    ///     .case_insensitive(true)
    ///     .build(["FOO", "BAR"]);
    ///
    /// let parts: Vec<&str> = engine.split("xxFo0yyBAARzz", 0.8).collect();
    /// assert_eq!(parts, vec!["xx", "yy", "zz"]);
    /// ```
    #[must_use]
    pub fn split<'a>(
        &'a self,
        haystack: &'a str,
        threshold: f32,
    ) -> impl Iterator<Item = &'a str> + 'a {
        self.search_non_overlapping(haystack, threshold).split()
    }

    /// Returns an **iterator** that yields interleaving [`Segment::Matched`]
    /// [`Segment::Unmatched`] items for the given text.
    pub fn segment_iter<'a>(
        &'a self,
        haystack: &'a str,
        threshold: f32,
    ) -> impl Iterator<Item = Segment<'a>> {
        self.search_non_overlapping(haystack, threshold)
            .segment_iter()
    }
    /// Convenience wrapper around [`segment_iter`](Self::segment_iter).
    #[must_use]
    pub fn segment_text(&self, haystack: &str, threshold: f32) -> String {
        self.search_non_overlapping(haystack, threshold)
            .segment_text()
    }
}
