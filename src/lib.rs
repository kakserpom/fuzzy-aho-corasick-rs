#![warn(clippy::pedantic)]
#![allow(clippy::too_many_lines, clippy::cast_precision_loss)]
mod builder;
mod replacer;
mod segment;
pub mod structs;
#[cfg(test)]
mod tests;

pub use builder::FuzzyAhoCorasickBuilder;
pub use replacer::FuzzyReplacer;
use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};
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
    #[inline]
    fn get_node_limits(&self, node: usize) -> Option<&FuzzyLimits> {
        self.nodes[node]
            .pattern_index
            .and_then(|i| self.patterns.get(i).and_then(|p| p.limits.as_ref()))
    }
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

    #[must_use]
    pub fn patterns(&self) -> &Vec<Pattern> {
        &self.patterns
    }

    #[inline]
    #[must_use]
    pub fn search<'a>(&'a self, haystack: &'a str, similarity_threshold: f32) -> FuzzyMatches<'a> {
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

        let mut best: BTreeMap<(usize, usize, usize), FuzzyMatch> = BTreeMap::new();

        let mut queue: Vec<State> = Vec::with_capacity(64);

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

                /*trace!(
                    "visit: node={} j={} span=({}->{}) score={:.3} edits={}",
                    node, j, matched_start, matched_end, score, edits
                );*/

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
                                        notes: notes.to_owned(),
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
                                notes: notes.to_owned(),
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
                            let sim = *self.similarity.get(&(g_ch, current_ch)).unwrap_or(&0.);
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
        let mut inner: Vec<_> = best
            .into_values()
            .map(|mut m| {
                m.text = &haystack[m.start..m.end];
                m
            })
            .collect();

        inner.sort_by(|left, right| {
            right
                .similarity
                .total_cmp(&left.similarity)
                .then_with(|| right.pattern.len().cmp(&left.pattern.len()))
                .then_with(|| right.text.len().cmp(&left.text.len()))
                .then_with(|| left.start.cmp(&right.start))
        });

        FuzzyMatches { haystack, inner }
    }

    /// Search without overlapping matches (the engine will greedily choose the
    /// longest non‑overlapping matches from left to right).
    #[must_use]
    pub fn search_non_overlapping<'a>(
        &'a self,
        haystack: &'a str,
        similarity_threshold: f32,
    ) -> FuzzyMatches<'a> {
        let mut matches = self.search(haystack, similarity_threshold);
        let mut occupied_intervals: BTreeMap<usize, usize> = BTreeMap::new();
        matches.inner.retain(|m| {
            if occupied_intervals
                .range(..=m.start)
                .next_back()
                .is_none_or(|(_, &end)| end <= m.start)
                && occupied_intervals
                    .range(m.start..)
                    .next()
                    .is_none_or(|(&start, _)| start >= m.end)
            {
                occupied_intervals.insert(m.start, m.end);
                #[cfg(test)]
                trace!("ACCEPTING: \t{:?}", m);
                true
            } else {
                #[cfg(test)]
                trace!("DISCARDING OVERLAPPING: {m:?}");
                false
            }
        });
        matches.inner.sort_by_key(|m| m.start);
        matches
    }

    #[must_use]
    pub fn search_non_overlapping_unique<'a>(
        &'a self,
        haystack: &'a str,
        similarity_threshold: f32,
    ) -> FuzzyMatches<'a> {
        let mut matches = self.search(haystack, similarity_threshold);
        let mut used_patterns = BTreeSet::new();
        let mut occupied_intervals: BTreeMap<usize, usize> = BTreeMap::new();
        matches.inner.retain(|m| {
            let unique_id =
                if let Some(custom_unique_id) = self.patterns[m.pattern_index].custom_unique_id {
                    UniqueId::Custom(custom_unique_id)
                } else {
                    UniqueId::Automatic(m.pattern_index)
                };
            if !used_patterns.contains(&unique_id)
                && occupied_intervals
                    .range(..=m.start)
                    .next_back()
                    .is_none_or(|(_, &end)| end <= m.start)
                && occupied_intervals
                    .range(m.start..)
                    .next()
                    .is_none_or(|(&start, _)| start >= m.end)
            {
                used_patterns.insert(unique_id);
                occupied_intervals.insert(m.start, m.end);
                #[cfg(test)]
                trace!("ACCEPTING: \t{:?}", m);
                true
            } else {
                #[cfg(test)]
                trace!("DISCARDING OVERLAPPING: {m:?}");
                false
            }
        });
        matches.inner.sort_by_key(|m| m.start);
        matches
    }

    /// Performs a **fuzzy** find‑and‑replace using a list of `(pattern →
    /// replacement)` pairs.  Replacements are applied left‑to‑right, the longest
    /// non‑overlapping match wins.
    #[must_use]
    pub fn replace<'a, F>(&self, text: &str, callback: F, threshold: f32) -> String
    where
        F: Fn(&FuzzyMatch) -> Option<&'a str>,
    {
        let mut result = String::new();
        let mut last = 0;
        for matched in &self.search_non_overlapping(text, threshold) {
            if matched.start >= last {
                result.push_str(&text[last..matched.start]);
                last = matched.end;
                result.push_str(callback(matched).unwrap_or(matched.text));
            }
        }
        result.push_str(&text[last..]);
        result
    }
}
