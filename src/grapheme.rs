//! Grapheme-storage abstraction: lets the BFS hot loop be monomorphized over an
//! allocation-free ASCII fast path and the full Unicode path.
use crate::Node;
use std::borrow::Cow;

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
#[inline]
fn ascii_byte_to_str(b: u8) -> &'static str {
    debug_assert!(b < 128);
    // SAFETY: all bytes 0–127 are valid one-byte UTF-8 sequences.
    unsafe { std::str::from_utf8_unchecked(&ASCII_BYTES[(b as usize)..=(b as usize)]) }
}

/// Abstraction over grapheme storage so the BFS hot loop can be monomorphized for both the
/// ASCII fast path (zero-allocation `&[u8]`) and the full Unicode path (`Vec<(usize, Cow<str>)>`).
/// The trait is sealed to the two internal implementors so the compiler can devirtualise every
/// call.
pub(crate) trait GraphemeStorage {
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
    /// multi-byte graphemes need the full `&str` `HashMap` lookup path.
    fn gs_find_transition(&self, node: &Node, idx: usize, ch: char) -> Option<u32>;
    /// The first index at or after `from` whose grapheme can begin a match, or [`gs_len`](Self::gs_len).
    ///
    /// Lets the exact scan skip a whole run of graphemes that provably cannot start a match, instead
    /// of visiting each one only to discover that. Returns `from` unconditionally for storage that
    /// cannot answer it — grapheme boundaries are not byte-aligned there, so advancing to the next
    /// candidate would mean re-segmenting the text.
    fn gs_skip_unstartable(&self, from: usize) -> usize;
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
    #[inline]
    fn gs_skip_unstartable(&self, from: usize) -> usize {
        // No skipping: grapheme boundaries are not byte-aligned, so advancing to the next
        // candidate would mean re-segmenting the text.
        from
    }
}

/// The set of bytes that can begin a match, precomputed from a node's `edge_bits` for the exact
/// scan's skip.
///
/// `memchr` matches at most three needles at a time, so a small set is split into groups of three
/// and each group is searched separately, keeping the earliest hit. Every search is bounded by the
/// best hit so far, so later groups only look at the not-yet-ruled-out prefix and the whole thing
/// costs a couple of SIMD passes over the haystack.
///
/// Past [`EdgeSkip::MAX_BYTES`] the set is no longer sparse enough for that to pay: with ~26
/// candidate letters a byte hits a group every few positions, and the per-call overhead of several
/// `memchr` invocations overtakes a straight scalar scan. The bitmap is kept for that fallback.
#[derive(Clone, Copy)]
pub(crate) struct EdgeSkip {
    /// Candidate bytes, ascending. Only the first [`EdgeSkip::count`] are meaningful.
    bytes: [u8; EdgeSkip::MAX_BYTES],
    count: usize,
    /// All candidate bytes, for the scalar fallback. Non-ASCII bytes are never candidates: a
    /// multi-byte pattern grapheme starts at U+0080 or above and cannot match an ASCII haystack.
    bits: u128,
}

impl EdgeSkip {
    /// Enough for three `memchr3` groups, i.e. nine distinct starting bytes.
    const MAX_BYTES: usize = 9;

    /// Bytes to rule out with a plain scalar test before switching to SIMD.
    ///
    /// Sized to the crossover: a `memchr3` call costs a fixed ~30-50 cycles of setup, the scalar
    /// loop ~2 cycles/byte, so below roughly 30 bytes of skip the scalar loop wins outright. Set
    /// too low, every skip pays SIMD setup it did not need (measured +35% on a corpus whose match
    /// bytes are `h`/`w`/`r`/`s`, i.e. an average skip of about four bytes).
    const PROBE: usize = 32;

    /// Build from a node's ASCII edge bitmap. Done once per automaton, not per search.
    pub(crate) fn new(bits: u128) -> Self {
        let mut bytes = [0u8; Self::MAX_BYTES];
        let mut count = 0;
        for b in 0..128u32 {
            if (bits >> b) & 1 != 0 {
                if count == Self::MAX_BYTES {
                    // Too many candidates for the SIMD path; the scalar scan handles it.
                    return Self {
                        bytes,
                        count: 0,
                        bits,
                    };
                }
                bytes[count] = b as u8;
                count += 1;
            }
        }
        Self { bytes, count, bits }
    }

