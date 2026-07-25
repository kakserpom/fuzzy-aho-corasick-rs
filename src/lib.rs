#![warn(clippy::pedantic)]
// The automaton deliberately stores node indices and grapheme positions as `u32` to keep the
// hot-path structs compact; the corresponding `usize -> u32` casts are sound for any realistic
// input (fewer than ~4 billion nodes, haystacks under ~4 GiB — see `search_unsorted`).
#![allow(
    clippy::too_many_lines,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation
)]

//! Unicode-aware Aho–Corasick automaton with **fuzzy matching**: substitutions, insertions,
//! deletions, and transpositions, over grapheme clusters, with optional case-insensitive folding.
//!
//! Build an immutable engine with [`FuzzyAhoCorasickBuilder`], then query it. Similarity for a
//! candidate match against a length-`N` pattern is `(N - penalties) / N * weight`, and a match is
//! kept when it meets the caller's threshold. Edit limits can be set globally on the builder or
//! per [`Pattern`], and edit costs are tuned via [`FuzzyPenalties`].
//!
//! # Example
//! ```
//! use fuzzy_aho_corasick::{FuzzyAhoCorasickBuilder, FuzzyLimits};
//!
//! let engine = FuzzyAhoCorasickBuilder::new()
//!     .fuzzy(FuzzyLimits::new().edits(1))
//!     .case_insensitive(true)
//!     .build(["hello", "world"]);
//!
//! // Two typos: an extra 'l' (insertion) and swapped 'lr' (transposition).
//! let matches = engine.search_non_overlapping("helllo wolrd", 0.8);
//! let found: Vec<&str> = matches.iter().map(|m| m.pattern.as_str()).collect();
//! assert!(found.contains(&"hello") && found.contains(&"world"));
//! ```
//!
//! # Bounding worst-case work
//! The search is exact by default. When a high edit budget is combined with a low threshold the
//! state space can explode; [`FuzzyAhoCorasickBuilder::beam_width`] caps the active frontier, and
//! [`FuzzyAhoCorasickBuilder::auto_beam`] stays exact until a state budget is exceeded and only
//! then engages a beam — leaving ordinary searches unaffected.
//!
//! # Bit-parallel pre-filter
//! For large, mostly-non-matching inputs, [`FuzzyAhoCorasick::with_prefilter`] returns a
//! [`Prefiltered`] wrapper that runs a bit-parallel approximate scan to locate candidate regions and
//! only re-searches those with the full engine. Results are identical to [`FuzzyAhoCorasick::search`];
//! it falls back to a plain full search when the configuration can't be reduced to the bit model.
//!
//! See the [README](https://github.com/kakserpom/fuzzy-aho-corasick-rs) for a full guide.
mod builder;
mod matches;
mod prefilter;
mod replacer;
mod stream;
pub mod structs;
#[cfg(test)]
mod tests;

pub use builder::FuzzyAhoCorasickBuilder;
pub use prefilter::Prefiltered;
pub use replacer::FuzzyReplacer;
use std::borrow::Cow;
use std::collections::hash_map::Entry;
use std::hash::{Hash, Hasher};
pub use stream::{StreamMatch, StreamMatches};
use unicode_segmentation::UnicodeSegmentation;
pub type PatternIndex = usize;
pub use structs::*;

/// Compile-time table of bytes `[0x00, 0x01, …, 0x7F]` so that we can return a `&'static str`
/// for any single ASCII byte without allocating. Used by the ASCII case-insensitive fast path
/// to avoid `Cow::Owned(String)` per uppercase character — each such allocation was a heap
/// miss on every grapheme access during the search.
const fn make_ascii_bytes() -> [u8; 128] {
    let mut arr = [0u8; 128];
    let mut i = 0;
    while i < 128 {
        arr[i] = i as u8;
        i += 1;
    }
    arr
}
static ASCII_BYTES: [u8; 128] = make_ascii_bytes();

/// Return a `&'static str` for a single ASCII byte (0–127) without allocating.
#[inline(always)]
fn ascii_byte_to_str(b: u8) -> &'static str {
    debug_assert!(b < 128);
    // SAFETY: all bytes 0–127 are valid one-byte UTF-8 sequences.
    unsafe { std::str::from_utf8_unchecked(&ASCII_BYTES[b as usize..b as usize + 1]) }
}

