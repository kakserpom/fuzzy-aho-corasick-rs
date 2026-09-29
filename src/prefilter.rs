//! Wu--Manber **pre-filter** for [`FuzzyAhoCorasick`].
//!
//! The full engine is exact but pays a per-start-position BFS. This module adds an opt-in fast lane:
//! a cheap approximate matcher runs first to locate *candidate regions*, and the full weighted engine
//! then re-searches only those regions. Results are **identical** to [`FuzzyAhoCorasick::search`] —
//! the filter is a conservative over-approximation (a necessary condition), so it never drops a real
//! match; it only saves the engine from scanning text that cannot contain one.
//!
//! # How it works
//!
//! Wu--Manber's lemma: split a pattern of length `m` into `k + 1` blocks. If a text window matches it
//! within `k` unit edits, at least one block matches **exactly**, so a `q = m / (k + 1)`-length
//! substring of the pattern occurs verbatim in the window. Every pattern's blocks go into one
//! exact-match lookup table and the text is scanned **once**, so the cost is `O(text)` rather than
//! `O(text x patterns)` — which is what the per-pattern Bitap scan this replaced cost, and why that
//! scan made the pre-filter *slower* than a plain search once a corpus had more than a handful of
//! patterns.
//!
//! The keys are exact rather than hashed: pattern symbol ids start at 1, so a block of up to 8 of them
//! packs into a `u64` with no collisions at all, and a lookup is a load, a mask and a compare.
//!
//! # Soundness
//!
//! `k` is derived so that **every** match the engine could accept has Levenshtein distance at most
//! `k`:
//! * the score threshold caps the total penalty a kept match may carry (`P_max = N(1 - theta/weight)`),
//! * each edit operation costs at least some minimum penalty, so the op count is bounded, and a
//!   transposition counts as 2 unit edits (its Levenshtein cost).
//!
//! The table is shared by all patterns using the *smallest* per-pattern `q`, which is the
//! conservative choice: a shorter block is a weaker condition, so it admits more candidates and can
//! never drop a real match. `min_symbol_similarity` needs no handling — it only ever *rejects*
//! substitutions, so it can shrink the match set, never grow it.
//!
//! # When it declines to help
//!
//! Three configurations fall back to the full search rather than pretending to help:
//!
//! * the configuration cannot be reduced to this model at all — mappings present, a pattern longer
//!   than 63 graphemes, or a free edit that makes `k` unbounded;
//! * the block length would be 1, which passes almost everywhere and filters nothing;
//! * the candidate regions end up covering as much text as a plain search would. This is where a
//!   large pattern set lands: `edits(1)` bounds the block at `m / 3`, and a few hundred patterns
//!   saturate the 3-gram space of a 26-letter alphabet, so nearly every position looks like a
//!   candidate. The sliced re-search can then only add overhead on top of the same work.
//!
//! In every case the result is the plain search's, exactly — a filter that cannot pay for itself is
//! removed, not approximated.
//!
//! See `examples/bitap_prototype.rs` for the standalone Bitap algorithm + a fuzzed correctness check.

use crate::structs::FxHashMap;
use crate::{FuzzyAhoCorasick, FuzzyLimits, FuzzyMatch, FuzzyMatches, SearchError, SearchOptions};
use unicode_segmentation::UnicodeSegmentation;

/// Longest pattern (in graphemes) the `u64` bit-vectors can hold.
const MAX_PATTERN_GRAPHEMES: usize = 63;
/// Beyond this edit budget the filter stops pruning meaningfully; fall back to the full search.
const MAX_USEFUL_K: usize = 24;
/// Most distinct symbols the filter supports. Kept at 255 so the id stream fits `u8` (id `0` is the
/// "other" bucket); configs with more distinct grapheme symbols fall back to the full search.
const MAX_ALPHABET: usize = 255;

/// Grapheme-index → byte-offset mapping for a transcoded haystack.
enum Offsets {
    /// All-ASCII haystack: every byte is its own grapheme, so the offset *is* the index. Storing
    /// this avoids materialising an `n+1` element table (the single largest transcode allocation).
    Identity,
    /// Non-ASCII: explicit grapheme-start byte offsets with a trailing sentinel = `haystack.len()`.
    Table(Vec<usize>),
}

impl Offsets {
    /// Byte offset of grapheme `i` (or the sentinel for `i == grapheme count`).
    #[inline]
    fn byte(&self, i: usize) -> usize {
        match self {
            Offsets::Identity => i,
            Offsets::Table(t) => t[i],
        }
    }
}

