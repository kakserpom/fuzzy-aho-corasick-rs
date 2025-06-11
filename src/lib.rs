#![warn(clippy::pedantic)]
#![allow(clippy::too_many_lines, clippy::cast_precision_loss)]
mod builder;
mod replacer;
mod segment;
mod structs;
#[cfg(test)]
mod tests;

pub use replacer::FuzzyReplacer;

pub use builder::FuzzyAhoCorasickBuilder;
use std::borrow::Cow;
use std::collections::BTreeMap;
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
    fn within_limits_ins_del_ahead(
        &self,
        limits: Option<&FuzzyLimits>,
        edits: NumEdits,
        insertions: NumEdits,
        deletions: NumEdits,
    ) -> (bool, bool) {
        if let Some(max) = limits.or(self.limits.as_ref()) {
            let edits_ok = max.edits.is_none_or(|max| edits < max);
            (
                edits_ok && max.insertions.is_none_or(|max| insertions < max),
                edits_ok && max.deletions.is_none_or(|max| deletions < max),
            )
        } else {
            (false, false)
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
            max.edits.is_none_or(|max| edits < max) && max.swaps.is_none_or(|max| swaps < max)
        } else {
            false
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
            max.edits.is_none_or(|max| edits <= max)
                && max.insertions.is_none_or(|max| insertions <= max)
                && max.deletions.is_none_or(|max| deletions <= max)
                && max.substitutions.is_none_or(|max| substitutions <= max)
                && max.swaps.is_none_or(|max| swaps <= max)
        } else {
            edits == 0 && insertions == 0 && deletions == 0 && substitutions == 0 && swaps == 0
        }
    }

    #[inline]
    #[allow(clippy::too_many_arguments)]
    fn scalar_output_handling(
        &self,
        output: &[usize],
        penalties: f32,
        edits: usize,
        insertions: usize,
        deletions: usize,
        substitutions: usize,
        swaps: usize,
        matched_start: usize,
        matched_end: usize,
        grapheme_idx: &[(usize, &str)],
        text: &str,
        best: &mut BTreeMap<(usize, usize, usize), FuzzyMatch>,
        similarity_threshold: f32,
        #[cfg(debug_assertions)] notes: &Vec<String>,
    ) {
        for &pat_idx in output {
            if !self.within_limits(
                self.patterns[pat_idx].limits.as_ref(),
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
                .map_or_else(|| text.len(), |&(b, _)| b);
            let key = (start_byte, end_byte, pat_idx);

            let total = self.patterns[pat_idx].grapheme_len as f32;

            let similarity = (total - penalties) / total * self.patterns[pat_idx].weight;

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
                            pattern_index: pat_idx,
                            start: start_byte,
                            end: end_byte,
                            pattern: self.patterns[pat_idx].pattern.clone(),
                            similarity,
                            text: text[start_byte..end_byte].to_string(),
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
                    pattern_index: pat_idx,
                    start: start_byte,
                    end: end_byte,
                    pattern: self.patterns[pat_idx].pattern.clone(),
                    similarity,
                    text: text[start_byte..end_byte].to_string(),
                    #[cfg(debug_assertions)]
                    notes: notes.clone(),
                });
        }
    }

    #[inline]
    #[must_use]
    pub fn search(&self, haystack: &str, similarity_threshold: f32) -> Vec<FuzzyMatch> {
        let grapheme_idx: Vec<(usize, &str)> = haystack.grapheme_indices(true).collect();
        if grapheme_idx.is_empty() {
            return vec![];
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
                let mut notes = queue[q_idx].notes.clone();
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
                    self.scalar_output_handling(
                        output,
                        penalties,
                        edits,
                        insertions,
                        deletions,
                        substitutions,
                        swaps,
                        matched_start,
                        matched_end,
                        &grapheme_idx,
                        haystack,
                        &mut best,
                        similarity_threshold,
                        #[cfg(debug_assertions)]
                        &notes,
                    );
                }

                if j == text_chars.len() {
                    continue;
                }

                let current_grapheme = text_chars[j].as_ref();
                let current_grapheme_first_char = current_grapheme.chars().next().unwrap_or('\0');

                // 1)  Same or similar symbol
                for (edge_g, &next_node) in transitions {
                    #[cfg(debug_assertions)]
                    let mut notes = notes.clone();
                    let g_ch = edge_g.chars().next().unwrap_or('\0');
                    let (next_penalties, next_edits, next_subs) = if edge_g == current_grapheme {
                        trace!(
                            "  match   {:>8} ─{:>3}→ node={}  sim=1.00",
                            edge_g, "ok", next_node
                        );
                        (penalties, edits, substitutions)
                    } else {
                        let sim = *self
                            .similarity
                            .get(&(g_ch, current_grapheme_first_char))
                            .unwrap_or(&0.);
                        let penalty = self.penalties.substitution * (1. - sim);
                        trace!(
                            "  subst {:?} ─{:>3}→ {current_grapheme:?} node={:?}  base_penalty={:.2} sim={:.2} penalty={:.2}",
                            edge_g, "sub", next_node, self.penalties.substitution, sim, penalty
                        );
                        #[cfg(debug_assertions)]
                        notes.push(format!(
                            "sub {edge_g:?} -> {current_grapheme:?} (sim={sim:?}, penalty={penalty:?}) (sub+1={:?}, edits+1={:?})",
                            substitutions + 1,
                            edits + 1,
                        ));
                        (penalties + penalty, edits + 1, substitutions + 1)
                    };

                    queue.push(State {
                        node: next_node,
                        j: j + 1,
                        matched_start: if matched_end == matched_start {
                            j
                        } else {
                            matched_start
                        },
                        matched_end: j + 1,
                        penalties: next_penalties,
                        edits: next_edits,
                        insertions,
                        deletions,
                        substitutions: next_subs,
                        swaps,
                        #[cfg(debug_assertions)]
                        notes,
                    });
                }

                // Swap (transposition of two neighboring graphemes)
                if j + 1 < text_chars.len() {
                    let a = &text_chars[j];
                    let b = &text_chars[j + 1];
                    // check if the node has B-transition and then A-transition
                    if let Some(&node) = transitions
                        .get(b.as_ref())
                        .and_then(|&x| self.nodes[x].transitions.get(a.as_ref()))
                    {
                        // Checking swap
                        // Correct option
                        if self.within_limits_swap_ahead(self.get_node_limits(node), edits, swaps) {
                            #[cfg(debug_assertions)]
                            notes.push(format!(
                                "swap a:{a:?} b:{b:?} (swaps+1={:?}, edits+1={:?})",
                                substitutions + 1,
                                edits + 1,
                            ));
                            queue.push(State {
                                node,
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
                                notes: notes.clone(),
                            });
                        }
                    }
                }

                // 3)  Insertion / Deletion
                let (ins_ex, del_ex) = self.within_limits_ins_del_ahead(
                    self.get_node_limits(node),
                    edits,
                    insertions,
                    deletions,
                );
                if ins_ex || del_ex {
                    trace!(
                        "  insert  (skip {:?})  penalty={:.2}",
                        text_chars[j], self.penalties.insertion
                    );
                    if ins_ex && matched_start != matched_end || matched_start != j {
                        #[cfg(debug_assertions)]
                        notes.push(format!(
                            "ins {:?} (sub+1={:?}, edits+1={:?})",
                            text_chars[j],
                            substitutions + 1,
                            edits + 1,
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
                            notes: notes.clone(),
                        });
                    }
                    if del_ex {
                        for (edge_g, &next_node) in transitions {
                            trace!(
                                "  delete to node={next_node} penalty={:.2}",
                                self.penalties.deletion
                            );
                            #[cfg(debug_assertions)]
                            notes.push(format!("del {:?} (del+1={:?})", edge_g, deletions + 1,));
                            queue.push(State {
                                node: next_node,
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
                                notes: notes.clone(),
                            });
                        }
                    }
                }
            }
        }

        best.into_values().collect()
    }

    /// Search without overlapping matches (the engine will greedily choose the
    /// longest non‑overlapping matches from left to right).
    #[must_use]
    pub fn search_non_overlapping(
        &self,
        haystack: &str,
        similarity_threshold: f32,
    ) -> Vec<FuzzyMatch> {
        let mut matches = self.search(haystack, similarity_threshold);
        #[cfg(test)]
        trace!("raw matches: {:?}", matches);
        matches.sort_by(|left, right| {
            right
                .similarity
                .total_cmp(&left.similarity)
                .then_with(|| (right.end - right.start).cmp(&(left.end - left.start)))
                .then_with(|| left.start.cmp(&right.start))
        });
        let mut chosen = Vec::new();
        let mut occupied_intervals: BTreeMap<usize, usize> = BTreeMap::new();
        for matched in matches {
            if occupied_intervals
                .range(..=matched.start)
                .next_back()
                .is_none_or(|(_, &end)| end <= matched.start)
                && occupied_intervals
                    .range(matched.start..)
                    .next()
                    .is_none_or(|(&start, _)| start >= matched.end)
            {
                occupied_intervals.insert(matched.start, matched.end);
                chosen.push(matched);
            }
        }

        chosen.sort_by_key(|m| m.start);
        chosen
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
        for matched in self.search_non_overlapping(text, threshold) {
            if matched.start >= last {
                result.push_str(&text[last..matched.start]);
                last = matched.end;
                result.push_str(callback(&matched).unwrap_or(&matched.text));
            }
        }
        result.push_str(&text[last..]);
        result
    }
}
