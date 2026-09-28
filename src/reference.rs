//! A deliberately naive reference implementation of fuzzy search, for tests only.
//!
//! The search in `search.rs` is a BFS with a per-window state-dedup table and a family of pruning
//! rules (per-node penalty ceilings, push-time global ceilings, dead-end filters). Each of those is
//! a *proof obligation* — if a bound is not sound, real matches silently disappear — and a test suite
//! only exercises the cases its authors thought of.
//!
//! This module instead enumerates every alignment directly, with **no pruning at all**: for each
//! start position and pattern, a small dynamic program over `(pattern grapheme, text offset, per-type
//! edit counts, span end)` that keeps the cheapest way to reach each cell. It is quadratic in the
//! pattern length, uses none of the engine's shortcuts, and is written to be obviously right rather
//! than fast. If the two agree over many random configurations, the pruning is sound; if they
//! disagree, the engine has a bug that hand-written examples would not have caught.
//!
//! Multi-character mappings and beam pruning are out of scope: this pins the edit model, which is
//! where the pruning lives.
//!
//! Not compiled outside `cfg(test)`.

use std::collections::HashMap;

/// A set of edit limits, mirroring [`crate::FuzzyLimits`] after `finalize`.
///
/// An `None` per-type cap means "not permitted", because `finalize` replaces unset per-type caps
/// with `0` whenever no total budget is set. A `Some` total budget leaves the per-type caps unset and
/// is instead checked as a sum.
// The `max_` prefixes mirror `FuzzyLimits` deliberately: the point of this type is to be a
// transcription of the engine's, so that reading one against the other shows no drift.
#[allow(clippy::struct_field_names)]
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct RefLimits {
    pub(crate) max_edits: Option<u8>,
    pub(crate) max_insertions: Option<u8>,
    pub(crate) max_deletions: Option<u8>,
    pub(crate) max_substitutions: Option<u8>,
    pub(crate) max_swaps: Option<u8>,
}

impl RefLimits {
    /// The per-type bounds these limits impose, reading them the way the engine does.
    ///
    /// These are *post-`finalize`* values, so the rule is `finalize`'s: an unset per-type cap means
    /// `0` when there is no total budget, and is bounded by that budget when there is one. The total
    /// is additionally enforced as a constraint on the sum, which is what makes a limit set carrying
    /// only `edits` bound every type.
    fn bounds(&self) -> [u8; 4] {
        let bound = |cap: Option<u8>| self.max_edits.or(cap).unwrap_or(0);
        [
            bound(self.max_insertions),
            bound(self.max_deletions),
            bound(self.max_substitutions),
            bound(self.max_swaps),
        ]
    }
}

/// A pattern to search for, in the engine's terms.
#[derive(Clone, Copy, Debug)]
pub(crate) struct RefPattern<'a> {
    pub(crate) text: &'a str,
    pub(crate) weight: f32,
    /// Limits specific to this pattern, overriding the engine-wide ones. The builder finalizes these
    /// exactly like the global ones, so the same interpretation applies.
    pub(crate) limits: Option<RefLimits>,
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

/// Knobs the engine exposes that the reference has to mirror.
#[derive(Clone, Copy, Debug)]
pub(crate) struct RefConfig {
    pub(crate) pen: RefPenalties,
    pub(crate) min_symbol_similarity: f32,
    /// Engine-wide limits, used for any pattern that does not carry its own.
    pub(crate) limits: RefLimits,
    pub(crate) threshold: f32,
    pub(crate) case_insensitive: bool,
}

/// One reported match: `(start, end, pattern_index, similarity_bits)`.
///
/// Only the span and the score are compared. The engine also reports per-type edit counts, and
/// those depend on which of several equally-cheap alignments its BFS happens to reach first, so
/// they are not part of the contract under test.
pub(crate) type RefMatch = (usize, usize, usize, u32);

/// The most edits of one type the reference DP will track. The DP's state space is the product of
/// the per-type caps, so a cap far above this would allocate gigabytes; clamping surfaces a stray
/// large budget as a test failure rather than an OOM kill. No sweep comes close.
const TRACKED_EDITS: u8 = 12;