/// A [`FuzzyAhoCorasick`] wrapped with an optional Wu--Manber pre-filter.
///
/// Obtain one with [`FuzzyAhoCorasick::with_prefilter`]. Its [`search`](Prefiltered::search) returns
/// exactly what [`FuzzyAhoCorasick::search`] would, but skips the engine over regions the bit-parallel
/// scan proves cannot match. When the engine's configuration isn't reducible to the bit model the
/// filter is absent and every call is a plain full search.
pub struct Prefiltered<'e> {
    engine: &'e FuzzyAhoCorasick,
    filter: Option<BitapFilter>,
}

/// Precomputed, threshold-independent state for the pre-filter scan.
struct BitapFilter {
    /// Case-folded grapheme → symbol id in `1..=len`. Id `0` is reserved for "any other symbol",
    /// which matches no pattern position (so it can only ever be consumed as an edit — conservative).
    symbol_ids: FxHashMap<String, u32>,
    /// Fast path for all-ASCII haystacks: byte → symbol id (already case-folded), `0` = other. Every
    /// ASCII byte is its own grapheme, so this reproduces the grapheme path exactly without
    /// segmenting or hashing.
    ascii_id: [u8; 128],
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
    /// The pattern's symbol ids, in order. This is what the `q`-gram filter needs; the filter used to
    /// run one Bitap scan per pattern over a `mask[id] -> bitmask` table, which cost
    /// `O(text x patterns)` and made the pre-filter *slower* than a plain search once a corpus had
    /// more than a handful of patterns.
    ids: Vec<u8>,
    /// Upper bound on Levenshtein distance implied by this pattern's edit limits, if any. Used to
    /// tighten `k` below the penalty-derived bound; `None` means limits don't bound it.
    k_limit: Option<usize>,
}