/// Abstraction over grapheme storage so the BFS hot loop can be monomorphised for both the
/// ASCII fast path (zero-allocation `&[u8]`) and the full Unicode path (`Vec<(usize, Cow<str>)>`).
/// The trait is sealed to the two internal implementors so the compiler can devirtualise every
/// call.
trait GraphemeStorage {
    fn gs_len(&self) -> usize;
    /// Byte offset of the `idx`-th grapheme within the haystack.
    fn gs_byte_offset(&self, idx: usize) -> usize;
    /// The (case-folded) grapheme text at position `idx`.
    fn gs_text(&self, idx: usize) -> &str;
    /// First `char` of the (case-folded) grapheme at position `idx`.
    /// Used by the substitution scan to avoid the `&str → chars().next().unwrap_or()` chain.
    fn gs_first_char(&self, idx: usize) -> char;
    /// Find the automaton transition from `node` for the grapheme at position `idx`.
    /// The caller passes the already-computed first `char` (`ch`) to avoid a redundant
    /// `gs_first_char` call. For ASCII storage this skips the `&str` creation, `as_bytes()`,
    /// and byte-length check that `Node::find_transition` would do, by going straight to the
    /// char-based linear scan. For Unicode storage it delegates to `find_transition` since
    /// multi-byte graphemes need the full `&str` HashMap lookup path.
    fn gs_find_transition(&self, node: &Node, idx: usize, ch: char) -> Option<u32>;
}

impl GraphemeStorage for Vec<(usize, Cow<'_, str>)> {
    #[inline]
    fn gs_len(&self) -> usize {
        self.len()
    }
    #[inline]
    fn gs_byte_offset(&self, idx: usize) -> usize {
        self[idx].0
    }
    #[inline]
    fn gs_text(&self, idx: usize) -> &str {
        self[idx].1.as_ref()
    }
    #[inline]
    fn gs_first_char(&self, idx: usize) -> char {
        self[idx].1.chars().next().unwrap_or('\0')
    }
    #[inline]
    fn gs_find_transition(&self, node: &Node, idx: usize, _ch: char) -> Option<u32> {
        node.find_transition(self.gs_text(idx))
    }
}

/// Zero-allocation grapheme storage for all-ASCII haystacks: each byte is a grapheme, and
/// case-folding is computed on the fly via the static `ascii_byte_to_str` table.
struct AsciiGraphemes<'a> {
    bytes: &'a [u8],
    case_insensitive: bool,
}

impl<'a> AsciiGraphemes<'a> {
    fn new(haystack: &'a str, case_insensitive: bool) -> Self {
        Self {
            bytes: haystack.as_bytes(),
            case_insensitive,
        }
    }
}

impl<'a> GraphemeStorage for AsciiGraphemes<'a> {
    #[inline]
    fn gs_len(&self) -> usize {
        self.bytes.len()
    }
    #[inline]
    fn gs_byte_offset(&self, idx: usize) -> usize {
        idx
    }
    #[inline]
    fn gs_text(&self, idx: usize) -> &str {
        let b = self.bytes[idx];
        if self.case_insensitive {
            ascii_byte_to_str(b.to_ascii_lowercase())
        } else {
            // SAFETY: caller guaranteed `haystack.is_ascii()`; every byte is a valid 1-byte
            // UTF-8 sequence.
            unsafe { std::str::from_utf8_unchecked(std::slice::from_ref(&self.bytes[idx])) }
        }
    }
    #[inline]
    fn gs_first_char(&self, idx: usize) -> char {
        let b = self.bytes[idx];
        if self.case_insensitive {
            b.to_ascii_lowercase() as char
        } else {
            b as char
        }
    }
    #[inline]
    fn gs_find_transition(&self, node: &Node, _idx: usize, ch: char) -> Option<u32> {
        // All graphemes are single-byte ASCII, so skip the &str creation and
        // byte-length check in `find_transition` and go straight to the char scan.
        node.find_transition_char(ch)
    }
}

/// Automaton node index (u32 for compact struct packing; >4B nodes is unrealistic).
type NodeIndex = u32;
/// Current position (grapheme index) in the haystack.
type HaystackPos = u32;
/// Start grapheme index of the matched span in the haystack.
type MatchStart = u32;
/// End grapheme index of the matched span in the haystack.
type MatchEnd = u32;

/// Key for the per-window state-dedup map: automaton position, matched span, and the four
/// per-edit-type counts packed into one `u32` (one byte each). Two states with equal keys behave
/// identically going forward, so only the lowest-penalty one needs expanding. Packing the counts
/// keeps the key at five fields, so the per-state hash mixes four words instead of eight.
///
/// The custom `Hash` impl packs pairs of `u32`s into `u64`s, reducing FxHash rounds from 5 to 2
/// `(2 × write_u64 + write_u32)`. `packed_counts` is included via a third
/// `write_u32` call — it reduces probe count significantly in multi-edit
/// search (4-edit beam: ~10% faster) while costing ~1.5% on 1-edit search.
#[derive(Clone, Copy, PartialEq, Eq)]
struct VisitedKey {
    node: NodeIndex,
    j: HaystackPos,
    matched_start: MatchStart,
    matched_end: MatchEnd,
    packed_counts: u32,
}

