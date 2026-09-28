//! Core fuzzy search: the monomorphized BFS over grapheme storage and its helpers.
use crate::grapheme::{AsciiGraphemes, GraphemeStorage};
use crate::structs::{FxHashMap, Node, Similarity, State};
use crate::{
    FuzzyAhoCorasick, FuzzyLimits, FuzzyMatch, FuzzyMatches, NumEdits, Pattern, SearchError,
};
use std::borrow::Cow;
use std::hash::{Hash, Hasher};
use unicode_segmentation::UnicodeSegmentation;

/// Automaton node index (u32 for compact struct packing; >4B nodes is unrealistic).
type NodeIndex = u32;
/// Current position (grapheme index) in the haystack.
type HaystackPos = u32;
/// Start grapheme index of the matched span in the haystack.
type MatchStart = u32;

/// Key for the per-window state-dedup map: automaton position, matched span start, and the four
/// per-edit-type counts packed into one `u32` (one byte each). Two states with equal keys behave
/// identically going forward, so only the lowest-penalty one needs expanding.
///
/// The span *end* is deliberately not part of the key: every transition either advances `j` and
/// `matched_end` together (exact / substitution / swap / mapping) or advances `j` alone (insertion)
/// or neither (deletion), so the invariant `matched_end == j - insertions` holds from the initial
/// state onwards. `insertions` is a byte of `packed_counts`, hence the end is recoverable from
/// fields already in the key. Dropping it takes the key from five fields to four, which halves the
/// number of words the per-state hash has to mix.
///
/// The custom `Hash` impl packs each pair of `u32`s into a `u64`, so the whole key costs two
/// `FxHash` rounds (two dependent multiply chains) instead of one round per field. `packed_counts`
/// shares the second round with `matched_start`: it does not reduce the number of rounds, but it
/// does spread the keys, which measurably lowers the probe count in multi-edit search.
#[derive(Clone, Copy, PartialEq, Eq)]
struct VisitedKey {
    node: NodeIndex,
    j: HaystackPos,
    matched_start: MatchStart,
    packed_counts: u32,
}

