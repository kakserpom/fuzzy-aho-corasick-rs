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