impl Hash for VisitedKey {
    #[inline]
    fn hash<H: Hasher>(&self, hasher: &mut H) {
        hasher.write_u64(u64::from(self.node) | (u64::from(self.j) << 32));
        hasher.write_u64(
            u64::from(self.matched_start) | (u64::from(self.matched_end) << 32),
        );
        // Including `packed_counts` in the hash dramatically reduces probe count
        // for multi-edit search (4-edit beam: ~10% faster). For 1-edit search
        // the extra write_u32 adds ~2% overhead but the net win is large in
        // absolute terms (50µs saved on beam vs 0.05µs lost on 1-edit).
        hasher.write_u32(self.packed_counts);
    }
}

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
    fn get_node_limits(&self, node: u32) -> Option<&FuzzyLimits> {
        self.nodes[node as usize]
            .pattern_index
            .and_then(|i| self.patterns.get(i).and_then(|p| p.limits.as_ref()))
    }

    /// Fast path similarity lookup with inline handling of common cases.
    /// Uses precomputed ASCII table for O(1) lookup, falls back to `HashMap` for non-ASCII.
    #[inline]
    fn get_similarity(&self, a: char, b: char) -> f32 {
        // Fast path: exact match
        if a == b {
            return 1.0;
        }
        self.similarity.get(a, b)
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
            max.edits.is_none_or(|max| edits < max)
                && max.substitutions.is_none_or(|max| substitutions < max)
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
        // Dispatch on whether any mappings exist so the multi-character-mapping branch is compiled
        // out entirely for the common (no-mapping) case, keeping the hot loop identical to before.
        if haystack.is_ascii() {
            let g = AsciiGraphemes::new(haystack, self.case_insensitive);
            let skip = self.max_edits_fast == 1;
            if self.mappings.is_empty() {
                if skip {
                    self.search_unsorted_impl::<false, true, _>(haystack, similarity_threshold, &g)
                } else {
                    self.search_unsorted_impl::<false, false, _>(haystack, similarity_threshold, &g)
                }
            } else {
                if skip {
                    self.search_unsorted_impl::<true, true, _>(haystack, similarity_threshold, &g)
                } else {
                    self.search_unsorted_impl::<true, false, _>(haystack, similarity_threshold, &g)
                }
            }
        } else {
            let g = self.build_unicode_graphemes(haystack);
            let skip = self.max_edits_fast == 1;
            if self.mappings.is_empty() {
                if skip {
                    self.search_unsorted_impl::<false, true, _>(haystack, similarity_threshold, &g)
                } else {
                    self.search_unsorted_impl::<false, false, _>(haystack, similarity_threshold, &g)
                }
            } else {
                if skip {
                    self.search_unsorted_impl::<true, true, _>(haystack, similarity_threshold, &g)
                } else {
                    self.search_unsorted_impl::<true, false, _>(haystack, similarity_threshold, &g)
                }
            }
        }
    }

    /// Build the `Vec<(usize, Cow<str>)>` grapheme list for non-ASCII haystacks.
    fn build_unicode_graphemes<'a>(&'a self, haystack: &'a str) -> Vec<(usize, Cow<'a, str>)> {
        let mut vec = Vec::new();
        vec.extend(haystack.grapheme_indices(true).map(|(byte, g)| {
            // Only allocate a lowercased copy when the grapheme could actually change. For
            // an all-ASCII grapheme with no uppercase byte (spaces, digits, punctuation, and
            // already-lowercase letters — the bulk of typical text) `to_lowercase()` is a
            // no-op, so borrow instead. Non-ASCII graphemes may still lowercase, so those
            // go the owned path.
            let needs_lowercasing = self.case_insensitive
                && (!g.is_ascii() || g.bytes().any(|b| b.is_ascii_uppercase()));
            let text = if needs_lowercasing {
                Cow::Owned(g.to_lowercase())
            } else {
                Cow::Borrowed(g)
            };
            (byte, text)
        }));
        vec
    }

    fn search_unsorted_impl<'a, const MAPPINGS: bool, const WINDOW_SKIP: bool, G: GraphemeStorage>(
        &'a self,
        haystack: &'a str,
        similarity_threshold: f32,
        graphemes: &G,
    ) -> FuzzyMatches<'a> {
        if graphemes.gs_len() == 0 {
            return FuzzyMatches {
                haystack,
                inner: vec![],
            };
        }
        // Grapheme count as `u32` for comparisons against the `u32` state positions (see the
        // crate-level note on the index/position width).
        let text_len = graphemes.gs_len() as u32;

        // Keyed by (start_byte, end_byte, pattern_index). Uses the fast FxHash hasher instead of
        // the default SipHash: keys are small integer tuples looked up on every accepted match.
        let mut best: FxHashMap<(usize, usize, usize), FuzzyMatch> = FxHashMap::default();
        best.reserve(self.patterns.len() * 4);

        // Pre-allocate queue - size based on beam width or a generous default. The default
        // of 128 avoids the first-window realloc (profiled at ~0.3% of search time with 64).
        let mut queue: Vec<State> = Vec::with_capacity(self.beam_width.unwrap_or(128));

        // Visited set for state deduplication, reused (cleared) per start window. Insertions and
        // deletions can reach the same automaton position via exponentially many distinct paths;
        // without dedup this BFS explodes in time and memory on long haystacks. Two states that
        // agree on automaton position, matched span, and per-edit-type counts behave identically
        // in the future, so only the lowest-penalty one needs to be expanded. FxHash is used
        // because the key is an integer tuple hashed once per expanded state (the hottest map).
        let mut visited: FxHashMap<VisitedKey, f32> = FxHashMap::default();
        // Pre-warm the visited map to avoid incremental rehashing (0→4→8→16…) during the
        // first few windows. Profiling showed `reserve_rehash` at ~3.5% of search time for
        // texts under 128 graphemes because the map started at capacity 0 and grew on every
        // window until stabilising. A modest pre-allocation eliminates this: the capacity is
        // retained by `clear()` between windows, so it's a one-time cost per search call.
        // For very short inputs (< 16 graphemes) the overhead of even a small allocation
        // outweighs the rehashing savings, so we skip those.
        if text_len > 16 {
            // Smaller tables reduce `clear()` memset cost between windows. The dead-end
            // filter (opt-17/18) reduces the number of states per window, so smaller
            // reserves suffice. Scale with edit budget: more edits → more states.
            let cap = match self.max_edits_fast {
                1 => 64,
                2 => 128,
                _ => 256,
            };
            visited.reserve((text_len as usize * 4).min(cap));
        }

        // Global penalty ceiling, used for the cheap push-time guards below: a state carrying more
        // penalty than this can never reach the threshold. The root reaches every pattern, so its
        // per-node coefficients give exactly the global bound (longest/heaviest pattern). See
        // `Node::prune_len` for the derivation.
        let root = &self.nodes[0];
        let max_penalties = root.prune_len - root.prune_len_over_weight * similarity_threshold;
        // Per-substitution similarity floor (0.0 = no floor); hoisted out of the hot loop.
        let min_symbol_similarity = self.min_symbol_similarity;
        // Fast-path edit ceiling (see `FuzzyAhoCorasick::max_edits_fast`). `255` disables the
        // fast path; otherwise the hot loop checks `edits <= max_edits_fast` (or `<` for
        // ahead-checks) instead of calling `within_limits_*`.
        let max_edits_fast = self.max_edits_fast;
        let has_pattern_limits = self.has_pattern_limits;

        // 2-gram window skip for 1-edit search: precompute bitmaps of root edge chars
        // (first chars) and root children's edge chars (second chars). A window can only
        // yield a match if text[start] is a first or second char (exact match or deletion),
        // or text[start+1] is a second char (substitution dead-end filter passes). This
        // skips ~70% of windows for typical inputs, saving the visited-check + edge-scan
        // overhead for non-matching windows. Only applies when: exactly 1 edit, no
        // multi-char mappings, root has no output (no empty patterns), and no root child
        // has an output (no 1-char patterns).
        let window_skip: Option<(u128, u128)> = if WINDOW_SKIP
            && !MAPPINGS
            && root.output.is_empty()
        {
            let mut first = root.single_char_edge_bits;
            let mut second = 0u128;
            let mut child_output = false;
            for edge in &root.edges {
                let child = &self.nodes[edge.next as usize];
                second |= child.single_char_edge_bits;
                first |= child.single_char_edge_bits;
                if !child.output.is_empty() {
                    child_output = true;
                }
            }
            (!child_output).then_some((first, second))
        } else {
            None
        };

        // Effective beam width. Starts at the explicit `beam_width` (if any); otherwise it stays
        // `None` (exact) until the automatic-beam budget is exhausted, at which point it drops to the
        // configured width to bound a runaway exploration. `states_expanded` is counted across all
        // start windows so the budget caps total work, not per-window work.
        let mut effective_beam = self.beam_width;
        let mut states_expanded = 0usize;

        trace!(
            "=== fuzzy_search on {haystack:?} (similarity_threshold {similarity_threshold:.2}) ===",
        );
        for start in 0..graphemes.gs_len() {
            // 2-gram window skip: cheaply reject windows that cannot produce a match.
            if let Some((first_bits, second_bits)) = window_skip {
                let ch = graphemes.gs_first_char(start);
                let ch_idx = ch as u32;
                if ch_idx < 128 && (first_bits >> ch_idx) & 1 == 0 {
                    // text[start] is not a first or second char.
                    // Check if text[start+1] is a second char (substitution dead-end filter).
                    let next_idx = start + 1;
                    if next_idx >= text_len as usize {
                        continue; // no next char — no match possible
                    }
                    let next_ch = graphemes.gs_first_char(next_idx);
                    let next_ch_idx = next_ch as u32;
                    if next_ch_idx < 128 && (second_bits >> next_ch_idx) & 1 == 0 {
                        continue; // text[start+1] not a second char — skip
                    }
                    // Non-ASCII next_ch or in second_chars: don't skip
                }
                // text[start] in first_chars or non-ASCII: don't skip
            }

            trace!(
                "=== new window at grapheme #{start} ({:?}) ===",
                graphemes.gs_text(start)
            );

            queue.clear();
            visited.clear();
            let start = start as u32;
            queue.push(State {
                node: 0,
                j: start,
                matched_start: start,
                matched_end: start,
                penalties: 0.,
                edits: 0,
                packed_counts: 0,
                #[cfg(debug_assertions)]
                notes: vec![],
            });

            let mut q_idx = 0;
            while q_idx < queue.len() {
                // Beam pruning: if queue grows too large, keep only best candidates
                if let Some(bw) = effective_beam {
                    let remaining = queue.len() - q_idx;
                    if remaining > bw * 2 {
                        // Sort remaining items by penalties (lowest first = best candidates)
                        queue[q_idx..].sort_unstable_by(|a, b| a.penalties.total_cmp(&b.penalties));
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
                    packed_counts,
                    ..
                } = queue[q_idx];
                #[cfg(debug_assertions)]
                let notes = queue[q_idx].notes.clone();
                q_idx += 1;

                // State deduplication: skip if an equal-or-better (lower-penalty) state with the
                // same automaton position, matched span, and per-edit-type counts was already
                // expanded. This collapses the exponential set of insertion/deletion paths that
                // reach the same position into a polynomial number of distinct states.
                let dedup_key = VisitedKey {
                    node,
                    j,
                    matched_start,
                    matched_end,
                    packed_counts,
                };
                // Use the entry API so the key is hashed once (a plain `get` followed by `insert`
                // hashes it twice); this map is probed on every expanded state, so that second hash
                // was a measurable slice of the hot path.
                match visited.entry(dedup_key) {
                    Entry::Occupied(mut slot) => {
                        if *slot.get() <= penalties {
                            continue;
                        }
                        slot.insert(penalties);
                    }
                    Entry::Vacant(slot) => {
                        slot.insert(penalties);
                    }
                }

                let node_ref = &self.nodes[node as usize];

                // Early pruning against this node's own (tight) ceiling: a state whose penalties
                // exceed what the longest/heaviest pattern still reachable from here allows cannot
                // yield an above-threshold match, and neither can any descendant (edits only add
                // penalties) — so pruning here cuts the entire subtree. This is tighter than the
                // global `max_penalties` used for the push guards, and it reuses the node reference
                // already loaded below, so it costs nothing extra on the hot path.
                if penalties
                    > node_ref.prune_len - node_ref.prune_len_over_weight * similarity_threshold
                {
                    continue;
                }

                let Node { output, edges, .. } = node_ref;

                // Remaining penalty budget for push-time guards. Computing this once saves
                // an FP add per guard (substitution, swap, insertion, deletion).
                let remaining = max_penalties - penalties;

                // Per-node limits are the same for every edit-type check below; compute once instead
                // of re-deriving them (a pattern lookup) up to four times per state. Skip the lookup
                // entirely in the common case where no pattern has its own limits.
                let node_limits = if has_pattern_limits {
                    self.get_node_limits(node)
                } else {
                    None
                };

                if !output.is_empty() {
                    let insertions = (packed_counts & 0xFF) as NumEdits;
                    let deletions = ((packed_counts >> 8) & 0xFF) as NumEdits;
                    let substitutions = ((packed_counts >> 16) & 0xFF) as NumEdits;
                    let swaps = ((packed_counts >> 24) & 0xFF) as NumEdits;
                    for &pattern_index in output {
                        let pattern_index = pattern_index as usize;
                        if max_edits_fast != 255 {
                            if edits > max_edits_fast {
                                continue;
                            }
                        } else if !self.within_limits(
                            self.patterns[pattern_index].limits.as_ref(),
                            edits,
                            insertions,
                            deletions,
                            substitutions,
                            swaps,
                        ) {
                            continue;
                        }
                        let start_byte = if (matched_start as usize) < graphemes.gs_len() {
                            graphemes.gs_byte_offset(matched_start as usize)
                        } else {
                            0
                        };
                        let end_byte = if (matched_end as usize) < graphemes.gs_len() {
                            graphemes.gs_byte_offset(matched_end as usize)
                        } else {
                            haystack.len()
                        };
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
                let is_last_edit = max_edits_fast != 255 && edits + 1 >= max_edits_fast;
                if j < text_len {
                    let current_ch = graphemes.gs_first_char(j as usize);
                    // For dead-end filtering: if at the last edit level, check
                    // whether text[j+1] can match any child's outgoing edge.
                    let next_ch_opt = if is_last_edit && j + 1 < text_len {
                        Some(graphemes.gs_first_char((j + 1) as usize))
                    } else {
                        None
                    };
                    let matched_start_next = if matched_end == matched_start {
                        j
                    } else {
                        matched_start
                    };

                    // Exact transition: for ASCII storage, `gs_find_transition` goes straight
                    // to the char-based edge scan, skipping `&str` creation and byte-length check.
                    let exact_next = graphemes.gs_find_transition(node_ref, j as usize, current_ch);
                    if let Some(next_node) = exact_next {
                        trace!(
                            "  match   {:>8} ─ok→ node={}  sim=1.00",
                            graphemes.gs_text(j as usize), next_node
                        );
                        queue.push(State {
                            node: next_node,
                            j: j + 1,
                            matched_start: matched_start_next,
                            matched_end: j + 1,
                            penalties,
                            edits,
                            packed_counts,
                            #[cfg(debug_assertions)]
                            notes: notes.clone(),
                        });
                    }

                    // Substitutions require scanning every outgoing edge, so only do so when a
                    // substitution is still within limits. When it is not, the exact lookup above
                    // already covered the only reachable transition.
                    let subst_ok = if max_edits_fast != 255 {
                        edits < max_edits_fast
                    } else {
                        self.within_limits_subst(node_limits, edits, (packed_counts >> 16) as NumEdits)
                    };
                    if subst_ok {
                        // `current_ch` was already computed above from `gs_first_char(j)`.
                        for edge in edges {
                            let next_node = edge.next;
                            // Skip the exact transition (already enqueued above). Its target is
                            // reached with zero penalty and no extra edit, so any edge leading to
                            // the same node — possible after minimisation merges siblings — is
                            // strictly dominated by it and needs no substitution branch.
                            if Some(next_node) == exact_next {
                                continue;
                            }
                            // substitution
                            let sim = self.get_similarity(edge.first_char, current_ch);
                            // Weakest-link floor: reject a too-dissimilar character outright.
                            if sim < min_symbol_similarity {
                                continue;
                            }
                            let penalty = self.penalties.substitution * (1.0 - sim);

                            // Skip substitutions that would push the state past the global ceiling.
                            if penalty > remaining {
                                continue;
                            }

                            // Dead-end filter: at the last edit level, the child state can
                            // only do exact match and output check. If the child has no
                            // output and no edge matching text[j+1], skip the push.
                            if is_last_edit {
                                let child = &self.nodes[next_node as usize];
                                if child.output.is_empty()
                                    && next_ch_opt.map_or(true, |ch| !child.has_matching_edge_char(ch))
                                {
                                    continue;
                                }
                            }

                            trace!(
                                "  subst {:>8?} ─sub→ {current_ch:?} \
                                 node={}  sim={:.2} pen={:.2} edits->{}",
                                edge.first_char,
                                next_node,
                                sim,
                                penalty,
                                edits + 1
                            );
                            #[cfg(debug_assertions)]
                            let mut notes = notes.clone();
                            #[cfg(debug_assertions)]
                            notes.push(format!("sub {:?} -> {current_grapheme:?} (sim={sim:.2}, pen={penalty:.2}) (subst->{}, edits->{})", edge.first_char, ((packed_counts >> 16) & 0xFF) + 1, edits + 1));

                            queue.push(State {
                                node: next_node,
                                j: j + 1,
                                matched_start: matched_start_next,
                                matched_end: j + 1,
                                penalties: penalties + penalty,
                                edits: edits + 1,
                                packed_counts: packed_counts + 0x1_0000,
                                #[cfg(debug_assertions)]
                                notes,
                            });
                        }

                        //
                        // 1b) Multi-character mappings (opt-in; e.g. "æ"↔"ae", "ks"↔"x")
                        //
                        // Compiled out entirely when `MAPPINGS` is false (the common case), so the hot
                        // loop is unchanged for callers without mappings. Each precomputed mapping
                        // consumes a fixed haystack grapheme sequence and jumps to the node the
                        // mapping's pattern-side reaches, counting as one substitution.
                        if MAPPINGS && let Some(mapping_transitions) = self.mappings.get(&node) {
                            for mt in mapping_transitions {
                                // A mapping's haystack side is a handful of graphemes at most.
                                let hlen = mt.haystack.len() as u32;
                                if j + hlen > text_len {
                                    continue;
                                }
                                let hay_matches = mt.haystack.iter().enumerate().all(|(k, g)| {
                                    graphemes.gs_text(j as usize + k) == g.as_ref()
                                });
                                if !hay_matches {
                                    continue;
                                }
                                let new_penalties = penalties + mt.penalty;
                                if new_penalties > max_penalties {
                                    continue;
                                }
                                #[cfg(debug_assertions)]
                                let mut notes = notes.clone();
                                #[cfg(debug_assertions)]
                                notes.push(format!(
                                    "map {:?} (pen={:.2}) (subst->{}, edits->{})",
                                    mt.haystack,
                                    mt.penalty,
                                    ((packed_counts >> 16) & 0xFF) + 1,
                                    edits + 1
                                ));
                                queue.push(State {
                                    node: mt.next,
                                    j: j + hlen,
                                    matched_start: matched_start_next,
                                    matched_end: j + hlen,
                                    penalties: new_penalties,
                                    edits: edits + 1,
                                    packed_counts: packed_counts + 0x1_0000,
                                    #[cfg(debug_assertions)]
                                    notes,
                                });
                            }
                        }
                    }

                    //
                    // 2) Swap (transposition of two neighboring graphemes)
                    //
                    if j + 1 < text_len && self.penalties.swap <= remaining {
                        // Use gs_find_transition to skip &str creation for ASCII storage.
                        // Pre-compute the next char so both lookups reuse it.
                        let next_ch = graphemes.gs_first_char((j + 1) as usize);
                        if let Some(node2) = graphemes
                            .gs_find_transition(node_ref, (j + 1) as usize, next_ch)
                            .and_then(|x| graphemes.gs_find_transition(&self.nodes[x as usize], j as usize, current_ch))
                            && (if max_edits_fast != 255 {
                                edits < max_edits_fast
                            } else {
                                self.within_limits_swap_ahead(
                                    self.get_node_limits(node2),
                                    edits,
                                    (packed_counts >> 24) as NumEdits,
                                )
                            })
                        {
                            #[cfg(debug_assertions)]
                            let mut notes = notes.clone();
                            #[cfg(debug_assertions)]
                            notes.push(format!(
                                "swap a:{current_grapheme:?} b:{b:?} (swaps->{}, edits->{})",
                                ((packed_counts >> 24) & 0xFF) + 1,
                                edits + 1
                            ));
                            queue.push(State {
                                node: node2,
                                j: j + 2,
                                matched_start,
                                matched_end: j + 2,
                                penalties: penalties + self.penalties.swap,
                                edits: edits + 1,
                                packed_counts: packed_counts + 0x100_0000,
                                #[cfg(debug_assertions)]
                                notes,
                            });
                        }
                    }

                    //
                    // 3a) Insertion (skip a haystack character)
                    //
                    if (matched_start != matched_end || matched_start != j)
                        && self.penalties.insertion <= remaining
                        && (if max_edits_fast != 255 {
                            edits < max_edits_fast
                        } else {
                            self.within_limits_insertion_ahead(node_limits, edits, (packed_counts & 0xFF) as NumEdits)
                        })
                        && !(is_last_edit
                            && output.is_empty()
                            && next_ch_opt.map_or(true, |ch| !node_ref.has_matching_edge_char(ch)))
                    {
                        #[cfg(debug_assertions)]
                        let mut notes = notes.clone();
                        #[cfg(debug_assertions)]
                        notes.push(format!(
                            "ins {:?} (ins->{} , edits->{})",
                            graphemes.gs_text(j as usize),
                            (packed_counts & 0xFF) + 1,
                            edits + 1
                        ));
                        queue.push(State {
                            node,
                            j: j + 1,
                            matched_start,
                            matched_end,
                            penalties: penalties + self.penalties.insertion,
                            edits: edits + 1,
                            packed_counts: packed_counts + 1,
                            #[cfg(debug_assertions)]
                            notes,
                        });
                    }
                }

                //
                // 3b) Deletion (skip a pattern character) — always, even if j == len
                //
                if self.penalties.deletion <= remaining
                    && (if max_edits_fast != 255 {
                        edits < max_edits_fast
                    } else {
                        self.within_limits_deletion_ahead(node_limits, edits, ((packed_counts >> 8) & 0xFF) as NumEdits)
                    })
                {
                    // At the last edit level the child state can only do exact match
                    // and output check. If the child has no output and no edge matching
                    // the current text char, it's a dead end — skip the push to avoid
                    // wasted pop+dedup+find_transition work.
                    let current_ch_opt = if is_last_edit && j < text_len {
                        Some(graphemes.gs_first_char(j as usize))
                    } else {
                        None
                    };
                    for edge in edges {
                        let next_node2 = edge.next;
                        if is_last_edit {
                            let child = &self.nodes[next_node2 as usize];
                            if child.output.is_empty()
                                && current_ch_opt.map_or(true, |ch| !child.has_matching_edge_char(ch))
                            {
                                continue;
                            }
                        }
                        trace!(
                            "  delete to node={next_node2} penalty={:.2}",
                            self.penalties.deletion
                        );
                        #[cfg(debug_assertions)]
                        let mut notes = notes.clone();
                        #[cfg(debug_assertions)]
                        notes.push(format!(
                            "edge_g2 {:?} (del->{:?})",
                            edge.first_char,
                            ((packed_counts >> 8) & 0xFF) + 1
                        ));
                        queue.push(State {
                            node: next_node2,
                            j,
                            matched_start,
                            matched_end,
                            penalties: penalties + self.penalties.deletion,
                            edits: edits + 1,
                            packed_counts: packed_counts + 0x100,
                            #[cfg(debug_assertions)]
                            notes,
                        });
                    }
                }
            }

            // Automatic beam: accumulate the states this window expanded and, once the running total
            // crosses the budget, beam the frontier for all remaining windows. Checked per window
            // (not per state) so the exact default path carries no hot-loop cost. `queue.len()` is
            // the number of states expanded this window (the frontier is drained to the end).
            if let Some((budget, width)) = self.auto_beam
                && effective_beam.is_none()
            {
                states_expanded += queue.len();
                if states_expanded > budget {
                    effective_beam = Some(width);
                }
            }
        }
        // Collect matches from the `best` map. The order is the hash-bucket order of FxHashMap,
        // which is deterministic (FxHash has no random seed) but unrelated to match position.
        // Downstream sort functions (`default_sort`, `non_overlapping`) use `sort_unstable_by`,
        // which produces deterministic results given a deterministic input order, so no pre-sort
        // is needed here. Users of `search_unsorted` are documented to receive matches "in no
        // particular order."
        let inner: Vec<FuzzyMatch> = best
            .into_values()
            .map(|mut m| {
                m.text = &haystack[m.start..m.end];
                m
            })
            .collect();
        FuzzyMatches { haystack, inner }
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

    /// Convenience wrapper over `search_unsorted` that applies a coverage-weighted sort.
    /// Uses `similarity * text.len()` to prefer matches that cover more text.
    ///
    /// # Parameters
    /// - `haystack`: the input text to search in.
    /// - `similarity_threshold`: minimum similarity threshold for candidates.
    ///
    /// # Returns
    /// `FuzzyMatches` with matches sorted by coverage-weighted score.
    #[inline]
    #[must_use]
    pub fn search_coverage_weighted<'a>(
        &'a self,
        haystack: &'a str,
        similarity_threshold: f32,
    ) -> FuzzyMatches<'a> {
        let mut matches = self.search_unsorted(haystack, similarity_threshold);
        matches.coverage_weighted_sort();
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

    /// Like `search_non_overlapping_unique`, but uses coverage-weighted sorting.
    /// This prefers matches that cover more text (`similarity * text.len()`),
    /// which helps when short high-similarity matches would otherwise beat
    /// longer patterns that match more of a word.
    ///
    /// # Parameters
    /// - `haystack`: the input text to search in.
    /// - `similarity_threshold`: minimum similarity threshold for candidates.
    ///
    /// # Returns
    /// `FuzzyMatches` containing a non-overlapping, pattern-unique subset of matches.
    #[must_use]
    pub fn search_non_overlapping_unique_coverage_weighted<'a>(
        &'a self,
        haystack: &'a str,
        similarity_threshold: f32,
    ) -> FuzzyMatches<'a> {
        let mut matches = self.search_coverage_weighted(haystack, similarity_threshold);
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
    /// - Conducts a non‐overlapping fuzzy search (via [`Self::search_non_overlapping`]).
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
    ///   [`Self::search_non_overlapping`].
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