impl Hash for VisitedKey {
    #[inline]
    fn hash<H: Hasher>(&self, hasher: &mut H) {
        hasher.write_u64(u64::from(self.node) | (u64::from(self.j) << 32));
        hasher.write_u64(u64::from(self.matched_start) | (u64::from(self.packed_counts) << 32));
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

/// Similarity of a substituted pair, with the exact-match case short-circuited before the table
/// lookup. A free function so the hot loop can hoist the `&Similarity` out of `&self` (see the
/// locals bound at the top of `search_unsorted_impl`) instead of re-loading it for every candidate
/// edge — the BFS writes to `queue`/`visited`, which the optimiser must assume might alias the
/// engine.
#[inline]
fn similarity_of(similarity: &Similarity, a: char, b: char) -> f32 {
    if a == b { 1.0 } else { similarity.get(a, b) }
}

/// Where the exact scan's current position ends, in both units the scan needs: the grapheme index
/// (to derive each reported pattern's start) and the byte offset (the span's exclusive end).
#[derive(Clone, Copy)]
struct ExactEnd {
    grapheme: usize,
    byte: usize,
}

/// One slot of [`DedupTable`]. Key, value and generation stamp live inline so a probe reads a
/// single cache line.
#[derive(Clone, Copy)]
struct DedupSlot {
    /// Window generation this slot was written in. A slot stamped with an older generation is
    /// treated as empty, which is what makes the per-window reset O(1).
    epoch: u32,
    /// Full 64-bit fingerprint of the key. Checked before the key itself so that two distinct keys
    /// landing in the same slot are rejected without touching the 16-byte key.
    hash: u64,
    key: VisitedKey,
    penalty: f32,
}

/// Per-window state-dedup table: open addressing with linear probing over a power-of-two table.
///
/// This is the hottest structure in the search — one probe per expanded state, and 13M of them on
/// a 250 KiB haystack against a 500-pattern automaton — so it is hand-rolled rather than a
/// `HashMap` for three reasons:
/// * **O(1) reset.** Windows are started hundreds of thousands of times per search;
///   `HashMap::clear` memsets the whole control array each time, whereas bumping an epoch
///   invalidates every slot at once.
/// * **A cheaper hash.** One multiply-and-shift, versus two dependent `FxHash` rounds.
/// * **One cache line per probe.** Hashbrown keeps keys and values together but pays for a
///   separate control array and its group-probe bookkeeping; here the stamp, fingerprint, key and
///   penalty are contiguous in a single 32-byte slot.
///
/// The load factor is kept at or below 3/4 by growing, so linear probing stays short.
struct DedupTable {
    slots: Box<[DedupSlot]>,
    /// `slots.len() - 1`; the table length is always a power of two.
    mask: usize,
    /// Current window generation. Starts at 1 so that the all-zero `slots` read as empty.
    epoch: u32,
    /// Slots occupied in the current generation, used to decide when to grow.
    len: usize,
}

impl DedupTable {
    /// Multiplicative constant (Fibonacci hashing) for the slot index.
    const K: u64 = 0x9E37_79B9_7F4A_7C15;

    /// A table sized for `expected` states per window. `expected == 0` builds a table that is never
    /// used (the exact search, which needs no dedup at all).
    fn new(expected: usize) -> Self {
        let slots = expected
            .max(8)
            .checked_next_power_of_two()
            .unwrap_or(1 << 20);
        Self {
            slots: Self::empty_slots(slots),
            mask: slots - 1,
            epoch: 1,
            len: 0,
        }
    }

    /// Discard every entry, in constant time, by advancing the window generation.
    fn next_window(&mut self) {
        self.epoch = self.epoch.wrapping_add(1);
        if self.epoch == 0 {
            // Wrapped past `u32::MAX`: the stamps can no longer distinguish old slots from
            // empty ones, so wipe them. Unreachable in practice (a search is bounded by the
            // haystack's grapheme count), but cheap to keep correct.
            self.slots.iter_mut().for_each(|s| s.epoch = 0);
            self.epoch = 1;
        }
        self.len = 0;
    }

    /// Fingerprint of a key. The key's four `u32`s are folded into two words, mixed with one
    /// multiply, and the *high* half of the product picks the slot (classic Fibonacci hashing, so
    /// the low bits of the inputs still spread). Collisions cannot lose an entry: the full
    /// fingerprint and then the key itself are compared before any slot is overwritten.
    #[inline]
    fn hash_key(key: &VisitedKey) -> u64 {
        let a = u64::from(key.node) | (u64::from(key.j) << 32);
        let b = u64::from(key.matched_start) | (u64::from(key.packed_counts) << 32);
        (a ^ b.rotate_left(29)).wrapping_mul(Self::K)
    }

    /// Record that `key` has been expanded with `penalty`, and report the penalty of a
    /// previously-expanded equal key if it is `<=` the new one.
    ///
    /// The caller skips the state when this returns `Some(seen) <= penalties`: an equal or better
    /// (lower-penalty) state with the same automaton position, span and edit counts was already
    /// expanded, and being identical in every respect that affects the future, its subtree has
    /// been (or is being) explored at least as well. A stored *higher* penalty is overwritten,
    /// since this state is strictly better for the same future.
    #[inline]
    fn probe_and_update(&mut self, key: VisitedKey, penalty: f32) -> Option<f32> {
        let hash = Self::hash_key(&key);
        let mut idx = ((hash >> 32) as usize) & self.mask;
        loop {
            let slot = &mut self.slots[idx];
            if slot.epoch == self.epoch {
                if slot.hash == hash && slot.key == key {
                    if slot.penalty <= penalty {
                        return Some(slot.penalty);
                    }
                    slot.penalty = penalty;
                    return None;
                }
            } else {
                slot.epoch = self.epoch;
                slot.hash = hash;
                slot.key = key;
                slot.penalty = penalty;
                self.len += 1;
                if self.len * 4 > self.slots.len() * 3 {
                    self.grow();
                }
                return None;
            }
            idx = (idx + 1) & self.mask;
        }
    }

    /// Double the table and reinsert the current window's entries, keeping the load factor at or
    /// below 3/4 so linear probes stay short.
    fn grow(&mut self) {
        let old = std::mem::take(&mut self.slots);
        // The length must stay a power of two: `mask = len - 1` is what turns the linear probe
        // into a wrap-around, and that only works if `len` is a power of two.
        self.slots = Self::empty_slots(old.len() * 2);
        self.mask = self.slots.len() - 1;
        self.len = 0;
        for slot in old {
            if slot.epoch == self.epoch {
                self.reinsert(slot);
            }
        }
    }

    /// Insert a known-unoccupied key/value pair during a rehash (no growth check).
    #[inline]
    fn reinsert(&mut self, slot: DedupSlot) {
        let mut idx = ((slot.hash >> 32) as usize) & self.mask;
        while self.slots[idx].epoch == self.epoch {
            idx = (idx + 1) & self.mask;
        }
        self.slots[idx] = DedupSlot {
            epoch: self.epoch,
            ..slot
        };
        self.len += 1;
    }

    /// `len` zeroed slots. An `epoch` of 0 marks them all as empty, which is why the generation
    /// counter starts at 1.
    fn empty_slots(len: usize) -> Box<[DedupSlot]> {
        debug_assert!(
            len.is_power_of_two(),
            "probe mask requires a power-of-two table"
        );
        vec![
            DedupSlot {
                epoch: 0,
                hash: 0,
                key: VisitedKey {
                    node: 0,
                    j: 0,
                    matched_start: 0,
                    packed_counts: 0,
                },
                penalty: 0.0,
            };
            len
        ]
        .into_boxed_slice()
    }
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

    /// Core fuzzy search over the haystack producing raw matches without any global ordering or
    /// overlap resolution. Explores all state transitions (substitutions, swaps, insertions,
    /// deletions) from each grapheme position, keeping the best match per unique
    /// (`start_byte`, `end_byte`, `pattern_index`) span above `similarity_threshold`. The public
    /// [`search`](Self::search) applies ranking/overlap on top of this.
    ///
    /// # Errors
    /// Returns [`SearchError::HaystackTooLarge`] if `haystack` has more than `u32::MAX` grapheme
    /// clusters — see [`search`](Self::search).
    #[inline]
    pub(crate) fn search_raw<'a>(
        &'a self,
        haystack: &'a str,
        similarity_threshold: f32,
    ) -> Result<FuzzyMatches<'a>, SearchError> {
        // With no edit budget at all, the general BFS below degenerates into a trie walk restarted
        // at every start position — O(n x states-per-window) for a job one left-to-right pass does
        // in O(n). Hand those configurations to the single-pass Aho-Corasick scan instead. Skipped
        // when the root itself has output (an empty pattern), whose "match everywhere" semantics
        // the scan does not model.
        if self.max_edits_fast == 0 && self.nodes[0].output.is_empty() {
            return Ok(if haystack.is_ascii() {
                let g = AsciiGraphemes::new(haystack, self.case_insensitive, self.edge_skip);
                if u32::try_from(g.gs_len()).is_err() {
                    return Err(SearchError::HaystackTooLarge {
                        graphemes: g.gs_len(),
                    });
                }
                self.search_exact(haystack, similarity_threshold, &g)
            } else {
                let g = self.build_unicode_graphemes(haystack);
                if u32::try_from(g.gs_len()).is_err() {
                    return Err(SearchError::HaystackTooLarge {
                        graphemes: g.gs_len(),
                    });
                }
                self.search_exact(haystack, similarity_threshold, &g)
            });
        }
        // Precompute a Vec<char> for the text so search_unsorted_impl can use direct slice
        // indexing instead of the GraphemeStorage::gs_first_char method (which has a match on
        // the enum discriminant, albeit predictable). This eliminates the enum dispatch overhead
        // in the hot loop (~2 calls per expanded state).
        Ok(if haystack.is_ascii() {
            let g = AsciiGraphemes::new(haystack, self.case_insensitive, self.edge_skip);
            if u32::try_from(g.gs_len()).is_err() {
                return Err(SearchError::HaystackTooLarge {
                    graphemes: g.gs_len(),
                });
            }
            let text_chars: Vec<char> = (0..g.gs_len()).map(|i| g.gs_first_char(i)).collect();
            if self.mappings.is_empty() {
                match self.max_edits_fast {
                    1 => self.search_unsorted_impl::<false, true, 1, _>(
                        haystack,
                        similarity_threshold,
                        &g,
                        &text_chars,
                    ),
                    2 => self.search_unsorted_impl::<false, false, 2, _>(
                        haystack,
                        similarity_threshold,
                        &g,
                        &text_chars,
                    ),
                    3 => self.search_unsorted_impl::<false, false, 3, _>(
                        haystack,
                        similarity_threshold,
                        &g,
                        &text_chars,
                    ),
                    4 => self.search_unsorted_impl::<false, false, 4, _>(
                        haystack,
                        similarity_threshold,
                        &g,
                        &text_chars,
                    ),
                    5 => self.search_unsorted_impl::<false, false, 5, _>(
                        haystack,
                        similarity_threshold,
                        &g,
                        &text_chars,
                    ),
                    6 => self.search_unsorted_impl::<false, false, 6, _>(
                        haystack,
                        similarity_threshold,
                        &g,
                        &text_chars,
                    ),
                    _ => self.search_unsorted_impl::<false, false, 255, _>(
                        haystack,
                        similarity_threshold,
                        &g,
                        &text_chars,
                    ),
                }
            } else {
                match self.max_edits_fast {
                    1 => self.search_unsorted_impl::<true, true, 1, _>(
                        haystack,
                        similarity_threshold,
                        &g,
                        &text_chars,
                    ),
                    2 => self.search_unsorted_impl::<true, false, 2, _>(
                        haystack,
                        similarity_threshold,
                        &g,
                        &text_chars,
                    ),
                    3 => self.search_unsorted_impl::<true, false, 3, _>(
                        haystack,
                        similarity_threshold,
                        &g,
                        &text_chars,
                    ),
                    4 => self.search_unsorted_impl::<true, false, 4, _>(
                        haystack,
                        similarity_threshold,
                        &g,
                        &text_chars,
                    ),
                    5 => self.search_unsorted_impl::<true, false, 5, _>(
                        haystack,
                        similarity_threshold,
                        &g,
                        &text_chars,
                    ),
                    6 => self.search_unsorted_impl::<true, false, 6, _>(
                        haystack,
                        similarity_threshold,
                        &g,
                        &text_chars,
                    ),
                    _ => self.search_unsorted_impl::<true, false, 255, _>(
                        haystack,
                        similarity_threshold,
                        &g,
                        &text_chars,
                    ),
                }
            }
        } else {
            let g = self.build_unicode_graphemes(haystack);
            if u32::try_from(g.gs_len()).is_err() {
                return Err(SearchError::HaystackTooLarge {
                    graphemes: g.gs_len(),
                });
            }
            let text_chars: Vec<char> = (0..g.gs_len()).map(|i| g.gs_first_char(i)).collect();
            if self.mappings.is_empty() {
                match self.max_edits_fast {
                    1 => self.search_unsorted_impl::<false, true, 1, _>(
                        haystack,
                        similarity_threshold,
                        &g,
                        &text_chars,
                    ),
                    2 => self.search_unsorted_impl::<false, false, 2, _>(
                        haystack,
                        similarity_threshold,
                        &g,
                        &text_chars,
                    ),
                    3 => self.search_unsorted_impl::<false, false, 3, _>(
                        haystack,
                        similarity_threshold,
                        &g,
                        &text_chars,
                    ),
                    4 => self.search_unsorted_impl::<false, false, 4, _>(
                        haystack,
                        similarity_threshold,
                        &g,
                        &text_chars,
                    ),
                    5 => self.search_unsorted_impl::<false, false, 5, _>(
                        haystack,
                        similarity_threshold,
                        &g,
                        &text_chars,
                    ),
                    6 => self.search_unsorted_impl::<false, false, 6, _>(
                        haystack,
                        similarity_threshold,
                        &g,
                        &text_chars,
                    ),
                    _ => self.search_unsorted_impl::<false, false, 255, _>(
                        haystack,
                        similarity_threshold,
                        &g,
                        &text_chars,
                    ),
                }
            } else {
                match self.max_edits_fast {
                    1 => self.search_unsorted_impl::<true, true, 1, _>(
                        haystack,
                        similarity_threshold,
                        &g,
                        &text_chars,
                    ),
                    2 => self.search_unsorted_impl::<true, false, 2, _>(
                        haystack,
                        similarity_threshold,
                        &g,
                        &text_chars,
                    ),
                    3 => self.search_unsorted_impl::<true, false, 3, _>(
                        haystack,
                        similarity_threshold,
                        &g,
                        &text_chars,
                    ),
                    4 => self.search_unsorted_impl::<true, false, 4, _>(
                        haystack,
                        similarity_threshold,
                        &g,
                        &text_chars,
                    ),
                    5 => self.search_unsorted_impl::<true, false, 5, _>(
                        haystack,
                        similarity_threshold,
                        &g,
                        &text_chars,
                    ),
                    6 => self.search_unsorted_impl::<true, false, 6, _>(
                        haystack,
                        similarity_threshold,
                        &g,
                        &text_chars,
                    ),
                    _ => self.search_unsorted_impl::<true, false, 255, _>(
                        haystack,
                        similarity_threshold,
                        &g,
                        &text_chars,
                    ),
                }
            }
        })
    }

