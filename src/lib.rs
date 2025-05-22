mod builder;
mod segment;
mod structs;
#[cfg(test)]
mod tests;

pub use segment::FuzzyReplacer;

pub use builder::FuzzyAhoCorasickBuilder;
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
    fn within_limits_ins_del_ahead(
        &self,
        limits: Option<&FuzzyLimits>,
        edits: NumEdits,
        insertions: NumEdits,
        deletions: NumEdits,
    ) -> (bool, bool) {
        if let Some(m) = limits.or(self.limits.as_ref()) {
            let edits_ok = m.edits.is_none_or(|max| edits < max);
            (
                edits_ok && insertions < m.insertions,
                edits_ok && deletions < m.deletions,
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
        if let Some(m) = limits.or(self.limits.as_ref()) {
            m.edits.is_none_or(|max| edits < max) && swaps < m.swaps
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
        if let Some(m) = limits.or(self.limits.as_ref()) {
            m.edits.is_none_or(|max| edits <= max)
                && insertions <= m.insertions
                && deletions <= m.deletions
                && substitutions <= m.substitutions
                && swaps <= m.swaps
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
        #[cfg(debug_assertions)] notes: Vec<String>,
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
            let start_byte = grapheme_idx
                .get(matched_start)
                .map(|&(b, _)| b)
                .unwrap_or(0);
            let end_byte = grapheme_idx
                .get(matched_end)
                .map(|&(b, _)| b)
                .unwrap_or(text.len());
            let key = (start_byte, end_byte, pat_idx);

            let total = self.patterns[pat_idx].grapheme_len as f32;
            let similarity = (total - penalties) / total * self.patterns[pat_idx].weight;

            if similarity < similarity_threshold {
                continue;
            }

            best.entry(key)
                .and_modify(|entry| {
                    if similarity > entry.similarity {
                        entry.similarity = similarity;
                    }
                })
                .or_insert_with(|| FuzzyMatch {
                    insertions,
                    deletions,
                    substitutions,
                    edits,
                    swaps: 0,
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
    pub fn search(&self, text: &str, similarity_threshold: f32) -> Vec<FuzzyMatch> {
        if text.is_empty() {
            return Vec::new();
        }

        let grapheme_idx: Vec<(usize, &str)> = text.grapheme_indices(true).collect();
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

        trace!("=== fuzzy_search on {text:?} (similarity_threshold {similarity_threshold:.2}) ===",);
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
                        text,
                        &mut best,
                        similarity_threshold,
                        #[cfg(debug_assertions)]
                        notes.clone(),
                    );
                }

                if j == text_chars.len() {
                    continue;
                }

                let txt = text_chars[j].as_ref();
                let txt_ch = txt.chars().next().unwrap_or('\0');

                // 1)  Same or similar symbol
                for (edge_g, &next_node) in transitions {
                    #[cfg(debug_assertions)]
                    let mut notes = notes.clone();
                    let g_ch = edge_g.chars().next().unwrap_or('\0');
                    let (next_penalties, next_edits, next_subs) = if edge_g == txt {
                        trace!(
                            "  match   {:>8} ─{:>3}→ node={}  sim=1.00",
                            edge_g, "ok", next_node
                        );
                        (penalties, edits, substitutions)
                    } else {
                        let sim = *self.similarity.get(&(g_ch, txt_ch)).unwrap_or(&0.);
                        let penalty = 1. - self.penalties.substitution * (1. - sim);
                        trace!(
                            "  subst   {:?} ─{:>3}→ {txt:?} node={:?}  base_penalty={:.2} sim={:.2} penalty={:.2}",
                            edge_g, "sub", next_node, self.penalties.substitution, sim, penalty
                        );
                        #[cfg(debug_assertions)]
                        notes.push(format!(
                            "subst {edge_g:?} -> {txt:?} (substitutions + 1) = {:?}",
                            substitutions + 1
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
                    if let Some(&n1) = transitions.get(b.as_ref()) {
                        if let Some(&n2) = self.nodes[n1].transitions.get(a.as_ref()) {
                            // Checking swap
                            // Correct option
                            if self.within_limits_swap_ahead(self.get_node_limits(n2), edits, swaps)
                            {
                                queue.push(State {
                                    node: n2,
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
                }

                // 3)  Insertion
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
                        queue.push(State {
                            node,
                            j: j + 1,
                            matched_start,
                            matched_end,
                            penalties: penalties * self.penalties.insertion,
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
                        for &next_node in transitions.values() {
                            trace!(
                                "  delete  to node={}  penalty={:.2}",
                                next_node, self.penalties.deletion
                            );
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

        let mut matches: Vec<FuzzyMatch> = best.into_values().collect();
        if self.non_overlapping {
            matches.sort_by(|a, b| {
                b.similarity
                    .partial_cmp(&a.similarity)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| (b.end - b.start).cmp(&(a.end - a.start)))
                    .then_with(|| a.start.cmp(&b.start))
            });

            let mut chosen = Vec::new();
            let mut occupied = BTreeSet::new();
            for m in matches {
                if (m.start..m.end).any(|pos| occupied.contains(&pos)) {
                    continue;
                }
                occupied.extend(m.start..m.end);
                chosen.push(m);
            }

            chosen.sort_by_key(|m| m.start);
            chosen
        } else {
            #[cfg(test)]
            {
                trace!("*** raw matches ***");
                for m in &matches {
                    trace!("{:?}", m);
                }
            }
            matches
        }
    }
}
