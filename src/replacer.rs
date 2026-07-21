use crate::FuzzyAhoCorasick;
use std::io::{self, Read, Write};

pub struct FuzzyReplacer {
    pub(crate) engine: FuzzyAhoCorasick,
    pub(crate) replacements: Vec<String>,
}

impl FuzzyReplacer {
    /// Performs a **fuzzy** find‑and‑replace using a list of `(pattern →
    /// replacement)` pairs.  Replacements are applied left‑to‑right, the longest
    /// non‑overlapping match wins.
    #[must_use]
    pub fn replace(&self, text: &str, threshold: f32) -> String {
        self.engine
            .replace(text, |m| self.replacements.get(m.pattern_index), threshold)
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
        self.engine.replace_stream(
            reader,
            writer,
            |m| self.replacements.get(m.pattern_index),
            threshold,
        )
    }

    #[must_use]
    pub fn engine(&self) -> &FuzzyAhoCorasick {
        &self.engine
    }
}