    /// Single-pass exact search: a classic Aho-Corasick scan over the grapheme stream.
    ///
    /// Used when `max_edits_fast == 0`, where the general BFS is a pure trie walk restarted at
    /// every start position. That costs O(n x states-per-window); this is O(n) amortised, because
    /// the automaton's failure links carry the partial match across positions instead of
    /// rediscovering it. Measured on a 250 KiB haystack with 4 patterns: 15.8 ns/byte down to
    /// ~1.7, which is parity with the purpose-built `aho-corasick` crate (1.6 ns/byte).
    ///
    /// The builder already propagates each node's `output` along the failure chain, so a node's
    /// output list is exactly the set of patterns that are suffixes of the text consumed so far.
    /// Each is reported at its own span `[end - grapheme_len, end)`, which is where this also
    /// becomes *more* correct than the BFS: restarting at each start position, the walk consults
    /// the fail-propagated output of a node it reached by consuming more graphemes than the
    /// pattern has, so a pattern that is a suffix of a longer match gets reported a second time
    /// at the walker's whole span — `"cd"` came back as `[0,4)` / `"abcd"` for the patterns
    /// `["abcd", "cd"]` on `"abcd"`. A single pass knows each pattern's length, so it reports the
    /// real span only, and that spurious match disappears.
    fn search_exact<'a, G: GraphemeStorage>(
        &'a self,
        haystack: &'a str,
        similarity_threshold: f32,
        graphemes: &G,
    ) -> FuzzyMatches<'a> {
        let text_len = graphemes.gs_len();
        if text_len == 0 {
            return FuzzyMatches {
                haystack,
                inner: vec![],
            };
        }
        let mut inner: Vec<FuzzyMatch> = Vec::new();

        let nodes = &self.nodes;
        // Hoisted: whether any pattern is a proper suffix of another, and so whether a failure
        // ancestor can have output of its own. False for all but the most nested pattern sets.
        let has_suffix_patterns = self.has_suffix_patterns;
        let mut state: u32 = 0;
        let mut i = 0;
        while i < text_len {
            // When no match is in progress (`state` is the root), a grapheme with no outgoing
            // root edge cannot start one, and consuming it would leave the state at the root
            // anyway. Skip the whole run of such graphemes in one go. This is the same
            // first-occurrence prefilter `aho-corasick` and `regex` apply, and it is what closes
            // most of the remaining gap to them on text where matches are sparse.
            if state == 0 {
                i = graphemes.gs_skip_unstartable(i);
                if i >= text_len {
                    break;
                }
            }
            let ch = graphemes.gs_first_char(i);
            let ch_ascii = (ch as u32) < 128;
            // Advance one grapheme, descending failure links until some state has a transition on
            // `ch` (or we fall off the root, which matches the empty string). Amortised O(1):
            // every failure-link step strictly decreases the state's depth, and each successful
            // transition increases it by one.
            loop {
                let node = &nodes[state as usize];
                // O(1) reject before the edge scan: on real text nearly every transition misses,
                // and the bitmap answers "does this state have an edge for this ASCII char?"
                // exactly (see `Node::edge_bits`). Non-ASCII `ch` has no bit and always scans.
                if ch_ascii && (node.edge_bits >> (ch as u32)) & 1 == 0 {
                    if state == 0 {
                        break;
                    }
                    state = node.fail;
                    continue;
                }
                if let Some(next) = graphemes.gs_find_transition(node, i, ch) {
                    state = next;
                    break;
                }
                if state == 0 {
                    break;
                }
                state = node.fail;
            }

            // Report every pattern ending at this position. Kept out of line so that the common
            // position -- no pattern ends here -- costs one test and no extra code in the loop body.
            //
            // `output` holds only the patterns that end at their own node, so a pattern that is a
            // *suffix* of the consumed text lives on a failure ancestor; `report_exact_at` walks
            // the chain for those when the engine says any exist. When none do -- the usual case --
            // the reached node is the only candidate and the walk is skipped entirely.
            if !nodes[state as usize].output.is_empty() || has_suffix_patterns {
                let end_grapheme = i + 1;
                let end_byte = if end_grapheme == text_len {
                    haystack.len()
                } else {
                    graphemes.gs_byte_offset(end_grapheme)
                };
                self.report_exact_at(
                    haystack,
                    graphemes,
                    similarity_threshold,
                    ExactEnd {
                        grapheme: end_grapheme,
                        byte: end_byte,
                    },
                    state,
                    &mut inner,
                );
            }
            i += 1;
        }

        // No dedup pass is needed, and this is worth spelling out: the span of a reported pattern
        // is `(end - grapheme_len, end)`, so `(start, end, pattern_index)` is in bijection with
        // `(end, pattern_index)`. The loop visits each end position once and each node's `output`
        // holds no duplicate indices, so every pushed match is a distinct key — the general
        // search's `(start, end, pattern)` -> best-penalty map has nothing to merge here. The
        // output is therefore in ascending end order, which `search_unsorted` leaves unspecified
        // and every ranked/overlap-resolved order sorts anyway.
        FuzzyMatches { haystack, inner }
    }

    /// Append every match of the exact scan that ends at `end`.
    ///
    /// Out of line on purpose: the exact scan's inner loop is the hot path for large haystacks, and
    /// most positions report nothing, so this body should not sit in it.
    #[inline(never)]
    fn report_exact_at<'a, G: GraphemeStorage>(
        &'a self,
        haystack: &'a str,
        graphemes: &G,
        similarity_threshold: f32,
        end: ExactEnd,
        state: u32,
        inner: &mut Vec<FuzzyMatch<'a>>,
    ) {
        let nodes = &self.nodes;
        let mut reporter = state;
        while reporter != 0 {
            let node = &nodes[reporter as usize];
            for &pattern_index in &node.output {
                let pattern = &self.patterns[pattern_index as usize];
                // No edits are permitted on this path, so every match scores exactly its weight.
                let similarity = pattern.weight;
                if similarity < similarity_threshold {
                    continue;
                }
                // A reported pattern ends at this position, so its length can never exceed the
                // graphemes consumed and this cannot underflow.
                let start_byte = graphemes.gs_byte_offset(end.grapheme - pattern.grapheme_len);
                inner.push(FuzzyMatch {
                    insertions: 0,
                    deletions: 0,
                    substitutions: 0,
                    edits: 0,
                    swaps: 0,
                    pattern_index: pattern_index as usize,
                    start: start_byte,
                    end: end.byte,
                    pattern,
                    similarity,
                    text: &haystack[start_byte..end.byte],
                });
            }
            if !self.has_suffix_patterns {
                // Nothing inherits through failure links, so the reached node was the only
                // candidate.
                break;
            }
            reporter = node.fail;
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

    fn search_unsorted_impl<
        'a,
        const MAPPINGS: bool,
        const WINDOW_SKIP: bool,
        const MAX_EDITS_FAST: u8,
        G: GraphemeStorage,
    >(
        &'a self,
        haystack: &'a str,
        similarity_threshold: f32,
        graphemes: &G,
        text_chars: &[char],
    ) -> FuzzyMatches<'a> {
        if text_chars.is_empty() {
            return FuzzyMatches {
                haystack,
                inner: vec![],
            };
        }
        // Grapheme count as `u32` for comparisons against the `u32` state positions. The public
        // `search_unsorted` has already rejected haystacks whose grapheme count exceeds `u32::MAX`,
        // so this cast never truncates.
        let text_len = text_chars.len() as u32;

        // Keyed by (start_byte, end_byte, pattern_index). Uses the fast FxHash hasher instead of
        // the default SipHash: keys are small integer tuples looked up on every accepted match.
        let mut best: FxHashMap<(usize, usize, usize), FuzzyMatch> = FxHashMap::default();
        // Reserve only a token amount. `best` holds one entry per accepted match span, and an entry
        // is ~90 bytes (a 24-byte key plus the full `FuzzyMatch`), so sizing the table off the
        // pattern count is a bad proxy: a 500-pattern automaton used to reserve ~90 KiB per search
        // call even when the search finds nothing at all. The overwhelming majority of searches
        // return a handful of matches, and growing from a small table costs a couple of rehashes
        // against a search that has already done thousands of expansions. `reserve` is a capacity
        // hint only — results are unaffected.
        best.reserve(8);

        // Pre-allocate the queue. A state's count per window is bounded by the search's branching
        // factor, not by the haystack, so this is deliberately small: capping it by the haystack
        // length keeps short inputs from paying for a large allocation they can never use, while
        // long ones still get the default. `reserve` is a capacity hint only.
        let mut queue: Vec<State> = Vec::with_capacity(
            self.beam_width
                .unwrap_or(128)
                .min(16usize.saturating_add(text_len as usize)),
        );

        // Visited set for state deduplication, reused (reset) per start window. Insertions and
        // deletions can reach the same automaton position via exponentially many distinct paths;
        // without dedup this BFS explodes in time and memory on long haystacks. Two states that
        // agree on automaton position, matched span, and per-edit-type counts behave identically
        // in the future, so only the lowest-penalty one needs to be expanded.
        //
        // With `MAX_EDITS_FAST == 0` the search is a pure trie walk: only exact transitions fire,
        // and a trie node has exactly one path from the root, so every state in a window sits on a
        // distinct path and the table can never report a duplicate. Every use below is therefore
        // behind a `MAX_EDITS_FAST != 0` const guard, which compiles the exact search down to no
        // hashing and no per-window reset at all — it used to pay a hash insert for every start
        // position, which was pure overhead. (The table itself is still built, so a tiny allocation
        // remains; making it conditional on the const generic measured *slower* everywhere, the
        // extra branch on the state loop costing more than the one small allocation saves.)
        let mut visited = DedupTable::new(if MAX_EDITS_FAST == 0 {
            0
        } else {
            // Size for the states one window is expected to expand. The dead-end filter keeps
            // that well below the raw branching factor, and the table grows itself if a window
            // turns out to be wider, so this only has to avoid the first few rehashes.
            let cap = match MAX_EDITS_FAST {
                1 => 64,
                2 => 128,
                _ => 256,
            };
            (text_len as usize * 4).clamp(16, cap).next_power_of_two()
        });

        // Global penalty ceiling, used for the cheap push-time guards below: a state carrying more
        // penalty than this can never reach the threshold. The root reaches every pattern, so its
        // per-node coefficients give exactly the global bound (longest/heaviest pattern). See
        // `Node::prune_len` for the derivation.
        let root = &self.nodes[0];
        let max_penalties = root.prune_len - root.prune_len_over_weight * similarity_threshold;
        // Per-substitution similarity floor (0.0 = no floor); hoisted out of the hot loop.
        let min_symbol_similarity = self.min_symbol_similarity;
        // Fast-path edit ceiling: MAX_EDITS_FAST is a const generic so the compiler can
        // eliminate the `!= 255` checks and dead-code the `else` (within_limits) branches.
        // `255` disables the fast path; otherwise the hot loop checks `edits <= MAX_EDITS_FAST`
        // (or `<` for ahead-checks) instead of calling `within_limits_*`.
        let has_pattern_limits = self.has_pattern_limits;
        // The rest of the engine's immutable configuration, hoisted into locals. `self` is only
        // ever shared-borrowed here, but the BFS writes to `queue`/`visited` and the optimiser has
        // to assume those writes might alias the engine, so each access would otherwise be a
        // re-load from memory — once per expanded state, for a handful of fields the loop reads
        // repeatedly. Binding them up front keeps them in registers.
        let nodes = &self.nodes;
        let pen = &self.penalties;
        let similarity = self.similarity;

        // 2-gram window skip for 1-edit search: precompute bitmaps of root edge chars
        // (first chars) and root children's edge chars (second chars). A window can only
        // yield a match if text[start] is a first or second char (exact match or deletion),
        // or text[start+1] is a second char (substitution dead-end filter passes). This
        // skips ~70% of windows for typical inputs, saving the visited-check + edge-scan
        // overhead for non-matching windows. Only applies when: exactly 1 edit, no
        // multi-char mappings, root has no output (no empty patterns), and no root child
        // has an output (no 1-char patterns).
        let window_skip: Option<(u128, u128)> =
            if WINDOW_SKIP && !MAPPINGS && root.output.is_empty() {
                let mut first = root.single_char_edge_bits();
                let mut second = 0u128;
                let mut child_output = false;
                for edge in &root.edges {
                    let child = &nodes[edge.next() as usize];
                    let child_bits = child.single_char_edge_bits();
                    second |= child_bits;
                    first |= child_bits;
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
        for start in 0..text_chars.len() {
            // 2-gram window skip: cheaply reject windows that cannot produce a match.
            if let Some((first_bits, second_bits)) = window_skip {
                let ch = text_chars[start];
                let ch_idx = ch as u32;
                if ch_idx < 128 && (first_bits >> ch_idx) & 1 == 0 {
                    // text[start] is not a first or second char.
                    // Check if text[start+1] is a second char (substitution dead-end filter).
                    let next_idx = start + 1;
                    if next_idx >= text_len as usize {
                        continue; // no next char — no match possible
                    }
                    let next_ch = text_chars[next_idx];
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
            if MAX_EDITS_FAST != 0 {
                visited.next_window();
            }
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
                        // Keep only the `bw` lowest-penalty states. We don't need them sorted
                        // (the dedup and best-map logic is order-independent for the final result),
                        // so use a partial selection (O(n)) instead of a full sort (O(n log n)).
                        queue[q_idx..].select_nth_unstable_by(bw - 1, |a, b| {
                            a.penalties.total_cmp(&b.penalties)
                        });
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
                //
                // Skipped entirely when no edit is permitted: the walk is then a pure trie
                // traversal, whose states all have distinct nodes, so the map could never match.
                if MAX_EDITS_FAST != 0
                    && visited
                        .probe_and_update(
                            VisitedKey {
                                node,
                                j,
                                matched_start,
                                packed_counts,
                            },
                            penalties,
                        )
                        .is_some_and(|seen| seen <= penalties)
                {
                    continue;
                }

                let node_ref = &nodes[node as usize];

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
                    // The matched span (and hence its byte offsets and text slice) is a property of
                    // the state, not of the individual pattern ending here, so compute it once for
                    // the whole `output` list instead of per pattern.
                    let start_byte = if (matched_start as usize) < text_chars.len() {
                        graphemes.gs_byte_offset(matched_start as usize)
                    } else {
                        0
                    };
                    let end_byte = if (matched_end as usize) < text_chars.len() {
                        graphemes.gs_byte_offset(matched_end as usize)
                    } else {
                        haystack.len()
                    };
                    let text = &haystack[start_byte..end_byte];
                    for &pattern_index in output {
                        let pattern_index = pattern_index as usize;
                        if MAX_EDITS_FAST != 255 {
                            if edits > MAX_EDITS_FAST {
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
                                        text,
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
                                text,
                            });
                    }
                }

                //
                // 1) Same or similar symbol — only within the text
                //
                let is_last_edit = MAX_EDITS_FAST != 255 && edits + 1 >= MAX_EDITS_FAST;
                // Compute current_ch once and reuse in both the exact-match section (inside
                // `if j < text_len`) and the deletion section (outside it), avoiding a
                // redundant `gs_first_char(j)` call per state.
                let current_ch = if j < text_len {
                    text_chars[j as usize]
                } else {
                    '\0'
                };
                if j < text_len {
                    // For dead-end filtering: if at the last edit level, check
                    // whether text[j+1] can match any child's outgoing edge.
                    // Only compute when edits are still available (i.e., at the
                    // last edit level where dead-end filtering is applicable);
                    // for non-root states with exhausted edit budget, the
                    // substitution/insertion blocks are skipped, so this is dead.
                    let next_ch_opt = if is_last_edit
                        && (MAX_EDITS_FAST == 255 || edits < MAX_EDITS_FAST)
                        && j + 1 < text_len
                    {
                        Some(text_chars[(j + 1) as usize])
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
                    // When MAPPINGS is false, all edges have grapheme_len == 1, so we can skip
                    // the grapheme_len check entirely for a tighter inner loop.
                    let exact_next = if MAPPINGS {
                        graphemes.gs_find_transition(node_ref, j as usize, current_ch)
                    } else {
                        node_ref.find_transition_char_no_mappings(current_ch)
                    };
                    if let Some(next_node) = exact_next {
                        trace!(
                            "  match   {:>8} ─ok→ node={}  sim=1.00",
                            graphemes.gs_text(j as usize),
                            next_node
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
                    let subst_ok = if MAX_EDITS_FAST == 255 {
                        self.within_limits_subst(
                            node_limits,
                            edits,
                            (packed_counts >> 16) as NumEdits,
                        )
                    } else {
                        edits < MAX_EDITS_FAST
                    };
                    if subst_ok {
                        // `current_ch` was already computed above from `gs_first_char(j)`.
                        for edge in edges {
                            let next_node = edge.next();
                            // Skip the exact transition (already enqueued above). Its target is
                            // reached with zero penalty and no extra edit, so any edge leading to
                            // the same node — possible after minimisation merges siblings — is
                            // strictly dominated by it and needs no substitution branch.
                            if Some(next_node) == exact_next {
                                continue;
                            }
                            // substitution
                            let sim = similarity_of(similarity, edge.first_char, current_ch);
                            // Weakest-link floor: reject a too-dissimilar character outright.
                            if sim < min_symbol_similarity {
                                continue;
                            }
                            let penalty = pen.substitution * (1.0 - sim);

                            // Skip substitutions that would push the state past the global ceiling.
                            if penalty > remaining {
                                continue;
                            }

                            // Dead-end filter: at the last edit level, the child state can
                            // only do exact match and output check. If the child has no
                            // output and no edge matching text[j+1], skip the push.
                            if is_last_edit {
                                let child = &nodes[next_node as usize];
                                if child.output.is_empty()
                                    && next_ch_opt
                                        .is_none_or(|ch| !child.has_matching_edge_char(ch))
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
                            notes.push(format!("sub {:?} -> {current_ch:?} (sim={sim:.2}, pen={penalty:.2}) (subst->{}, edits->{})", edge.first_char, ((packed_counts >> 16) & 0xFF) + 1, edits + 1));

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
                                let hay_matches =
                                    mt.haystack.iter().enumerate().all(|(k, g)| {
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
                    // Fast-path edit-limit check before the transition lookups: for 1-edit
                    // search, most non-root states have edits >= MAX_EDITS_FAST, so the
                    // two gs_find_transition calls below would be wasted work. The const
                    // generic lets the compiler dead-code the outer guard for the slow
                    // path (MAX_EDITS_FAST == 255) and eliminate the inner guard for the
                    // fast path.
                    if j + 1 < text_len
                        && pen.swap <= remaining
                        && (MAX_EDITS_FAST == 255 || edits < MAX_EDITS_FAST)
                    {
                        // Reuse next_ch_opt when available (1-edit: always Some here);
                        // fall back to gs_first_char for multi-edit where is_last_edit is false.
                        let next_ch = match next_ch_opt {
                            Some(ch) => ch,
                            None => text_chars[(j + 1) as usize],
                        };
                        if let Some(node2) = if MAPPINGS {
                            graphemes
                                .gs_find_transition(node_ref, (j + 1) as usize, next_ch)
                                .and_then(|x| {
                                    graphemes.gs_find_transition(
                                        &self.nodes[x as usize],
                                        j as usize,
                                        current_ch,
                                    )
                                })
                        } else {
                            node_ref
                                .find_transition_char_no_mappings(next_ch)
                                .and_then(|x| {
                                    nodes[x as usize].find_transition_char_no_mappings(current_ch)
                                })
                        } && (MAX_EDITS_FAST != 255
                            || self.within_limits_swap_ahead(
                                self.get_node_limits(node2),
                                edits,
                                (packed_counts >> 24) as NumEdits,
                            ))
                        {
                            #[cfg(debug_assertions)]
                            let mut notes = notes.clone();
                            #[cfg(debug_assertions)]
                            notes.push(format!(
                                "swap a:{current_ch:?} b:{next_ch:?} (swaps->{}, edits->{})",
                                ((packed_counts >> 24) & 0xFF) + 1,
                                edits + 1
                            ));
                            queue.push(State {
                                node: node2,
                                j: j + 2,
                                matched_start,
                                matched_end: j + 2,
                                penalties: penalties + pen.swap,
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
                        && pen.insertion <= remaining
                        && if MAX_EDITS_FAST == 255 {
                            self.within_limits_insertion_ahead(
                                node_limits,
                                edits,
                                (packed_counts & 0xFF) as NumEdits,
                            )
                        } else {
                            edits < MAX_EDITS_FAST
                        }
                        && !(is_last_edit
                            && output.is_empty()
                            && next_ch_opt.is_none_or(|ch| !node_ref.has_matching_edge_char(ch)))
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
                            penalties: penalties + pen.insertion,
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
                if pen.deletion <= remaining
                    && if MAX_EDITS_FAST == 255 {
                        self.within_limits_deletion_ahead(
                            node_limits,
                            edits,
                            ((packed_counts >> 8) & 0xFF) as NumEdits,
                        )
                    } else {
                        edits < MAX_EDITS_FAST
                    }
                {
                    // At the last edit level the child state can only do exact match
                    // and output check. If the child has no output and no edge matching
                    // the current text char, it's a dead end — skip the push to avoid
                    // wasted pop+dedup+find_transition work.
                    let current_ch_opt = if is_last_edit && j < text_len {
                        Some(current_ch)
                    } else {
                        None
                    };
                    for edge in edges {
                        let next_node2 = edge.next();
                        if is_last_edit {
                            let child = &self.nodes[next_node2 as usize];
                            if child.output.is_empty()
                                && current_ch_opt.is_none_or(|ch| !child.has_matching_edge_char(ch))
                            {
                                continue;
                            }
                        }
                        trace!("  delete to node={next_node2} penalty={:.2}", pen.deletion);
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
                            penalties: penalties + pen.deletion,
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
}

#[cfg(test)]
mod dedup_table_tests {
    use super::{DedupTable, VisitedKey};
    use std::collections::HashMap;

    fn key(a: u32) -> VisitedKey {
        VisitedKey {
            node: a,
            j: a.wrapping_mul(7),
            matched_start: a.wrapping_mul(13),
            packed_counts: a.wrapping_mul(3),
        }
    }

    /// The table must behave exactly like the `HashMap<VisitedKey, f32>` it replaced: report a
    /// stored penalty when it is `<=` the incoming one, overwrite when it is higher.
    #[test]
    fn matches_hashmap_semantics() {
        // A tiny table forces the growth path.
        let mut table = DedupTable::new(8);
        let mut reference: HashMap<VisitedKey, f32> = HashMap::new();

        let mut state = 12345u64;
        let mut next = move || {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (state >> 33) as u32
        };

        for window in 0..200 {
            table.next_window();
            reference.clear();
            for step in 0..500 {
                let k = key(next() % 400);
                let penalty = (step % 17) as f32 * 0.25;

                let expected = match reference.get(&k) {
                    Some(&seen) if seen <= penalty => Some(seen),
                    // A stored higher penalty is overwritten: the new state is strictly better.
                    Some(_) | None => {
                        reference.insert(k, penalty);
                        None
                    }
                };
                assert_eq!(
                    table.probe_and_update(k, penalty),
                    expected,
                    "window {window} step {step}"
                );
                assert_eq!(reference.len(), table.len, "window {window} step {step}");
            }
        }
    }

    /// A stale slot from a previous window must read as empty, so the per-window reset cannot let
    /// an old key suppress a new state.
    #[test]
    fn reset_invalidates_previous_window() {
        let mut table = DedupTable::new(8);
        let k = key(1);
        assert_eq!(table.probe_and_update(k, 1.0), None);
        assert_eq!(table.probe_and_update(k, 1.0), Some(1.0));
        table.next_window();
        assert_eq!(table.probe_and_update(k, 1.0), None);
        assert_eq!(table.len, 1);
    }

    /// A better (lower-penalty) state for an already-expanded key must be admitted, not skipped.
    #[test]
    fn admits_strictly_better_state() {
        let mut table = DedupTable::new(8);
        let k = key(2);
        assert_eq!(table.probe_and_update(k, 5.0), None);
        assert_eq!(table.probe_and_update(k, 1.0), None);
        assert_eq!(table.probe_and_update(k, 1.0), Some(1.0));
        assert_eq!(table.probe_and_update(k, 9.0), Some(1.0));
    }

    /// The generation counter must keep working when it wraps.
    #[test]
    fn epoch_wrap_resets_slots() {
        let mut table = DedupTable::new(8);
        let k = key(3);
        table.probe_and_update(k, 1.0);
        table.epoch = u32::MAX;
        table.next_window();
        assert_eq!(table.epoch, 1);
        assert_eq!(table.probe_and_update(k, 1.0), None);
    }
}
