use crate::{FuzzyAhoCorasick, SearchError, SearchOptions};
use std::io::{self, Read, Write};

/// A turnkey fuzzy find-and-replace built from `(pattern → replacement)` pairs.
///
/// Pairs the automaton with a parallel list of replacement strings (one per pattern), so a fuzzy
/// match of pattern *i* is substituted with replacement *i*. Build one with
/// [`FuzzyAhoCorasickBuilder::build_replacer`](crate::FuzzyAhoCorasickBuilder::build_replacer).
pub struct FuzzyReplacer {
    pub(crate) engine: FuzzyAhoCorasick,
    pub(crate) replacements: Vec<String>,
}

impl FuzzyReplacer {
    /// Performs a **fuzzy** find‑and‑replace using a list of `(pattern →
    /// replacement)` pairs.  Replacements are applied left‑to‑right, the longest
    /// non‑overlapping match wins.
    ///
    /// # Errors
    /// Propagates [`SearchError`] when the haystack is too large to index — see
    /// [`FuzzyAhoCorasick::search`](crate::FuzzyAhoCorasick::search).
    pub fn replace(&self, text: &str, opts: &SearchOptions) -> Result<String, SearchError> {
        self.engine
            .replace(text, opts, |m| self.replacements.get(m.pattern_index))
    }

    /// Streaming counterpart of [`replace`](Self::replace): read from `reader`, write the
    /// transformed stream to `writer` in constant memory, substituting each pattern with its
    /// configured replacement. Returns the number of bytes written.
    ///
    /// See [`FuzzyAhoCorasick::replace_stream`] for the exact windowing semantics.
    ///
    /// # Errors
    /// Propagates any [`io::Error`] from `reader` or `writer`.
    pub fn replace_stream<R: Read, W: Write>(
        &self,
        reader: R,
        writer: W,
        threshold: f32,
    ) -> io::Result<u64> {
        self.engine.replace_stream(reader, writer, threshold, |m| {
            self.replacements.get(m.pattern_index)
        })
    }

    /// Borrow the underlying [`FuzzyAhoCorasick`], e.g. to run a plain search with the same
    /// configuration the replacer was built with.
    #[must_use]
    pub fn engine(&self) -> &FuzzyAhoCorasick {
        &self.engine
    }
}
