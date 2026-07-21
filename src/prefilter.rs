//! Bit-parallel (Bitap / Wu–Manber) **pre-filter** for [`FuzzyAhoCorasick`].
//!
//! The full engine is exact but pays a per-start-position BFS. This module adds an opt-in fast lane:
//! a bit-parallel approximate matcher runs first at hundreds of MB/s to locate *candidate regions*,
//! and the full weighted engine then re-searches only those regions. Results are **identical** to
//! [`FuzzyAhoCorasick::search`] — the filter is a conservative over-approximation (a necessary
//! condition), so it never drops a real match; it only saves the engine from scanning text that
//! cannot contain one.
//!
//! # Soundness
//! The filter admits a region whenever the *unit-cost Levenshtein* distance between a pattern and
//! some substring is within a budget `k`. `k` is derived so that **every** match the engine could
//! accept has Levenshtein distance ≤ `k`:
//! * the score threshold caps the total penalty a kept match may carry (`P_max = N·(1 − θ/weight)`),
//! * each edit operation costs at least some minimum penalty, so the op count is bounded, and a
//!   transposition counts as 2 unit edits (its Levenshtein cost).
//!
//! When the configuration can't be reduced to this bit model (mappings present, a pattern longer
//! than 63 graphemes, a free edit that makes `k` unbounded, or `k` so large the filter stops being
//! selective), [`Prefiltered`] transparently falls back to the full search — always correct, merely
//! without the speedup.
//!
//! See `examples/bitap_prototype.rs` for the standalone algorithm + a fuzzed correctness check.

use crate::structs::FxHashMap;
use crate::{FuzzyAhoCorasick, FuzzyLimits, FuzzyMatch, FuzzyMatches};
use unicode_segmentation::UnicodeSegmentation;

/// Longest pattern (in graphemes) the `u64` bit-vectors can hold.
const MAX_PATTERN_GRAPHEMES: usize = 63;
/// Beyond this edit budget the filter stops pruning meaningfully; fall back to the full search.
const MAX_USEFUL_K: usize = 24;

/// A [`FuzzyAhoCorasick`] wrapped with an optional bit-parallel pre-filter.
///
/// Obtain one with [`FuzzyAhoCorasick::with_prefilter`]. Its [`search`](Prefiltered::search) returns
/// exactly what [`FuzzyAhoCorasick::search`] would, but skips the engine over regions the bit-parallel
/// scan proves cannot match. When the engine's configuration isn't reducible to the bit model the
/// filter is absent and every call is a plain full search.
pub struct Prefiltered<'e> {
    engine: &'e FuzzyAhoCorasick,
    filter: Option<BitapFilter>,
}

/// Precomputed, threshold-independent state for the bit-parallel scan.
struct BitapFilter {
    /// Case-folded grapheme → symbol id in `1..=len`. Id `0` is reserved for "any other symbol",
    /// which matches no pattern position (so it can only ever be consumed as an edit — conservative).
    symbol_ids: FxHashMap<String, u32>,
    /// Fast path for all-ASCII haystacks: byte → symbol id (already case-folded), `0` = other. Every
    /// ASCII byte is its own grapheme, so this reproduces the grapheme path exactly without
    /// segmenting or hashing.
    ascii_id: Box<[u32; 128]>,
    case_insensitive: bool,
    patterns: Vec<BitapPattern>,
    /// `max(1/p_ins, 1/p_del, 1/p_sub_min, 2/p_swap)` — Levenshtein ops per unit of penalty budget.
    edit_cost_mult: f32,
}

struct BitapPattern {
    /// Length in graphemes (`1..=63`).
    m: usize,
    /// Pattern weight, for the per-pattern penalty budget.
    weight: f32,
    /// `mask[id]` has bit `i` set iff the pattern's `i`-th symbol is `id`.
    mask: Vec<u64>,
    /// Upper bound on Levenshtein distance implied by this pattern's edit limits, if any. Used to
    /// tighten `k` below the penalty-derived bound; `None` means limits don't bound it.
    k_limit: Option<usize>,
}