    /// Earliest index at or after `from` whose byte is a candidate, or `hay.len()`.
    #[inline]
    fn next(&self, hay: &[u8], from: usize, case_insensitive: bool) -> usize {
        // `memchr` matches raw bytes and cannot case-fold, but a case-insensitive automaton's
        // candidates are stored folded -- so scanning a `HELLO` haystack for the needle `h` would
        // never find the `H`, and the match would be skipped past. Fall back to folding
        // byte-at-a-time there, rather than doubling the candidate set with uppercase variants,
        // which pushes a typical set past the point where grouping still pays.
        if self.count == 0 || case_insensitive {
            return self.next_scalar(hay, from, case_insensitive);
        }
        // Probe the first few bytes by hand before reaching for SIMD. `memchr` costs a fixed
        // setup per call, and with a dense candidate set -- `h`/`w`/`r`/`s` in ordinary prose, say
        // -- the average skip is only a handful of bytes, so jumping straight to SIMD loses badly
        // (measured +35% on a 4-pattern corpus). A scalar probe pays only for the skips that turn
        // out to be short, and hands the long ones -- the sparse-candidate case SIMD is actually
        // for -- to `memchr`.
        let probe_end = from.saturating_add(Self::PROBE).min(hay.len());
        let mut i = from;
        while i < probe_end {
            if self.is_candidate(hay[i], case_insensitive) {
                return i;
            }
            i += 1;
        }
        if i == hay.len() {
            return i;
        }
        let from = i;
        let mut best = hay.len();
        let mut group = 0;
        while group < self.count {
            let end = (group + 3).min(self.count);
            let window = &hay[from..best];
            let found = match end - group {
                1 => memchr::memchr(self.bytes[group], window),
                2 => memchr::memchr2(self.bytes[group], self.bytes[group + 1], window),
                _ => memchr::memchr3(
                    self.bytes[group],
                    self.bytes[group + 1],
                    self.bytes[group + 2],
                    window,
                ),
            };
            if let Some(offset) = found {
                let pos = from + offset;
                if pos < best {
                    best = pos;
                }
            }
            group += 3;
        }
        best
    }

    /// One byte at a time: the too-many-candidates fallback, and the only correct path for a
    /// case-insensitive engine.
    fn next_scalar(&self, hay: &[u8], from: usize, case_insensitive: bool) -> usize {
        let mut i = from;
        while i < hay.len() {
            if self.is_candidate(hay[i], case_insensitive) {
                break;
            }
            i += 1;
        }
        i
    }

    /// Whether `b` is one of the match-starting bytes.
    ///
    /// `bits` only ever has bits set below 128 -- a multi-byte grapheme cannot start a match against
    /// an ASCII candidate set -- so a byte at or above 128 is never a candidate. Testing that
    /// *first* is not an optimisation, it is the whole point: shifting the 128-bit bitmap by a raw
    /// byte panics on overflow in debug builds, and in release the shift amount is masked to 7 bits
    /// so a UTF-8 lead or continuation byte reads bit `b & 127` and can be mistaken for a candidate.
    /// That costs a wasted window per false positive rather than a wrong answer, which is why it
    /// survived every release-mode test.
    #[inline]
    fn is_candidate(&self, b: u8, case_insensitive: bool) -> bool {
        let b = if case_insensitive {
            b.to_ascii_lowercase()
        } else {
            b
        };
        b < 128 && (self.bits >> u32::from(b)) & 1 != 0
    }
}

/// Grapheme storage for all-ASCII haystacks: each byte is a grapheme, and case-folding is computed
/// on the fly via the static `ascii_byte_to_str` table.
pub(crate) struct AsciiGraphemes<'a> {
    bytes: &'a [u8],
    case_insensitive: bool,
    /// Which bytes can start a match, for [`GraphemeStorage::gs_skip_unstartable`].
    skip: EdgeSkip,
}

