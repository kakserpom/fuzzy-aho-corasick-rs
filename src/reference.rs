//! A deliberately naive reference implementation of fuzzy search, for tests only.
//!
//! The search in `search.rs` is a BFS with a per-window state-dedup table and a family of pruning
//! rules (per-node penalty ceilings, push-time global ceilings, dead-end filters). Each of those is
//! a *proof obligation* — if a bound is unsound, real matches silently disappear — and a test suite
//! only exercises the cases its authors thought of.
//!
//! This module instead enumerates every alignment directly, with **no pruning at all**: for each
//! start position and pattern, a small dynamic program over `(pattern grapheme, text offset,
//! edits used)` that keeps the cheapest way to reach each cell. It is quadratic in the pattern
//! length, uses none of the engine's shortcuts, and is written to be obviously right rather than
//! fast. If the two agree over many random configurations, the pruning is sound; if they disagree,
//! the engine has a bug that hand-written examples would not have caught.
//!
//! Not compiled outside `cfg(test)`.

use std::collections::HashMap;

/// A pattern to search for, in the engine's terms.
#[derive(Clone, Copy, Debug)]
pub(crate) struct RefPattern<'a> {
    pub(crate) text: &'a str,
    pub(crate) weight: f32,
}

/// Similarity table, mirroring [`crate::Similarity`] for the `char` pairs we use.
pub(crate) type RefSim<'a> = &'a HashMap<(char, char), f32>;

/// Edit costs, mirroring [`crate::FuzzyPenalties`].
#[derive(Clone, Copy, Debug)]
pub(crate) struct RefPenalties {
    pub(crate) substitution: f32,
    pub(crate) insertion: f32,
    pub(crate) deletion: f32,
    pub(crate) swap: f32,
}

/// One reported match: `(start, end, pattern_index, similarity_bits)`.
///
/// Only the span and the score are compared. The engine also reports per-type edit counts, and
/// those depend on which of several equally-cheap alignments its BFS happens to reach first, so
/// they are not part of the contract under test.
pub(crate) type RefMatch = (usize, usize, usize, u32);

/// The similarity the engine uses for a substituted pair.
fn sim(table: RefSim<'_>, a: char, b: char) -> f32 {
    if a == b {
        1.0
    } else {
        table.get(&(a, b)).copied().unwrap_or(0.0)
    }
}

/// Every match the engine should report, as a sorted, canonical list.
///
/// `max_edits` is the *total* edit budget. Per-type caps, multi-character mappings, the
/// symbol-similarity floor and beam pruning are out of scope: this pins the core edit model, which
/// is where the pruning lives.
/// Knobs the engine exposes that the reference has to mirror.
#[derive(Clone, Copy, Debug)]
pub(crate) struct RefConfig {
    pub(crate) pen: RefPenalties,
    pub(crate) min_symbol_similarity: f32,
    pub(crate) max_edits: u8,
    pub(crate) threshold: f32,
    pub(crate) case_insensitive: bool,
}