impl FuzzyAhoCorasick {
    /// Wrap this engine with a bit-parallel pre-filter (see [`Prefiltered`]).
    ///
    /// Building the filter is cheap and done once; reuse the returned wrapper across searches. If the
    /// configuration can't be reduced to the bit model, the wrapper still works — it just performs a
    /// plain full search every time.
    ///
    /// ```
    /// use fuzzy_aho_corasick::{FuzzyAhoCorasickBuilder, FuzzyLimits};
    /// let engine = FuzzyAhoCorasickBuilder::new()
    ///     .fuzzy(FuzzyLimits::new().edits(1))
    ///     .build(["vestibulum", "consectetur"]);
    /// let pf = engine.with_prefilter();
    /// // Identical results to engine.search(..), just faster on large sparse inputs.
    /// let hits = pf.search("lorem vestibulm ipsum", 0.85);
    /// assert_eq!(hits.len(), engine.search("lorem vestibulm ipsum", 0.85).len());
    /// ```
    #[must_use]
    pub fn with_prefilter(&self) -> Prefiltered<'_> {
        Prefiltered {
            engine: self,
            filter: BitapFilter::build(self),
        }
    }
}

impl Prefiltered<'_> {
    /// Whether a usable bit-parallel filter was built. When `false`, [`search`](Self::search) is a
    /// plain full search (the configuration wasn't reducible to the bit model).
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.filter.is_some()
    }

    /// Fuzzy search with the pre-filter applied. Returns exactly what
    /// [`FuzzyAhoCorasick::search`] would for the same arguments.
    #[must_use]
    pub fn search<'a>(&'a self, haystack: &'a str, threshold: f32) -> FuzzyMatches<'a> {
        let mut matches = self.search_unsorted(haystack, threshold);
        matches.default_sort();
        matches
    }

    /// Unsorted variant, mirroring [`FuzzyAhoCorasick::search_unsorted`].
    #[must_use]
    pub fn search_unsorted<'a>(&'a self, haystack: &'a str, threshold: f32) -> FuzzyMatches<'a> {
        match &self.filter {
            Some(filter) => filter.search_unsorted(self.engine, haystack, threshold),
            None => self.engine.search_unsorted(haystack, threshold),
        }
    }
}

impl BitapFilter {
    /// Try to build a filter for `engine`; returns `None` if the config isn't reducible to the bit
    /// model (see the module docs).
    fn build(engine: &FuzzyAhoCorasick) -> Option<Self> {
        // Multi-character mappings are block edits that don't map cleanly to unit Levenshtein.
        if !engine.mappings.is_empty() {
            return None;
        }
        if engine.patterns.is_empty() {
            return None;
        }

        // Cheapest possible penalty per op. A free op would make k unbounded -> not reducible.
        let p = &engine.penalties;
        let max_sim = engine.similarity.max_off_diagonal();
        let p_sub_min = p.substitution * (1.0 - max_sim);
        let mults = [
            1.0 / p.insertion,
            1.0 / p.deletion,
            1.0 / p_sub_min,
            2.0 / p.swap,
        ];
        if mults.iter().any(|m| !m.is_finite() || *m <= 0.0) {
            return None;
        }
        let edit_cost_mult = mults.iter().copied().fold(0.0f32, f32::max);

        // Assign a symbol id to every distinct case-folded pattern grapheme.
        let mut symbol_ids: FxHashMap<String, u32> = FxHashMap::default();
        let mut patterns = Vec::with_capacity(engine.patterns.len());
        for pat in &engine.patterns {
            let graphemes: Vec<String> = fold_graphemes(&pat.pattern, engine.case_insensitive);
            let m = graphemes.len();
            if m == 0 || m > MAX_PATTERN_GRAPHEMES {
                return None;
            }
            let mut ids = Vec::with_capacity(m);
            for g in graphemes {
                let next_id = symbol_ids.len() as u32 + 1; // ids start at 1; 0 = "other"
                let id = *symbol_ids.entry(g).or_insert(next_id);
                ids.push(id);
            }
            let applicable = pat.limits.as_ref().or(engine.limits.as_ref());
            patterns.push(BitapPattern {
                m,
                weight: pat.weight,
                // Mask sizing is deferred until the alphabet is fully known (below).
                mask: ids.iter().map(|&id| u64::from(id)).collect(), // temp: store ids, rebuilt below
                k_limit: applicable.and_then(k_from_limits),
            });
        }

        // ASCII fast-path table: fold each ASCII char the way the engine would, then look up its id.
        let mut ascii_id = Box::new([0u32; 128]);
        for (b, slot) in ascii_id.iter_mut().enumerate() {
            let ch = b as u8 as char;
            let folded = if engine.case_insensitive {
                ch.to_lowercase().collect::<String>()
            } else {
                ch.to_string()
            };
            if let Some(&id) = symbol_ids.get(&folded) {
                *slot = id;
            }
        }

        let alphabet = symbol_ids.len();
        // Rebuild masks now that the alphabet size is final: mask[id] gets bit i for the i-th symbol.
        for bp in &mut patterns {
            let ids = std::mem::take(&mut bp.mask); // temp id list stashed above
            let mut mask = vec![0u64; alphabet + 1];
            for (i, &id) in ids.iter().enumerate() {
                mask[id as usize] |= 1u64 << i;
            }
            bp.mask = mask;
        }

        Some(Self {
            symbol_ids,
            ascii_id,
            case_insensitive: engine.case_insensitive,
            patterns,
            edit_cost_mult,
        })
    }