impl<'a> AsciiGraphemes<'a> {
    /// `skip` is the automaton's precomputed set of match-starting bytes (see [`EdgeSkip::new`]),
    /// so building the storage costs one copy regardless of haystack or pattern count.
    pub(crate) fn new(haystack: &'a str, case_insensitive: bool, skip: EdgeSkip) -> Self {
        Self {
            bytes: haystack.as_bytes(),
            case_insensitive,
            skip,
        }
    }

    /// The (case-folded) ASCII byte of the `idx`-th grapheme.
    ///
    /// Every accessor folds through this one place, so no accessor can disagree with another about
    /// what a grapheme looks like. That matters for the exact scan's skip, which matches folded
    /// bytes against a trie of folded patterns.
    #[inline]
    fn folded_byte(&self, idx: usize) -> u32 {
        let b = self.bytes[idx];
        if self.case_insensitive {
            u32::from(b.to_ascii_lowercase())
        } else {
            u32::from(b)
        }
    }
}

impl GraphemeStorage for AsciiGraphemes<'_> {
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
        ascii_byte_to_str(self.folded_byte(idx) as u8)
    }
    #[inline]
    fn gs_first_char(&self, idx: usize) -> char {
        char::from(self.folded_byte(idx) as u8)
    }
    #[inline]
    fn gs_find_transition(&self, node: &Node, _idx: usize, ch: char) -> Option<u32> {
        // All graphemes are single-byte ASCII, so skip the &str creation and
        // byte-length check in `find_transition` and go straight to the char scan.
        node.find_transition_char(ch)
    }
    #[inline]
    fn gs_skip_unstartable(&self, from: usize) -> usize {
        self.skip.next(self.bytes, from, self.case_insensitive)
    }
}

#[cfg(test)]
mod tests {
    use super::EdgeSkip;

    /// The reference implementation: the first position whose folded byte is a candidate.
    ///
    /// A byte at or above 128 is never a candidate, because the bitmap only covers ASCII. The
    /// comparison is written as a filter *before* the shift, for the same reason the real code does:
    /// a raw `bits >> b` with `b >= 128` overflows the 128-bit shift in a debug build, and is masked
    /// to `b & 127` in a release one. This reference had that bug too, and hid it from the test.
    fn naive(bits: u128, hay: &[u8], from: usize, case_insensitive: bool) -> usize {
        (from..hay.len())
            .find(|&i| {
                let b = if case_insensitive {
                    hay[i].to_ascii_lowercase()
                } else {
                    hay[i]
                };
                b < 128 && (bits >> u32::from(b)) & 1 != 0
            })
            .unwrap_or(hay.len())
    }

    fn bits_of(set: &[u8]) -> u128 {
        set.iter().fold(
            0u128,
            |acc, &b| {
                if b < 128 { acc | (1u128 << b) } else { acc }
            },
        )
    }

    /// `EdgeSkip` has three distinct paths -- the grouped `memchr` scan, the too-many-candidates
    /// scalar fallback, and the case-insensitive scalar path -- and each must agree with a plain
    /// scan at *every* starting offset. The search is only correct if the skip never steps over a
    /// candidate, so this is checked exhaustively rather than on a few hand-picked inputs.
    #[test]
    fn skip_agrees_with_naive_scan_on_every_path() {
        // One byte (memchr), three (memchr3), four (memchr3 + memchr: two groups), nine (three
        // groups, at the MAX_BYTES limit), ten (past it -> scalar fallback), and a set of
        // non-letters including the ASCII boundary bytes.
        let sets: &[&[u8]] = &[
            b"z",
            b"elo",
            b"howr",
            b"abcdefghi",
            b"abcdefghij",
            &[0x00, 0x7f, 0x21, 0x40, 0x5f, 0x60, 0x7e],
        ];
        // Haystacks covering: empty, no candidate at all, a candidate at the front, a candidate
        // only near the end, a long body so both the PROBE window and the memchr path are entered,
        // and bytes on either side of the ASCII/non-ASCII boundary.
        let hays: &[&[u8]] = &[
            b"",
            b"the quick brown fox",
            b"zebra",
            b"qqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqzqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqq",
            b"qqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqq",
            b"the quick brown fox jumps over the lazy dog while xylophones queue",
            b"\x00\x7f\xff\x80 z Z\x7e",
        ];
        for set in sets {
            let bits = bits_of(set);
            let skip = EdgeSkip::new(bits);
            for hay in hays {
                for &ci in &[false, true] {
                    for from in 0..=hay.len() {
                        assert_eq!(
                            skip.next(hay, from, ci),
                            naive(bits, hay, from, ci),
                            "set={set:?} hay={hay:?} from={from} case_insensitive={ci}"
                        );
                    }
                }
            }
        }
    }

