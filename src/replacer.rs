use crate::FuzzyAhoCorasick;

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
        self.engine.replace(
            text,
            |m| self.replacements.get(m.pattern_index).cloned(),
            threshold,
        )
    }

    #[must_use]
    pub fn engine(&self) -> &FuzzyAhoCorasick {
        &self.engine
    }
}