    /// Transcode the haystack to a symbol-id stream plus a parallel table of grapheme byte offsets
    /// (with a trailing sentinel = `haystack.len()`), in one linear pass.
    fn transcode(&self, haystack: &str) -> (Vec<u32>, Vec<usize>) {
        // Fast path: every ASCII byte is its own grapheme, so index the precomputed byte table
        // directly — no segmentation, no hashing.
        if haystack.is_ascii() {
            let bytes = haystack.as_bytes();
            let ids: Vec<u32> = bytes.iter().map(|&b| self.ascii_id[b as usize]).collect();
            let offsets: Vec<usize> = (0..=bytes.len()).collect();
            return (ids, offsets);
        }

        let mut ids = Vec::new();
        let mut offsets = Vec::new();
        for (byte, g) in haystack.grapheme_indices(true) {
            offsets.push(byte);
            let id = if self.case_insensitive {
                // Match the engine's per-grapheme lowercasing (borrow when it's a no-op).
                if g.is_ascii() && !g.bytes().any(|b| b.is_ascii_uppercase()) {
                    self.symbol_ids.get(g).copied()
                } else {
                    self.symbol_ids.get(g.to_lowercase().as_str()).copied()
                }
            } else {
                self.symbol_ids.get(g).copied()
            };
            ids.push(id.unwrap_or(0));
        }
        offsets.push(haystack.len());
        (ids, offsets)
    }

    /// Effective edit budget for `pat` at this `threshold`, or `None` to fall back to full search
    /// (budget too large to stay selective).
    fn k_for(&self, pat: &BitapPattern, threshold: f32) -> Option<usize> {
        let n = pat.m as f32;
        // Penalty budget a kept match may carry: (N - P)/N * weight >= threshold.
        let p_max = n * (1.0 - threshold / pat.weight);
        let k_pen = if p_max <= 0.0 {
            0
        } else {
            // Non-negative by the guard above.
            #[allow(clippy::cast_sign_loss)]
            let k = (p_max * self.edit_cost_mult).floor() as usize;
            k
        };
        let k = match pat.k_limit {
            Some(limit) => k_pen.min(limit),
            None => k_pen,
        };
        if k > MAX_USEFUL_K { None } else { Some(k) }
    }