// Edit-type slots, in the order their counts are packed into a DP index.
const INS: usize = 0;
const DEL: usize = 1;
const SUB: usize = 2;
const SWAP: usize = 3;

/// The similarity the engine uses for a substituted pair.
fn sim(table: RefSim<'_>, a: char, b: char) -> f32 {
    if a == b {
        1.0
    } else {
        table.get(&(a, b)).copied().unwrap_or(0.0)
    }
}

/// Every match the engine should report, as a sorted, canonical list.
pub(crate) fn reference_search(
    haystack: &str,
    patterns: &[RefPattern<'_>],
    table: RefSim<'_>,
    cfg: RefConfig,
) -> Vec<RefMatch> {
    let RefConfig {
        threshold,
        case_insensitive,
        ..
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
            // A pattern's own limits win; the engine-wide ones are the fallback, exactly as
            // `within_limits` resolves them.
            let limits = patterns[pi].limits.unwrap_or(cfg.limits);
            for (end, penalty) in align_all(&text, start, pat, table, cfg, limits) {
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
/// A cell of the DP is `(i, j, me, counts)`: pattern graphemes consumed, text graphemes consumed,
/// `me` -- the engine's `matched_end`, the text offset just past the **last pattern grapheme that was
/// aligned** -- and the four per-type edit counts.
///
/// Two of those exist to mirror the engine rather than for any intrinsic reason:
///
/// * `me` is not a function of the other coordinates. It advances on exact / substitution / swap but
///   not on an insertion, so it equals `j` minus the insertions taken *after* the last alignment.
///   Two paths can therefore reach the same `j` with the same insertion count at different `me`:
///   aligning `c`, inserting, aligning `x` ends at `me == j`, while aligning `c`, aligning,
///   inserting ends at `me == j - 1`. Conflating those was a real bug in the engine's own dedup.
/// * The four counts are kept separate, not summed, because the engine checks a swap against `edits`
///   and `swaps` only -- never against `substitutions`. A total budget is expressed as a sum
///   constraint on top, which makes one DP cover both the fast path (a total budget, no per-type
///   checks) and the slow path (`MAX_EDITS_FAST == 255`, per-type caps).
///
/// The insertion guard needs no separate flag: the engine blocks an insertion only while
/// `matched_start == matched_end && matched_start == j`, and an insertion cannot have happened
/// before the first alignment, so the condition reduces to `me > 0`.
///
/// The cheapest route to a cell wins, matching the engine's dedup table keeping the lowest penalty
/// for an equivalent state.
fn align_all(
    text: &[char],
    start: usize,
    pat: &[char],
    table: RefSim<'_>,
    cfg: RefConfig,
    limits: RefLimits,
) -> Vec<(usize, f32)> {
    let pattern_len = pat.len();
    let pen = cfg.pen;
    let bounds = limits.bounds();
    let total = limits.max_edits.unwrap_or(u8::MAX);

    // How many of each edit the DP has to be able to hold: the per-type bound, further capped by any
    // total and by `TRACKED_EDITS`. The DP is `O(product of the per-type caps)`, so an accidentally
    // huge cap would allocate gigabytes; clamping turns that into a loud test disagreement instead
    // of an OOM kill. The sweeps stay far below the cap.
    let per_type: [u8; 4] = std::array::from_fn(|k| bounds[k].min(total).min(TRACKED_EDITS));

    // A match consumes at most one text grapheme per pattern grapheme, plus one per insertion.
    let max_j = (pattern_len + per_type[INS] as usize).min(text.len() - start);
    let j_span = max_j + 1;

    // Mixed-radix index over the per-type counts, with a decoded table so the inner loop does no
    // arithmetic to unpack. `radix[k]` is the product of the *higher* slots' radices, so spending an
    // edit of type `k` is `c + radix[k]`.
    let mut radix = [0usize; 4];
    let mut ncomb = 1usize;
    for k in (0..4).rev() {
        radix[k] = ncomb;
        ncomb *= per_type[k] as usize + 1;
    }
    // Unpack by inverting `radix` itself, not by dividing by `per_type[k] + 1`. Those disagree
    // whenever a cap is 0: such a slot has weight 1, so it shares its increment with its neighbour,
    // and sequential division charges a deletion as an insertion. Found by the per-type-cap sweep,
    // which is the only one that produces a zero cap.
    let counts: Vec<([u8; 4], u8)> = (0..ncomb)
        .map(|c| {
            let mut v = [0u8; 4];
            let mut rest = c;
            for k in 0..4 {
                v[k] = (rest / radix[k]) as u8;
                rest %= radix[k];
            }
            (v, v.iter().sum())
        })
        .collect();
    // Whether one more edit of type `k` is allowed from this combination: under its own cap, and
    // under the total budget if there is one. An edit of any type raises the sum by one.
    let may_spend: Vec<[bool; 4]> = (0..ncomb)
        .map(|c| {
            let (v, sum) = counts[c];
            std::array::from_fn(|k| v[k] < bounds[k] && sum < total)
        })
        .collect();

    let stride =
        |i: usize, j: usize, me: usize, c: usize| ((i * j_span + j) * j_span + me) * ncomb + c;
    // `INFINITY` marks an unreachable cell, recognised with `is_infinite()` rather than an equality
    // test. Every value stored here is a non-negative penalty, so `+inf` is the only infinity that
    // can ever appear and the two are equivalent -- but asking the question we mean ("is this cell
    // unreachable?") beats asserting a sentinel.
    let mut cells = vec![f32::INFINITY; (pattern_len + 1) * j_span * j_span * ncomb];
    cells[stride(0, 0, 0, 0)] = 0.0;

    let mut out = Vec::new();
    for i in 0..=pattern_len {
        for j in 0..=max_j {
            for me in 0..=j {
                for c in 0..ncomb {
                    let here = cells[stride(i, j, me, c)];
                    if here.is_infinite() {
                        continue;
                    }
                    let spend = &may_spend[c];
                    if i == pattern_len {
                        // Fully aligned. The span ends just past the last aligned pattern grapheme.
                        out.push((start + me, here));
                    }
                    let at_text_end = start + j >= text.len();

                    // 1) exact or substitution: one pattern grapheme, one text grapheme. Advances
                    //    `me` too -- that is what makes it the last-aligned offset.
                    if i < pattern_len && !at_text_end {
                        let s = sim(table, pat[i], text[start + j]);
                        if s >= 1.0 {
                            let to = stride(i + 1, j + 1, j + 1, c);
                            cells[to] = cells[to].min(here);
                        } else if spend[SUB] && s >= cfg.min_symbol_similarity {
                            let to = stride(i + 1, j + 1, j + 1, c + radix[SUB]);
                            cells[to] = cells[to].min(here + pen.substitution * (1.0 - s));
                        }
                    }
                    // 2) swap: two of each, the pair reversed. One edit, charged to `swaps` alone.
                    if spend[SWAP]
                        && i + 1 < pattern_len
                        && start + j + 1 < text.len()
                        && pat[i] == text[start + j + 1]
                        && pat[i + 1] == text[start + j]
                    {
                        let to = stride(i + 2, j + 2, j + 2, c + radix[SWAP]);
                        cells[to] = cells[to].min(here + pen.swap);
                    }
                    // 3) deletion: a pattern grapheme, no text. `me` stays put.
                    if spend[DEL] && i < pattern_len {
                        let to = stride(i + 1, j, me, c + radix[DEL]);
                        cells[to] = cells[to].min(here + pen.deletion);
                    }
                    // 4) insertion: a text grapheme, no pattern grapheme, and `me` stays put.
                    //    Blocked until something has been aligned; since an insertion cannot precede
                    //    the first alignment, `me > 0` is the whole condition.
                    if spend[INS] && j < max_j && me > 0 {
                        let to = stride(i, j + 1, me, c + radix[INS]);
                        cells[to] = cells[to].min(here + pen.insertion);
                    }
                }
            }
        }
    }
    out
}