    /// An empty candidate set can never match, so the skip must always run to the end.
    #[test]
    fn empty_candidate_set_skips_everything() {
        let skip = EdgeSkip::new(0);
        let hay = b"the quick brown fox";
        for from in 0..=hay.len() {
            assert_eq!(skip.next(hay, from, false), hay.len());
        }
    }

    /// More than `MAX_BYTES` candidates must fall back to the scalar scan rather than dropping the
    /// overflow bytes, which would silently skip past real matches.
    #[test]
    fn oversized_candidate_set_keeps_every_byte() {
        let set: Vec<u8> = (b'a'..=b'z').collect();
        let bits = bits_of(&set);
        let skip = EdgeSkip::new(bits);
        assert_eq!(
            skip.count, 0,
            "oversized set must not take the grouped path"
        );
        let hay = b"....a....z....";
        for from in 0..=hay.len() {
            assert_eq!(skip.next(hay, from, false), naive(bits, hay, from, false));
        }
    }
}

#[cfg(test)]
mod boundary_tests {
    use unicode_segmentation::UnicodeSegmentation;

    /// The streaming commit boundary is computed *backwards* from the end of a window:
    /// `text.grapheme_indices(true).rev().nth(overlap - 1)` is the byte offset where the retained
    /// overlap begins, and everything before it is committed.
    ///
    /// That leans entirely on `GraphemeIndices`' `DoubleEndedIterator` impl agreeing with forward
    /// iteration, for every shape of input. If it ever disagrees, the window commits the wrong
    /// number of bytes and a match spanning a boundary is silently lost -- with no local symptom,
    /// because each window in isolation is searched perfectly. unicode-segmentation 1.13 backs
    /// `rev()` on this type, and the streaming tests only ever feed it ASCII, so nothing else in the
    /// suite covers it.
    #[test]
    fn reverse_grapheme_iteration_agrees_with_forward() {
        // Shapes that make a backwards scan non-trivial: combining marks, Hangul Jamo, ZWJ emoji,
        // regional indicators, CRLF, and text that is one cluster long.
        let samples: &[&str] = &[
            "",
            "a",
            "abc",
            "ab\u{301}c",
            "\u{e9}",
            "e\u{301}",
            "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467} tail",
            "\u{1F1EC}\u{1F1E7}\u{1F1EC}\u{1F1E7} tail",
            "\u{1112}\u{1161}\u{11AB}\u{1100}\u{1161} tail",
            "line one\r\nline two",
            "caf\u{e9} \u{FB01} \u{DF} \u{AC00} end",
            "\u{1F469}\u{1F3FD}",
            "x\u{0301}\u{0302}\u{0303}",
        ];

        for text in samples {
            let count = text.graphemes(true).count();
            // The empty string has no grapheme to anchor either scan, and both return `None`, so it
            // is checked once here rather than through a subtraction that would underflow.
            if count == 0 {
                assert_eq!(
                    text.grapheme_indices(true).next_back(),
                    None,
                    "an empty string should yield no backwards grapheme"
                );
                continue;
            }
            // Every possible retained-overlap size, from 1 grapheme up to the whole string.
            for overlap in 1..=count {
                let backwards = text
                    .grapheme_indices(true)
                    .rev()
                    .nth(overlap - 1)
                    .map(|(off, _)| off);
                // The same thing computed forwards: skip the last `overlap` graphemes and take the
                // offset of the one before them.
                let forwards = text
                    .grapheme_indices(true)
                    .nth(count - overlap)
                    .map(|(off, _)| off);
                assert_eq!(
                    backwards, forwards,
                    "text {text:?}: backwards scan for overlap {overlap} of {count} graphemes \
                     disagrees with the forward scan"
                );
            }
        }
    }
}