    fn search_unsorted<'a>(
        &self,
        engine: &'a FuzzyAhoCorasick,
        haystack: &'a str,
        threshold: f32,
    ) -> FuzzyMatches<'a> {
        // Decide budgets up front; any pattern needing an unbounded/huge k forces a full search.
        let mut ks = Vec::with_capacity(self.patterns.len());
        for pat in &self.patterns {
            match self.k_for(pat, threshold) {
                Some(k) => ks.push(k),
                None => return engine.search_unsorted(haystack, threshold),
            }
        }

        let (ids, offsets) = self.transcode(haystack);
        let n = ids.len();

        // Collect candidate windows (grapheme ranges) from every pattern's bit-parallel scan.
        let mut windows: Vec<(usize, usize)> = Vec::new();
        for (pat, &k) in self.patterns.iter().zip(&ks) {
            bitap_windows(&pat.mask, pat.m, k, &ids, &mut windows);
        }
        if windows.is_empty() {
            return FuzzyMatches {
                haystack,
                inner: vec![],
            };
        }

        // Merge overlapping/adjacent grapheme windows into disjoint spans.
        windows.sort_unstable();
        let mut merged: Vec<(usize, usize)> = Vec::with_capacity(windows.len());
        for (s, e) in windows {
            match merged.last_mut() {
                Some(last) if s <= last.1 => last.1 = last.1.max(e),
                _ => merged.push((s, e)),
            }
        }

        // Run the full engine on each window slice; collect the best match per (span, pattern).
        let mut best: FxHashMap<(usize, usize, usize), FuzzyMatch<'a>> = FxHashMap::default();
        for (gs, ge) in merged {
            let bstart = offsets[gs];
            let bend = offsets[ge.min(n)];
            let sub = &haystack[bstart..bend];
            for m in engine.search_unsorted(sub, threshold) {
                let start = bstart + m.start;
                let end = bstart + m.end;
                let key = (start, end, m.pattern_index);
                let entry = best.entry(key).or_insert_with(|| FuzzyMatch {
                    start,
                    end,
                    text: &haystack[start..end],
                    ..m.clone()
                });
                if m.similarity > entry.similarity {
                    *entry = FuzzyMatch {
                        start,
                        end,
                        text: &haystack[start..end],
                        ..m.clone()
                    };
                }
            }
        }

        let mut inner: Vec<FuzzyMatch<'a>> = best.into_values().collect();
        inner.sort_unstable_by_key(|m| (m.start, m.end, m.pattern_index));
        FuzzyMatches { haystack, inner }
    }
}

/// Case-fold (when requested) and split a string into its grapheme "symbols", matching the builder's
/// trie construction so pattern symbols line up with folded haystack graphemes.
fn fold_graphemes(s: &str, case_insensitive: bool) -> Vec<String> {
    if case_insensitive {
        s.graphemes(true).map(str::to_lowercase).collect()
    } else {
        s.graphemes(true).map(str::to_string).collect()
    }
}

/// Upper bound on the Levenshtein distance a match can have under `lim`, or `None` if unbounded.
fn k_from_limits(lim: &FuzzyLimits) -> Option<usize> {
    if let Some(e) = lim.edits {
        // A total-edit budget: worst case every edit is a transposition (2 Levenshtein each), unless
        // swaps are explicitly forbidden.
        let swaps_forbidden = lim.swaps == Some(0);
        return Some(if swaps_forbidden {
            e as usize
        } else {
            2 * e as usize
        });
    }
    // No total budget: sum per-type caps (swap counts double). Any uncapped type -> unbounded.
    let i = lim.insertions? as usize;
    let d = lim.deletions? as usize;
    let s = lim.substitutions? as usize;
    let w = lim.swaps? as usize;
    Some(i + d + s + 2 * w)
}

/// Bit-parallel approximate scan (Wu–Manber, shift-AND). For every grapheme end position where some
/// start gives `levenshtein(pattern, window) <= k`, push the candidate window `[end-m-k, end]` (in
/// grapheme indices) onto `out`.
fn bitap_windows(mask: &[u64], m: usize, k: usize, ids: &[u32], out: &mut Vec<(usize, usize)>) {
    let match_bit = 1u64 << (m - 1);
    let mut r = vec![0u64; k + 1];
    let mut nr = vec![0u64; k + 1];
    // Init: d deletions of the pattern prefix are free at the start (low d bits set).
    for (d, slot) in r.iter_mut().enumerate() {
        *slot = (1u64 << d) - 1;
    }
    let span = m + k;
    for (i, &c) in ids.iter().enumerate() {
        let bc = mask[c as usize];
        nr[0] = ((r[0] << 1) | 1) & bc;
        for d in 1..=k {
            nr[d] = ((r[d] << 1) & bc)            // match / exact extension
                | ((r[d - 1] | nr[d - 1]) << 1)   // substitution (prev) + deletion (current)
                | r[d - 1]                        // insertion
                | 1; // start state stays active at every error level (begin with an edit)
        }
        // R[k] subsumes all lower error levels, so one test suffices.
        if nr[k] & match_bit != 0 {
            let end = i + 1;
            out.push((end.saturating_sub(span), end));
        }
        std::mem::swap(&mut r, &mut nr);
    }
}

