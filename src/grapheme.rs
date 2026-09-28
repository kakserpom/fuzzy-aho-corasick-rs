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
    /// The first index at or after `from` whose grapheme has an outgoing edge from `node`.
    ///
    /// Lets the exact scan skip a whole run of graphemes that provably cannot continue the current
    /// match, instead of visiting each one only to discover that. Only meaningful when the answer
    /// is exact; the Unicode storage returns `from` unconditionally, because grapheme boundaries
    /// there are not byte-aligned and so cannot be walked past blindly.
    fn gs_next_possible(&self, node: &Node, from: usize) -> usize;
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
    fn gs_next_possible(&self, _node: &Node, from: usize) -> usize {
        // No skipping: grapheme boundaries are not byte-aligned, so advancing to the next
        // candidate would mean re-segmenting the text.
        from
    }
}

/// Zero-allocation grapheme storage for all-ASCII haystacks: each byte is a grapheme, and
/// case-folding is computed on the fly via the static `ascii_byte_to_str` table.
pub(crate) struct AsciiGraphemes<'a> {
    bytes: &'a [u8],
    case_insensitive: bool,
}

impl<'a> AsciiGraphemes<'a> {
    pub(crate) fn new(haystack: &'a str, case_insensitive: bool) -> Self {
        Self {
            bytes: haystack.as_bytes(),
            case_insensitive,
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
    fn gs_next_possible(&self, node: &Node, from: usize) -> usize {
        // Each grapheme is one byte, and `edge_bits` lists exactly the folded ASCII bytes that
        // have an outgoing edge (a non-ASCII pattern grapheme starts at U+0080 or above and can
        // never match a byte of an all-ASCII haystack). So this is a plain byte scan over folded
        // bytes, and it is what turns the exact search from "look at every grapheme" into "look
        // at every candidate" -- the same first-occurrence prefilter `aho-corasick` and `regex`
        // apply with SIMD `memchr`. (Byte-at-a-time here; `memchr` would be the next step, at the
        // cost of a dependency.)
        let bits = node.edge_bits;
        let bytes = self.bytes;
        let ci = self.case_insensitive;
        let mut i = from;
        while i < bytes.len() {
            let b = bytes[i];
            let b = if ci { b.to_ascii_lowercase() } else { b };
            if (bits >> u32::from(b)) & 1 != 0 {
                break;
            }
            i += 1;
        }
        i
    }
}