impl FuzzyAhoCorasick {
    /// Wrap this engine with a Wu--Manber pre-filter (see [`Prefiltered`]).
    ///
    /// Building the filter is cheap and done once; reuse the returned wrapper across searches. If the
    /// configuration can't be reduced to the bit model, the wrapper still works — it just performs a
    /// plain full search every time.
    ///
    /// ```
    /// use fuzzy_aho_corasick::{FuzzyAhoCorasickBuilder, FuzzyLimits, SearchOptions};
    /// let engine = FuzzyAhoCorasickBuilder::new()
    ///     .fuzzy(FuzzyLimits::new().edits(1))
    ///     .build(["vestibulum", "consectetur"]);
    /// let pf = engine.with_prefilter();
    /// // Identical results to engine.search(..), just faster on large sparse inputs.
    /// let hits = pf.search("lorem vestibulm ipsum", &SearchOptions::new().threshold(0.85).sorted()).unwrap();
    /// assert_eq!(hits.len(), engine.search("lorem vestibulm ipsum", &SearchOptions::new().threshold(0.85).sorted()).unwrap().len());
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
    /// Whether a usable pre-filter was built. When `false`, [`search`](Self::search) is a
    /// plain full search (the configuration wasn't reducible to the bit model).
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.filter.is_some()
    }

    /// Fuzzy search with the pre-filter applied. Returns exactly what
    /// [`FuzzyAhoCorasick::search`] would for the same `opts`.
    ///
    /// # Errors
    /// Propagates [`SearchError`] when the haystack is too large to index — see
    /// [`FuzzyAhoCorasick::search`].
    pub fn search<'a>(
        &'a self,
        haystack: &'a str,
        opts: &SearchOptions,
    ) -> Result<FuzzyMatches<'a>, SearchError> {
        let mut matches = self.raw(haystack, opts.threshold)?;
        matches.apply(opts.order, opts.overlap);
        Ok(matches)
    }

    /// Raw best-per-span matches (pre-filtered when a bit model was built), before ranking/overlap.
    fn raw<'a>(
        &'a self,
        haystack: &'a str,
        threshold: f32,
    ) -> Result<FuzzyMatches<'a>, SearchError> {
        match &self.filter {
            Some(filter) => filter.search_unsorted(self.engine, haystack, threshold),
            None => self.engine.search_raw(haystack, threshold),
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
                if id as usize > MAX_ALPHABET {
                    return None; // more distinct symbols than the u8 id stream can hold
                }
                ids.push(id as u8);
            }
            let applicable = pat.limits.as_ref().or(engine.limits.as_ref());
            patterns.push(BitapPattern {
                m,
                weight: pat.weight,
                ids,
                k_limit: applicable.and_then(k_from_limits),
            });
        }

        // ASCII fast-path table: fold each ASCII char the way the engine would, then look up its id.
        let mut ascii_id = [0u8; 128];
        for (b, slot) in ascii_id.iter_mut().enumerate() {
            let ch = b as u8 as char;
            let folded = if engine.case_insensitive {
                ch.to_lowercase().collect::<String>()
            } else {
                ch.to_string()
            };
            if let Some(&id) = symbol_ids.get(&folded) {
                *slot = id as u8; // <= MAX_ALPHABET, checked above
            }
        }

        let alphabet = symbol_ids.len();
        debug_assert!(
            (1..=alphabet).all(|a| a <= MAX_ALPHABET),
            "symbol ids must fit the u8 id stream"
        );
        let _ = alphabet;

        Some(Self {
            symbol_ids,
            ascii_id,
            case_insensitive: engine.case_insensitive,
            patterns,
            edit_cost_mult,
        })
    }

    /// Transcode the haystack to a `u8` symbol-id stream plus the grapheme→byte mapping, in one
    /// linear pass. For all-ASCII input the offset table is left implicit ([`Offsets::Identity`]) and
    /// ids come straight from the precomputed byte table — no segmentation, hashing, or `n+1` offset
    /// vector.
    fn transcode(&self, haystack: &str) -> (Vec<u8>, Offsets) {
        // Fast path: every ASCII byte is its own grapheme.
        if haystack.is_ascii() {
            let ids = haystack
                .as_bytes()
                .iter()
                .map(|&b| self.ascii_id[b as usize])
                .collect();
            return (ids, Offsets::Identity);
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
            // ids are <= MAX_ALPHABET (255) by construction, so this fits u8.
            ids.push(id.unwrap_or(0) as u8);
        }
        offsets.push(haystack.len());
        (ids, Offsets::Table(offsets))
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
    ) -> Result<FuzzyMatches<'a>, SearchError> {
        // Decide budgets up front; any pattern needing an unbounded/huge k forces a full search.
        let mut ks = Vec::with_capacity(self.patterns.len());
        for pat in &self.patterns {
            match self.k_for(pat, threshold) {
                Some(k) => ks.push(k),
                None => return engine.search_raw(haystack, threshold),
            }
        }

        let (ids, offsets) = self.transcode(haystack);
        let n = ids.len();

        // Collect candidate windows (grapheme ranges).
        let mut windows: Vec<(usize, usize)> = Vec::new();
        match self.qgram_filter(&ks) {
            Some((set, q, reach)) => qgram_windows(&set, q, reach, &ids, &mut windows),
            // Not selective enough to be worth running: a full search is the honest answer, and
            // saying so beats a filter that costs more than the thing it filters.
            None => return engine.search_raw(haystack, threshold),
        }
        if windows.is_empty() {
            return Ok(FuzzyMatches {
                haystack,
                inner: vec![],
            });
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

        // If the candidates end up covering as much text as a plain search would, the filter has
        // bought nothing and the sliced re-search -- one `search_raw` call, one `best` map and one
        // BFS queue per window -- can only add overhead on top. This is the regime a large pattern
        // set lands in: the edit budget bounds the block length at `m / (2 * edits + 1)`, so with
        // `edits(1)` it is a 3-gram, and a few hundred patterns saturate the 3-gram space of a
        // 26-letter alphabet. There the honest answer is to do the search.
        let covered: usize = merged.iter().map(|(s, e)| e - s).sum();
        if covered >= n {
            return engine.search_raw(haystack, threshold);
        }

        // Run the full engine on each window slice; collect the best match per (span, pattern).
        let mut best: FxHashMap<(usize, usize, usize), FuzzyMatch<'a>> = FxHashMap::default();
        for (gs, ge) in merged {
            let bstart = offsets.byte(gs);
            let bend = offsets.byte(ge.min(n));
            let sub = &haystack[bstart..bend];
            for m in engine.search_raw(sub, threshold)? {
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
        Ok(FuzzyMatches { haystack, inner })
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

/// A set of packed `q`-grams, open-addressed with linear probing.
///
/// The keys are *exact*, not hashed. Pattern symbol ids start at 1 (`0` is reserved for "a haystack
/// grapheme that is in no pattern"), so every `q`-gram taken from a pattern has all `q` bytes
/// non-zero, and `q <= 8` means the whole thing fits in a `u64` with no collisions at all. That
/// leaves a plain `u64` table with `0` as the empty sentinel — no hash function, no false positives
/// from collisions, and a lookup that is a load, a mask and a compare.
struct QGramSet {
    mask: usize,
    slots: Vec<u64>,
}

/// Pick a slot for `key`. The key is mixed first, and it has to be.
///
/// The packed q-gram key is not well distributed in its low bits, which is what `key & mask`
/// silently assumes. Symbol ids are small and densely numbered from 1, so a 3-gram key
/// (`id0 | id1 << 8 | id2 << 16`) against a 256-slot table is indexed by `id0` *alone*: every block
/// that starts with the same symbol lands in the same probe chain. On a corpus with few distinct
/// leading letters -- which is exactly the sparse, non-matching text a pre-filter is for -- that put
/// around a hundred blocks into a handful of chains, and because almost every lookup is a *miss*,
/// and a miss has to walk its chain all the way to an empty slot, the scan spent its time on chains
/// of dependent loads instead of on filtering.
///
/// Measured on a 24-pattern / 192 KiB sparse corpus, the scan cost ~60 ns per grapheme -- more than
/// the entire plain fuzzy search it exists to accelerate, which is why the pre-filter read as a
/// net loss almost everywhere. Taking the high bits of a golden-ratio multiply makes the slot depend
/// on all `q` symbols, and the scan drops to a couple of ns per grapheme.
#[inline]
fn slot_of(key: u64, mask: usize) -> usize {
    ((key.wrapping_mul(0x9E37_79B9_7F4A_7C15) >> 32) as usize) & mask
}

impl QGramSet {
    fn with_capacity(n: usize) -> Self {
        // Power of two, at least 2x the entries so linear probing stays short.
        let cap = (n.max(8) * 2).next_power_of_two();
        Self {
            mask: cap - 1,
            slots: vec![0u64; cap],
        }
    }

    #[inline]
    fn insert(&mut self, key: u64) {
        debug_assert!(key != 0, "0 is the empty sentinel");
        let mut i = slot_of(key, self.mask);
        loop {
            let slot = self.slots[i];
            if slot == 0 {
                self.slots[i] = key;
                return;
            }
            if slot == key {
                return;
            }
            i = (i + 1) & self.mask;
        }
    }

    #[inline]
    fn contains(&self, key: u64) -> bool {
        let mut i = slot_of(key, self.mask);
        loop {
            let slot = self.slots[i];
            if slot == 0 {
                return false;
            }
            if slot == key {
                return true;
            }
            i = (i + 1) & self.mask;
        }
    }
}

/// Largest `q` that still packs into the exact-`u64` key. Above this the key would need hashing,
/// and a longer block is a *stronger* filter, so capping is the conservative direction.
const MAX_Q: usize = 8;

/// Shortest `q` worth filtering on. Below this the "does any pattern share 2-grams with this text"
/// test passes almost everywhere, and the re-search costs more than the filter saved.
const MIN_Q: usize = 2;

impl BitapFilter {
    /// Build the multi-pattern `q`-gram filter, or `None` when the pattern set cannot be filtered
    /// selectively.
    ///
    /// # Why a `q`-gram is a sound necessary condition
    ///
    /// Wu--Manber's lemma: split a pattern of length `m` into `k + 1` blocks. If a text window
    /// matches it within `k` unit edits, at least one block matches **exactly**, so that block -- a
    /// `q = floor(m / (k + 1))`-length substring of the pattern -- appears verbatim in the window.
    ///
    /// `k` here is the budget `k_for` already derives from the threshold: the penalty a kept match
    /// may carry, divided by the cheapest penalty per edit, with a transposition charged two units
    /// so an alignment within `k` unit edits is within Levenshtein `k` and the lemma applies. A
    /// per-pattern `k_limit` only shrinks it, which keeps the lemma valid.
    ///
    /// One table serves every pattern, so the scan is `O(n)` rather than `O(n x patterns)`. Using
    /// the *smallest* per-pattern `q` is the conservative choice: a smaller block is a weaker
    /// condition, so it admits more candidates and can never drop a real match.
    ///
    /// `min_symbol_similarity` needs no handling: it only ever *rejects* substitutions, so it can
    /// shrink the match set, never grow it. Mappings are already excluded in `build`.
    ///
    /// Returns the set, the block length, and the reach: the furthest a match starting at `s` can
    /// extend, which is how wide a re-search window around a hit has to be.
    fn qgram_filter(&self, ks: &[usize]) -> Option<(QGramSet, usize, usize)> {
        debug_assert_eq!(ks.len(), self.patterns.len());
        let mut q = MAX_Q;
        let mut reach = 0usize;
        for (pat, &k) in self.patterns.iter().zip(ks) {
            // floor(m / (k + 1)), at least 1 so the division is always meaningful.
            let q_pat = (pat.m / (k + 1)).max(1);
            q = q.min(q_pat);
            reach = reach.max(pat.m + k);
        }
        if q < MIN_Q {
            return None;
        }
        // Every `q`-gram of every pattern: `m + 1 - q` of them. Counted exactly, because
        // `QGramSet` is sized from this and an undercount would fill the table and spin the probe
        // loop forever.
        let grams: usize = self.patterns.iter().map(|pat| pat.m + 1 - q).sum();
        let mut set = QGramSet::with_capacity(grams);
        for (pat, &k) in self.patterns.iter().zip(ks) {
            let q_pat = (pat.m / (k + 1)).max(1).min(q);
            let mut key = 0u64;
            for (t, &id) in pat.ids.iter().take(q_pat).enumerate() {
                key |= u64::from(id) << (8 * t);
            }
            set.insert(key);
            for t in q_pat..pat.m {
                key = (key >> 8) | (u64::from(pat.ids[t]) << (8 * (q_pat - 1)));
                set.insert(key);
            }
        }
        Some((set, q, reach))
    }
}

/// Low `q` bytes set, for masking a packed 8-byte load down to the block length.
#[inline]
fn q_mask(q: usize) -> u64 {
    if q >= 8 {
        u64::MAX
    } else {
        (1u64 << (8 * q)) - 1
    }
}

/// Scan once, pushing a re-search window around every position where a pattern `q`-gram occurs.
fn qgram_windows(
    set: &QGramSet,
    q: usize,
    reach: usize,
    ids: &[u8],
    out: &mut Vec<(usize, usize)>,
) {
    let n = ids.len();
    if n < q {
        return;
    }
    let mask = q_mask(q);
    // A match containing a hit at `i` starts at `s` with `s > i - reach` (the hit is inside the
    // window) and consumes at most `reach` graphemes, so `end <= s + reach <= i + reach`. The window
    // has to cover the match *whole* on both sides: a slice that clipped one would report a truncated
    // span, or miss it entirely when the head fell outside.
    //
    // Hits are collected first and merged into runs before being extended, because extending each hit
    // separately spends `2 * reach` graphemes of re-search per hit. Two hits closer together than
    // that are cheaper to cover with one window than two, so a run is grown across gaps up to
    // `2 * reach` and only extended once at its ends.
    let gap = 2 * reach;
    let mut run: Option<(usize, usize)> = None; // (first hit, last hit), inclusive
    for i in 0..=(n - q) {
        // One 8-byte load covers every position with at least 8 symbols to spare; the tail is rare
        // enough to assemble the key a byte at a time.
        let key = if i + 8 <= n {
            let chunk: [u8; 8] = ids[i..i + 8].try_into().expect("8 bytes");
            u64::from_le_bytes(chunk) & mask
        } else {
            let mut k = 0u64;
            for (t, &id) in ids[i..i + q].iter().enumerate() {
                k |= u64::from(id) << (8 * t);
            }
            k
        };
        if !set.contains(key) {
            continue;
        }
        match &mut run {
            // `i - last` cannot overflow: `i` only ever increases and both are bounded by `n`.
            Some((_, last)) if i - *last <= gap => *last = i,
            Some((first, last)) => {
                out.push((first.saturating_sub(reach), *last + 1 + reach));
                run = Some((i, i));
            }
            None => run = Some((i, i)),
        }
    }
    if let Some((first, last)) = run {
        out.push((first.saturating_sub(reach), last + 1 + reach));
    }
}

#[cfg(test)]
mod tests {
    use super::{QGramSet, slot_of};
    use crate::{FuzzyAhoCorasickBuilder, FuzzyLimits, FuzzyPenalties, SearchOptions};

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

    /// Assert the pre-filter reproduces the full search exactly across `trials` random configs and
    /// inputs, drawing patterns from `vocab` and filler graphemes from `filler`.
    fn differential(seed: u64, vocab: &[&str], filler: &[&str], trials: u32) {
        let mut rng = Rng(seed);
        for trial in 0..trials {
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
                    // Splice in a vocab word to force near-matches.
                    hay.push_str(patterns[(rng.next() as usize) % patterns.len()]);
                    hay.push(' ');
                } else {
                    hay.push_str(filler[(rng.next() as usize) % filler.len()]);
                }
            }

            let threshold = 0.6 + (rng.next() % 4) as f32 * 0.1; // 0.6..=0.9

            let mut expected: Vec<_> = engine
                .search(&hay, &SearchOptions::new().threshold(threshold))
                .unwrap()
                .iter()
                .map(key)
                .collect();
            let mut got: Vec<_> = pf
                .search(&hay, &SearchOptions::new().threshold(threshold))
                .unwrap()
                .iter()
                .map(key)
                .collect();
            expected.sort_unstable();
            got.sort_unstable();
            assert_eq!(
                expected, got,
                "mismatch (trial {trial}): patterns={patterns:?} edits={edits} ci={case_insensitive} \
                 threshold={threshold} hay={hay:?}",
            );
        }
    }

    #[test]
    fn prefilter_matches_full_search_ascii() {
        // ASCII haystacks exercise the byte fast-path (Offsets::Identity, ascii_id table).
        let vocab = ["hello", "world", "vestibulum", "abc", "lorem", "cell"];
        let filler = ["a", "b", "c", "d", "e", " ", "1", "o", "0", "l"];
        differential(0x1234_5678_9abc_def1, &vocab, &filler, 4000);
    }

    #[test]
    fn prefilter_matches_full_search_unicode() {
        // Non-ASCII haystacks exercise the grapheme path (Offsets::Table): multi-byte codepoints
        // and a combining-mark grapheme cluster ("e\u{0301}" = é).
        let vocab = ["café", "naïve", "Ωμέγα", "Москва", "señor", "e\u{0301}cole"];
        let filler = ["a", "é", "ñ", "ω", "м", " ", "o", "0", "e\u{0301}"];
        differential(0xdead_beef_0bad_f00d, &vocab, &filler, 4000);
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

    /// The q-gram slot must depend on the whole block, not just its low bytes.
    ///
    /// This is a performance property, not a correctness one -- a badly distributed slot still
    /// returns the right answer -- but it is worth a test because the cost was invisible in the
    /// output and severe in the time. Symbol ids are small and densely numbered from 1, so packing a
    /// 3-gram as `id0 | id1 << 8 | id2 << 16` and indexing a 256-slot table on the raw low bits
    /// selects on `id0` alone. Every block that began with the same symbol then shared one probe
    /// chain, and since a miss has to walk its chain to an empty slot, real text spent its time on
    /// dependent loads: the scan measured ~60 ns per grapheme, more than the entire plain search it
    /// exists to accelerate, which made the pre-filter a net loss in every shape measured.
    ///
    /// The keys below are the adversarial case -- a hundred blocks that all share their lowest byte,
    /// which is what a corpus with few distinct leading graphemes produces. Indexing on the low bits
    /// puts all hundred in one chain; mixing first spreads them.
    #[test]
    fn qgram_slots_depend_on_the_whole_block() {
        // Shared low byte, distinct bytes above it.
        let keys: Vec<u64> = (1..=100u64).map(|k| (k << 8) | 7).collect();

        let mut set = QGramSet::with_capacity(keys.len());
        for &k in &keys {
            set.insert(k);
        }

        // Correctness first: the table still answers correctly for every key, hit or miss.
        for &k in &keys {
            assert!(set.contains(k), "inserted key {k} was not found");
        }
        assert!(
            !set.contains(0),
            "the empty sentinel must never be reported as present"
        );
        assert!(
            !set.contains((101 << 8) | 7),
            "an absent key was reported present"
        );

        // Now the property: the starting slots have to be spread out.
        let distinct: std::collections::HashSet<usize> =
            keys.iter().map(|&k| slot_of(k, set.mask)).collect();
        assert!(
            distinct.len() * 2 >= keys.len(),
            "only {} of {} blocks got distinct slots -- the slot is still being taken from the low \
             bits, so blocks sharing a leading symbol collide into one probe chain",
            distinct.len(),
            keys.len()
        );

        // A miss must not have to walk a long chain, which is what made the scan slow.
        let longest = (0..4096u64)
            .map(|t| (t << 16) | (t & 0xFF))
            .map(|k| {
                let mut i = slot_of(k, set.mask);
                let mut steps = 0;
                while set.slots[i] != 0 {
                    steps += 1;
                    i = (i + 1) & set.mask;
                }
                steps
            })
            .max()
            .unwrap();
        assert!(
            longest <= 8,
            "a miss walked {longest} slots; a well-distributed table at this load factor stays short"
        );
    }
}
