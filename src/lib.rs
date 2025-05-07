#![feature(portable_simd)]

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

#[cfg(all(target_arch = "aarch64", feature = "simd"))]
const LANES: usize = 4;

#[cfg(all(target_arch = "x86_64", target_feature = "avx512f", feature = "simd"))]
const LANES: usize = 16;

#[cfg(all(target_arch = "x86_64", target_feature = "avx2", feature = "simd"))]
const LANES: usize = 8;

// Fallback
#[cfg(all(
    feature = "simd",
    not(any(
        all(target_arch = "aarch64"),
        all(target_arch = "x86_64", target_feature = "avx512f"),
        all(target_arch = "x86_64", target_feature = "avx2"),
    ))
))]
const LANES: usize = 1;

/// SIMD batch threshold
#[cfg(feature = "simd")]
const LANE_THRESHOLD: usize = LANES * 4;
/// If true, then SIMD is enabled.
#[cfg(feature = "simd")]
const SIMD_ENABLED: bool = LANES > 1;

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
            println!("!!m = {m:?}");
            let edits_ok = m.edits.is_none_or(|max| edits + 1 <= max);
            (
                edits_ok && insertions + 1 <= m.insertions,
                edits_ok && deletions + 1 <= m.deletions,
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
            println!("!!m = {m:?}");
            m.edits.is_none_or(|max| edits + 1 <= max) && swaps + 1 <= m.swaps
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
            println!("m = {m:?}");
            m.edits.is_none_or(|max| edits <= max)
                && insertions <= m.insertions
                && deletions <= m.deletions
                && substitutions <= m.substitutions
                && swaps <= m.swaps
        } else {
            println!("limits none");
            edits == 0 && insertions == 0 && deletions == 0 && substitutions == 0 && swaps == 0
        }
    }

    #[inline]
    #[allow(clippy::too_many_arguments)]
    fn scalar_output_handling(
        &self,
        output: &[usize],
        score: f32,
        edits: usize,
        insertions: usize,
        deletions: usize,
        substitutions: usize,
        matched_start: usize,
        matched_end: usize,
        grapheme_idx: &[(usize, &str)],
        text: &str,
        best: &mut BTreeMap<(usize, usize, usize), FuzzyMatch>,
    ) {
        for &pat_idx in output {
            if !self.within_limits(
                self.patterns[pat_idx].limits.as_ref(),
                edits,
                insertions,
                deletions,
                substitutions,
                0,
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

            let base = score * self.patterns[pat_idx].weight;
            let matched = (matched_end - matched_start) as f32;
            let total = self.patterns[pat_idx].grapheme_len as f32;
            let coverage = (matched / total).clamp(0.0, 1.0);
            let cand_score = base * coverage;

            best.entry(key)
                .and_modify(|entry| {
                    if cand_score > entry.similarity {
                        entry.similarity = cand_score;
                    }
                })
                .or_insert_with(|| FuzzyMatch {
                    insertions,
                    deletions,
                    substitutions,
                    swaps: 0,
                    pattern_index: pat_idx,
                    start: start_byte,
                    end: end_byte,
                    pattern: self.patterns[pat_idx].pattern.clone(),
                    similarity: cand_score,
                    text: text[start_byte..end_byte].to_string(),
                });
        }
    }

    #[cfg(feature = "simd")]
    #[inline]
    fn simd_output_handling(
        &self,
        output: &[usize],
        score: f32,
        edits: usize,
        insertions: usize,
        deletions: usize,
        substitutions: usize,
        matched_start: usize,
        matched_end: usize,
        grapheme_idx: &[(usize, &str)],
        text: &str,
        pattern_weights: &[f32],
        best: &mut BTreeMap<(usize, usize, usize), FuzzyMatch>,
    ) {
        use std::simd::Simd;

        // Chunking patterns' indexes
        let mut buf = [0usize; LANES];
        let mut lane = 0;
        for &pat_idx in output {
            buf[lane] = pat_idx;
            lane += 1;
            if lane == LANES {
                let idxs = Simd::<usize, LANES>::from_array(buf);
                let wv = Simd::gather_or_default(pattern_weights, idxs);
                let sv = Simd::splat(score) * wv;

                for i in 0..LANES {
                    let p = buf[i];
                    if !self.within_limits(
                        self.patterns[p].limits.as_ref(),
                        edits,
                        insertions,
                        deletions,
                        substitutions,
                        0,
                    ) {
                        continue;
                    }
                    let sb = grapheme_idx[matched_start].0;
                    let eb = grapheme_idx[matched_end].0;
                    let key = (sb, eb, p);
                    let base = score * sv[i];
                    let matched = (matched_end - matched_start) as f32;
                    let total = self.patterns[pat_idx].grapheme_len as f32;
                    let coverage = (matched / total).clamp(0.0, 1.0);
                    let cand_score = base * coverage;
                    best.entry(key)
                        .and_modify(|m| {
                            if cand_score > m.similarity {
                                m.similarity = cand_score;
                            }
                        })
                        .or_insert_with(|| FuzzyMatch {
                            insertions,
                            deletions,
                            substitutions,
                            swaps: 0,
                            pattern_index: p,
                            start: sb,
                            end: eb,
                            pattern: self.patterns[p].pattern.clone(),
                            similarity: cand_score,
                            text: text[sb..eb].to_string(),
                        });
                }

                lane = 0;
            }
        }

        // tail
        if lane > 0 {
            let mut arr = [0f32; LANES];
            for i in 0..lane {
                arr[i] = pattern_weights[buf[i]];
            }
            let wv = Simd::from_array(arr);
            let sv = Simd::splat(score) * wv;

            for i in 0..lane {
                let p = buf[i];
                if !self.within_limits(
                    self.patterns[p].limits.as_ref(),
                    edits,
                    insertions,
                    deletions,
                    substitutions,
                    0,
                ) {
                    continue;
                }
                let sb = grapheme_idx
                    .get(matched_start)
                    .map(|&(b, _)| b)
                    .unwrap_or(0);
                let eb = grapheme_idx
                    .get(matched_end)
                    .map(|&(b, _)| b)
                    .unwrap_or(text.len());

                let key = (sb, eb, p);
                let cand = sv[i];
                best.entry(key)
                    .and_modify(|m| {
                        if cand > m.similarity {
                            m.similarity = cand;
                        }
                    })
                    .or_insert_with(|| FuzzyMatch {
                        insertions,
                        deletions,
                        substitutions,
                        swaps: 0,
                        pattern_index: p,
                        start: sb,
                        end: eb,
                        pattern: self.patterns[p].pattern.clone(),
                        similarity: cand,
                        text: text[sb..eb].to_string(),
                    });
            }
        }
    }

    #[cfg(not(feature = "simd"))]
    #[inline]
    pub fn search(&self, text: &str, threshold: f32) -> Vec<FuzzyMatch> {
        self.scalar_search(text, threshold)
    }

    #[cfg(feature = "simd")]
    pub fn search(&self, text: &str, threshold: f32) -> Vec<FuzzyMatch> {
        use std::simd::Simd;
        use std::simd::cmp::SimdPartialEq;

        if !SIMD_ENABLED {
            return self.scalar_search(text, threshold);
        }
        // empty text shortcut
        if text.is_empty() {
            return Vec::new();
        }
        // prepare grapheme indices and possibly lowercase
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
        // best match map
        let mut best: BTreeMap<(usize, usize, usize), FuzzyMatch> = BTreeMap::new();
        // BFS state
        #[derive(Clone)]
        struct State {
            node: usize,
            j: usize,
            matched_start: usize,
            matched_end: usize,
            score: f32,
            edits: usize,
            insertions: usize,
            deletions: usize,
            substitutions: usize,
            swaps: usize,
        }
        let mut queue: Vec<State> = Vec::with_capacity(64);
        // main loop over possible start positions
        for start in 0..text_chars.len() {
            queue.clear();
            queue.push(State {
                node: 0,
                j: start,
                matched_start: start,
                matched_end: start,
                score: 1.0,
                edits: 0,
                insertions: 0,
                deletions: 0,
                substitutions: 0,
                swaps: 0,
            });
            let mut q_idx = 0;
            while q_idx < queue.len() {
                let st = queue[q_idx].clone();
                q_idx += 1;
                let State {
                    node,
                    j,
                    matched_start,
                    matched_end,
                    score,
                    edits,
                    insertions,
                    deletions,
                    substitutions,
                    swaps,
                } = st;
                let Node {
                    output,
                    transitions,
                    ..
                } = &self.nodes[node];
                // output handling (SIMD vs scalar)
                if !output.is_empty() && score >= threshold {
                    if output.len() >= LANE_THRESHOLD {
                        let weights: Vec<f32> = self.patterns.iter().map(|p| p.weight).collect();
                        self.simd_output_handling(
                            output,
                            score,
                            edits,
                            insertions,
                            deletions,
                            substitutions,
                            matched_start,
                            matched_end,
                            &grapheme_idx,
                            text,
                            &weights,
                            &mut best,
                        );
                    } else {
                        self.scalar_output_handling(
                            output,
                            score,
                            edits,
                            insertions,
                            deletions,
                            substitutions,
                            matched_start,
                            matched_end,
                            &grapheme_idx,
                            text,
                            &mut best,
                        );
                    }
                }
                // at end of text
                if j == text_chars.len() {
                    continue;
                }
                // prepare next character
                let txt = text_chars[j].as_ref();
                let txt_ch = txt.chars().next().unwrap_or('\0');
                // SIMD transitions for matches/substitutions
                let mut glyphs = Vec::new();
                let mut nodes = Vec::new();
                let mut sims = Vec::new();
                for (edge_g, &next) in transitions {
                    let ch = edge_g.chars().next().unwrap_or('\0');
                    glyphs.push(ch as u32);
                    nodes.push(next);
                    sims.push(if edge_g == txt {
                        1.0
                    } else {
                        *self.similarity.get(&(ch, txt_ch)).unwrap_or(&0.0)
                    });
                }
                // process in lanes
                for i in (0..glyphs.len()).step_by(LANES) {
                    let len = LANES.min(glyphs.len() - i);
                    // build chunk
                    let mut chunk = [0u32; LANES];
                    chunk[..len].copy_from_slice(&glyphs[i..i + len]);
                    let gv = Simd::from_array(chunk);
                    let tv = Simd::splat(txt_ch as u32);
                    let mask = gv.simd_eq(tv).to_bitmask();
                    for lane in 0..len {
                        let next = nodes[i + lane];
                        let new_start = if matched_end == matched_start {
                            j
                        } else {
                            matched_start
                        };
                        let sim = if (mask & (1 << lane)) != 0 {
                            sims[i + lane]
                        } else {
                            self.penalties.substitution
                        };
                        let (ed, subs) = if (mask & (1 << lane)) != 0 {
                            (edits, substitutions)
                        } else {
                            (edits + 1, substitutions + 1)
                        };
                        queue.push(State {
                            node: next,
                            j: j + 1,
                            matched_start: new_start,
                            matched_end: j + 1,
                            score: score * sim,
                            edits: ed,
                            insertions,
                            deletions,
                            substitutions: subs,
                            swaps,
                        });
                    }
                }
                // insertion/deletion
                let (ins_ex, del_ex) = self.within_limits_ins_del_ahead(
                    self.get_node_limits(node),
                    edits,
                    insertions,
                    deletions,
                );
                if ins_ex {
                    queue.push(State {
                        node,
                        j: j + 1,
                        matched_start,
                        matched_end,
                        score: score * self.penalties.insertion,
                        edits: edits + 1,
                        insertions: insertions + 1,
                        deletions,
                        substitutions,
                        swaps,
                    });
                }
                if del_ex {
                    for &next in transitions.values() {
                        queue.push(State {
                            node: next,
                            j,
                            matched_start,
                            matched_end,
                            score: score * self.penalties.deletion,
                            edits: edits + 1,
                            insertions,
                            deletions: deletions + 1,
                            substitutions,
                            swaps,
                        });
                    }
                }
                // swap (transposition)
                if j + 1 < text_chars.len() {
                    let a = &text_chars[j];
                    let b = &text_chars[j + 1];
                    if let Some(&n1) = transitions.get(b.as_ref()) {
                        if let Some(&n2) = self.nodes[n1].transitions.get(a.as_ref()) {
                            if self.within_limits_swap_ahead(self.get_node_limits(n2), edits, swaps)
                            {
                                queue.push(State {
                                    node: n2,
                                    j: j + 2,
                                    matched_start,
                                    matched_end: j + 2,
                                    score: score * self.penalties.swap,
                                    edits: edits + 1,
                                    insertions,
                                    deletions,
                                    substitutions,
                                    swaps: swaps + 1,
                                });
                            }
                        }
                    }
                }
            }
        }
        // finalize same as scalar
        let mut matches: Vec<FuzzyMatch> = best.into_values().collect();
        if self.non_overlapping {
            matches.sort_by(|a, b| {
                a.start
                    .cmp(&b.start)
                    .then_with(|| b.similarity.partial_cmp(&a.similarity).unwrap())
                    .then_with(|| (b.end - b.start).cmp(&(a.end - a.start)))
            });
            let mut chosen = Vec::new();
            let mut occ = BTreeSet::new();
            for m in matches {
                if (m.start..m.end).any(|p| occ.contains(&p)) {
                    continue;
                }
                occ.extend(m.start..m.end);
                chosen.push(m);
            }
            chosen
        } else {
            matches
        }
    }

    pub fn scalar_search(&self, text: &str, threshold: f32) -> Vec<FuzzyMatch> {
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

        trace!(
            "=== fuzzy_search on {:?} (threshold {:.2}) ===",
            text, threshold
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
                score: 1.0,
                edits: 0,
                insertions: 0,
                deletions: 0,
                substitutions: 0,
                swaps: 0,
            });

            let mut q_idx = 0;
            while q_idx < queue.len() {
                let State {
                    node,
                    j,
                    matched_start,
                    matched_end,
                    score,
                    edits,
                    insertions,
                    deletions,
                    substitutions,
                    swaps,
                } = queue[q_idx];
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

                if !output.is_empty() && score >= threshold {
                    self.scalar_output_handling(
                        output,
                        score,
                        edits,
                        insertions,
                        deletions,
                        substitutions,
                        matched_start,
                        matched_end,
                        &grapheme_idx,
                        text,
                        &mut best,
                    );
                }

                if j == text_chars.len() {
                    continue;
                }

                let txt = text_chars[j].as_ref();
                let txt_ch = txt.chars().next().unwrap_or('\0');

                // 1)  Same or similar symbol
                for (edge_g, &next_node) in transitions {
                    let sim = if edge_g == txt {
                        1.0
                    } else {
                        let g_ch = edge_g.chars().next().unwrap_or('\0');
                        if g_ch == txt_ch {
                            1.0
                        } else {
                            *self.similarity.get(&(g_ch, txt_ch)).unwrap_or(&0.0)
                        }
                    };

                    if sim > 0.0 {
                        trace!(
                            "  match   {:>8} ─{:>3}→ node={}  sim={:.2}",
                            edge_g, "ok", next_node, sim
                        );

                        let new_start = if matched_end == matched_start {
                            j
                        } else {
                            matched_start
                        };

                        queue.push(State {
                            node: next_node,
                            j: j + 1,
                            matched_start: new_start,
                            matched_end: j + 1,
                            score: score * sim,
                            edits,
                            insertions,
                            substitutions,
                            deletions,
                            swaps,
                        });
                    } else {
                        // substitution allowed by Max

                        trace!(
                            "  subst   {:>8} ─{:>3}→ node={}  penalty={:.2}",
                            edge_g, "sub", next_node, self.penalties.substitution
                        );
                        queue.push(State {
                            node: next_node,
                            j: j + 1,
                            matched_start,
                            matched_end: j + 1,
                            score: score * self.penalties.substitution,
                            edits: edits + 1,
                            insertions,
                            deletions,
                            substitutions: substitutions + 1,
                            swaps,
                        });
                    }
                }

                // Swap (transposition of two neighboring graphemes)
                if j + 1 < text_chars.len() {
                    let a = &text_chars[j];
                    let b = &text_chars[j + 1];
                    // check if the node has B-transition and then A-transition
                    if let Some(&n1) = transitions.get(b.as_ref()) {
                        if let Some(&n2) = self.nodes[n1].transitions.get(a.as_ref()) {
                            // Checking swap swap
                            // Correct option
                            if self.within_limits_swap_ahead(self.get_node_limits(n2), edits, swaps)
                            {
                                queue.push(State {
                                    node: n2,
                                    j: j + 2,
                                    matched_start,
                                    matched_end: j + 2,
                                    score: score * self.penalties.swap,
                                    edits: edits + 1,
                                    insertions,
                                    deletions,
                                    substitutions,
                                    swaps: swaps + 1,
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
                            score: score * self.penalties.insertion,
                            edits: edits + 1,
                            insertions: insertions + 1,
                            deletions,
                            substitutions,
                            swaps,
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
                                score: score * self.penalties.deletion,
                                edits: edits + 1,
                                insertions,
                                deletions: deletions + 1,
                                substitutions,
                                swaps,
                            });
                        }
                    }
                }
            }
        }

        let mut matches: Vec<FuzzyMatch> = best.into_values().collect();
        if self.non_overlapping {
            matches.sort_by(|a, b| {
                a.start
                    .cmp(&b.start)
                    .then_with(|| {
                        b.similarity
                            .partial_cmp(&a.similarity)
                            .unwrap_or(std::cmp::Ordering::Equal)
                    })
                    .then_with(|| (b.end - b.start).cmp(&(a.end - a.start)))
            });

            let mut chosen = Vec::<FuzzyMatch>::new();
            let mut occupied: BTreeSet<usize> = BTreeSet::new();
            for m in matches {
                if (m.start..m.end).any(|p| occupied.contains(&p)) {
                    continue;
                }
                occupied.extend(m.start..m.end);
                chosen.push(m);
            }
            #[cfg(test)]
            {
                trace!("*** raw matches ***");
                for m in &chosen {
                    trace!("{:?}", m);
                }
            }
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