#[cfg(test)]
mod tests {
    use crate::{FuzzyAhoCorasickBuilder, FuzzyLimits, FuzzyPenalties};

    /// Deterministic xorshift so the fuzz is reproducible.
    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 {
            let mut x = self.0;
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            self.0 = x;
            x
        }
    }

    /// Compare (start, end, `pattern_index`, similarity, edits) tuples so results are order-independent.
    fn key(m: &crate::FuzzyMatch) -> (usize, usize, usize, u32, u8) {
        (
            m.start,
            m.end,
            m.pattern_index,
            m.similarity.to_bits(),
            m.edits,
        )
    }

    #[test]
    fn prefilter_matches_full_search_differential() {
        let alphabet = b"abcde 1o0l";
        let vocab = ["hello", "world", "vestibulum", "abc", "lorem", "cell"];
        let mut rng = Rng(0x1234_5678_9abc_def1);
        let mut checked = 0u32;

        for trial in 0..4000u32 {
            // Random engine config.
            let npat = 1 + (rng.next() % 3) as usize;
            let patterns: Vec<&str> = (0..npat)
                .map(|_| vocab[(rng.next() as usize) % vocab.len()])
                .collect();
            let edits = (rng.next() % 3) as u8; // 0..=2
            let case_insensitive = rng.next() & 1 == 0;

            let mut builder = FuzzyAhoCorasickBuilder::new().case_insensitive(case_insensitive);
            if edits > 0 {
                builder = builder.fuzzy(FuzzyLimits::new().edits(edits));
            }
            // Occasionally exercise custom penalties (still finite/nonzero).
            if trial % 5 == 0 {
                builder = builder.penalties(
                    FuzzyPenalties::default()
                        .swap(0.6)
                        .insertion(0.5)
                        .deletion(0.8),
                );
            }
            let engine = builder.build(patterns.clone());
            let pf = engine.with_prefilter();

            // Random haystack.
            let len = (rng.next() % 60) as usize;
            let mut hay = String::new();
            for _ in 0..len {
                if rng.next().is_multiple_of(7) {
                    // Splice in a (possibly mutated) vocab word to force near-matches.
                    hay.push_str(patterns[(rng.next() as usize) % patterns.len()]);
                    hay.push(' ');
                } else {
                    hay.push(alphabet[(rng.next() as usize) % alphabet.len()] as char);
                }
            }

            let threshold = 0.6 + (rng.next() % 4) as f32 * 0.1; // 0.6..=0.9

            let mut expected: Vec<_> =
                engine.search_unsorted(&hay, threshold).iter().map(key).collect();
            let mut got: Vec<_> =
                pf.search_unsorted(&hay, threshold).iter().map(key).collect();
            expected.sort_unstable();
            got.sort_unstable();
            assert_eq!(
                expected, got,
                "mismatch (trial {trial}): patterns={patterns:?} edits={edits} ci={case_insensitive} \
                 threshold={threshold} hay={hay:?}",
            );
            checked += 1;
        }
        assert_eq!(checked, 4000);
    }

    #[test]
    fn falls_back_when_not_reducible() {
        // Mappings -> not reducible.
        let engine = FuzzyAhoCorasickBuilder::new()
            .mapping("ae", "æ")
            .build(["caesar"]);
        assert!(!engine.with_prefilter().is_active());

        // Reducible config -> active.
        let engine = FuzzyAhoCorasickBuilder::new()
            .fuzzy(FuzzyLimits::new().edits(1))
            .build(["caesar"]);
        assert!(engine.with_prefilter().is_active());
    }
}