pub(crate) fn reference_search(
    haystack: &str,
    patterns: &[RefPattern<'_>],
    table: RefSim<'_>,
    cfg: RefConfig,
) -> Vec<RefMatch> {
    let RefConfig {
        pen,
        min_symbol_similarity,
        max_edits,
        threshold,
        case_insensitive,
    } = cfg;
    // Fold up front, which is what a case-insensitive engine does to both its patterns and its
    // text. `to_lowercase` on a single-byte grapheme is the ASCII fold the engine's ASCII path uses.
    let fold = |s: &str| -> String {
        if case_insensitive {
            s.to_lowercase()
        } else {
            s.to_string()
        }
    };
    let text: Vec<char> = fold(haystack).chars().collect();
    let folded: Vec<Vec<char>> = patterns
        .iter()
        .map(|p| fold(p.text).chars().collect())
        .collect();

    let mut found: Vec<RefMatch> = Vec::new();
    // Start positions are `0..len`, not `0..=len`: the engine's window loop excludes the end
    // offset, so an all-deletion zero-length match sitting exactly at the end of the haystack
    // (`[len, len)`) is never reported. Everything else about the end offset is reachable — the
    // last real span still ends there — so this only drops that one degenerate case.
    for start in 0..text.len() {
        for (pi, pat) in folded.iter().enumerate() {
            for (end, penalty) in align_all(
                &text,
                start,
                pat,
                table,
                pen,
                min_symbol_similarity,
                max_edits,
            ) {
                let len = pat.len() as f32;
                let similarity = (len - penalty) / len * patterns[pi].weight;
                if similarity < threshold {
                    continue;
                }
                found.push((start, end, pi, similarity.to_bits()));
            }
        }
    }
    // Collapse to the same shape the engine's `best` map does: one entry per (start, end, pattern),
    // keeping the highest score. `align_all` can reach one span by several routes.
    let mut best: HashMap<(usize, usize, usize), f32> = HashMap::new();
    for (start, end, pi, bits) in found {
        let similarity = f32::from_bits(bits);
        best.entry((start, end, pi))
            .and_modify(|cur| {
                if similarity > *cur {
                    *cur = similarity;
                }
            })
            .or_insert(similarity);
    }
    let mut out: Vec<RefMatch> = best
        .into_iter()
        .map(|((start, end, pi), similarity)| (start, end, pi, similarity.to_bits()))
        .collect();
    out.sort_unstable();
    out
}

/// Every `(end, penalty)` reachable by aligning `pat` against `text` from `start`.
///
/// A cell of the DP is `(i, j, edits, me)`: pattern graphemes consumed, text graphemes consumed,
/// edits spent, and `me` -- the engine's `matched_end`, the text offset just past the **last pattern
/// grapheme that was aligned**.
///
/// `me` must be carried explicitly rather than derived, because it is not a function of the other
/// coordinates. It advances on exact / substitution / swap but not on an insertion, so it equals
/// `j` minus the insertions taken *after* the last alignment. Two paths can therefore reach the
/// same `j` with the same insertion count at different `me`: aligning `c`, inserting, aligning `x`
/// ends at `me == j`, while aligning `c`, aligning, inserting ends at `me == j - 1`.
///
/// The insertion guard needs no separate flag for the same reason: the engine blocks an insertion
/// only while `matched_start == matched_end && matched_start == j`, and an insertion cannot have
/// happened before the first alignment, so the condition reduces to `me > start`.
///
/// The cheapest route to a cell wins, matching the engine's dedup table keeping the lowest penalty
/// for an equivalent state.
#[allow(clippy::too_many_arguments)]
fn align_all(
    text: &[char],
    start: usize,
    pat: &[char],
    table: RefSim<'_>,
    pen: RefPenalties,
    min_symbol_similarity: f32,
    max_edits: u8,
) -> Vec<(usize, f32)> {
    let pattern_len = pat.len();
    // A match consumes at most one text grapheme per pattern grapheme, plus one per insertion.
    let max_j = (pattern_len + max_edits as usize).min(text.len() - start);
    let budget = max_edits as usize;
    // `f32::INFINITY` as the unreachable marker. The comparison below is exact by construction --
    // nothing here ever computes `inf - x`, so a tolerance comparison would be the wrong tool.
    let inf = f32::INFINITY;

    let stride = |i: usize, j: usize, edits: usize, me: usize| {
        let a = i * (max_j + 1) + j;
        let b = a * (budget + 1) + edits;
        b * (max_j + 1) + me
    };
    let mut cells = vec![inf; (pattern_len + 1) * (max_j + 1) * (budget + 1) * (max_j + 1)];
    cells[stride(0, 0, 0, 0)] = 0.0;

    let mut out = Vec::new();
    for i in 0..=pattern_len {
        for j in 0..=max_j {
            for edits in 0..=budget {
                for me in 0..=j {
                    let here = cells[stride(i, j, edits, me)];
                    #[allow(clippy::float_cmp)] // exact: see the `inf` note above
                    if here == inf {
                        continue;
                    }
                    if i == pattern_len {
                        out.push((start + me, here));
                    }
                    let at_text_end = start + j >= text.len();
                    let can_spend = edits < budget;

                    // 1) exact or substitution: one pattern grapheme, one text grapheme. Advances
                    //    `me` too -- that is what makes it the last-aligned offset.
                    if i < pattern_len && !at_text_end {
                        let s = sim(table, pat[i], text[start + j]);
                        if s >= 1.0 {
                            let to = stride(i + 1, j + 1, edits, j + 1);
                            cells[to] = cells[to].min(here);
                        } else if can_spend && s >= min_symbol_similarity {
                            let to = stride(i + 1, j + 1, edits + 1, j + 1);
                            cells[to] = cells[to].min(here + pen.substitution * (1.0 - s));
                        }
                    }
                    // 2) swap: two of each, the pair reversed. Costs one edit.
                    if can_spend
                        && i + 1 < pattern_len
                        && start + j + 1 < text.len()
                        && pat[i] == text[start + j + 1]
                        && pat[i + 1] == text[start + j]
                    {
                        let to = stride(i + 2, j + 2, edits + 1, j + 2);
                        cells[to] = cells[to].min(here + pen.swap);
                    }
                    // 3) deletion: a pattern grapheme, no text. Costs one edit.
                    if can_spend && i < pattern_len {
                        let to = stride(i + 1, j, edits + 1, me);
                        cells[to] = cells[to].min(here + pen.deletion);
                    }
                    // 4) insertion: a text grapheme, no pattern grapheme, and `me` stays put.
                    //    Blocked until something has been aligned. An insertion cannot happen before
                    //    the first alignment, so `me > 0` is the whole condition -- note the
                    //    offset is relative to the start window, so comparing against the absolute
                    //    start would block every insertion.
                    if can_spend && j < max_j && me > 0 {
                        let to = stride(i, j + 1, edits + 1, me);
                        cells[to] = cells[to].min(here + pen.insertion);
                    }
                }
            }
        }
    }
    out
}
