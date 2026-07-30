//! Public search / replace / segmentation convenience wrappers over the core engine.
use crate::{FuzzyAhoCorasick, FuzzyMatch, FuzzyMatches, SearchError, Segment};
use std::borrow::Cow;

impl FuzzyAhoCorasick {
    /// Convenience wrapper over `search_unsorted` that applies the default sorting
    /// order to the matches (via `default_sort()`).
    ///
    /// # Parameters
    /// - `haystack`: the input text to search in.
    /// - `similarity_threshold`: minimum similarity threshold for candidates.
    ///
    /// # Returns
    /// `FuzzyMatches` with matches sorted according to the default ranking.
    ///
    /// # Errors
    /// Propagates [`SearchError`] when the haystack is too large to index — see
    /// [`search_unsorted`](Self::search_unsorted).
    #[inline]
    pub fn search<'a>(
        &'a self,
        haystack: &'a str,
        similarity_threshold: f32,
    ) -> Result<FuzzyMatches<'a>, SearchError> {
        let mut matches = self.search_unsorted(haystack, similarity_threshold)?;
        matches.default_sort();
        Ok(matches)
    }

    /// Convenience wrapper over `search_unsorted` that applies a greedy sort (via `greedy_sort()`),
    ///
    /// # Parameters
    /// - `haystack`: the input text to search in.
    /// - `similarity_threshold`: minimum similarity threshold for candidates.
    ///
    /// # Returns
    /// `FuzzyMatches` with matches sorted by the greedy heuristic.
    ///
    /// # Errors
    /// Propagates [`SearchError`] when the haystack is too large to index — see
    /// [`search_unsorted`](Self::search_unsorted).
    #[inline]
    pub fn search_greedy<'a>(
        &'a self,
        haystack: &'a str,
        similarity_threshold: f32,
    ) -> Result<FuzzyMatches<'a>, SearchError> {
        let mut matches = self.search_unsorted(haystack, similarity_threshold)?;
        matches.greedy_sort();
        Ok(matches)
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
    ///
    /// # Errors
    /// Propagates [`SearchError`] when the haystack is too large to index — see
    /// [`search_unsorted`](Self::search_unsorted).
    #[inline]
    pub fn search_coverage_weighted<'a>(
        &'a self,
        haystack: &'a str,
        similarity_threshold: f32,
    ) -> Result<FuzzyMatches<'a>, SearchError> {
        let mut matches = self.search_unsorted(haystack, similarity_threshold)?;
        matches.coverage_weighted_sort();
        Ok(matches)
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
    ///
    /// # Errors
    /// Propagates [`SearchError`] when the haystack is too large to index — see
    /// [`search_unsorted`](Self::search_unsorted).
    pub fn search_non_overlapping<'a>(
        &'a self,
        haystack: &'a str,
        similarity_threshold: f32,
    ) -> Result<FuzzyMatches<'a>, SearchError> {
        let mut matches = self.search(haystack, similarity_threshold)?;
        matches.non_overlapping();
        Ok(matches)
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
    ///
    /// # Errors
    /// Propagates [`SearchError`] when the haystack is too large to index — see
    /// [`search_unsorted`](Self::search_unsorted).
    pub fn search_non_overlapping_unique<'a>(
        &'a self,
        haystack: &'a str,
        similarity_threshold: f32,
    ) -> Result<FuzzyMatches<'a>, SearchError> {
        let mut matches = self.search(haystack, similarity_threshold)?;
        matches.non_overlapping_unique();
        Ok(matches)
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
    ///
    /// # Errors
    /// Propagates [`SearchError`] when the haystack is too large to index — see
    /// [`search_unsorted`](Self::search_unsorted).
    pub fn search_non_overlapping_unique_coverage_weighted<'a>(
        &'a self,
        haystack: &'a str,
        similarity_threshold: f32,
    ) -> Result<FuzzyMatches<'a>, SearchError> {
        let mut matches = self.search_coverage_weighted(haystack, similarity_threshold)?;
        matches.non_overlapping_unique();
        Ok(matches)
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
    /// # Errors
    /// Propagates [`SearchError`] when the haystack is too large to index — see
    /// [`search_unsorted`](Self::search_unsorted).
    ///
    /// # Example
    /// ```rust
    /// use fuzzy_aho_corasick::FuzzyAhoCorasickBuilder;
    /// let automaton = FuzzyAhoCorasickBuilder::new().build(["FOO", "BAR", "BAZ"]);
    /// let result = automaton.replace("FOO BAR BAZ", |m| {
    ///     (m.pattern.pattern == "BAR").then_some("###")
    /// }, 0.8).unwrap();
    /// assert_eq!(result, "FOO ### BAZ");
    /// ```
    pub fn replace<'a, F, S: Into<Cow<'a, str>>>(
        &'a self,
        text: &'a str,
        callback: F,
        threshold: f32,
    ) -> Result<String, SearchError>
    where
        F: Fn(&FuzzyMatch<'a>) -> Option<S>,
    {
        Ok(self
            .search_non_overlapping(text, threshold)?
            .replace(callback))
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
    /// # Errors
    /// Propagates [`SearchError`] when the haystack is too large to index — see
    /// [`search_unsorted`](Self::search_unsorted).
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
    /// let result = f.strip_prefix("LrEM ISuM Lorm ZZZ", 0.8).unwrap();
    /// assert_eq!(result, "ZZZ");
    /// ```
    pub fn strip_prefix<'a>(
        &'a self,
        haystack: &'a str,
        threshold: f32,
    ) -> Result<String, SearchError> {
        Ok(self
            .search_non_overlapping(haystack, threshold)?
            .strip_prefix())
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
    /// # Errors
    /// Propagates [`SearchError`] when the haystack is too large to index — see
    /// [`search_unsorted`](Self::search_unsorted).
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
    /// let result = f.strip_postfix("ZZZ LrEM ISuM", 0.8).unwrap();
    /// assert_eq!(result, "ZZZ");
    /// ```
    pub fn strip_postfix<'a>(
        &'a self,
        haystack: &'a str,
        threshold: f32,
    ) -> Result<String, SearchError> {
        Ok(self
            .search_non_overlapping(haystack, threshold)?
            .strip_postfix())
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
    /// An iterator over the parts of `haystack` between each fuzzy match.
    ///
    /// # Errors
    /// Propagates [`SearchError`] when the haystack is too large to index — see
    /// [`search_unsorted`](Self::search_unsorted).
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
    /// let parts: Vec<&str> = engine.split("xxFo0yyBAARzz", 0.8).unwrap().collect();
    /// assert_eq!(parts, vec!["xx", "yy", "zz"]);
    /// ```
    pub fn split<'a>(
        &'a self,
        haystack: &'a str,
        threshold: f32,
    ) -> Result<impl Iterator<Item = &'a str> + 'a, SearchError> {
        Ok(self.search_non_overlapping(haystack, threshold)?.split())
    }

    /// Returns an **iterator** that yields interleaving [`Segment::Matched`]
    /// [`Segment::Unmatched`] items for the given text.
    ///
    /// # Errors
    /// Propagates [`SearchError`] when the haystack is too large to index — see
    /// [`search_unsorted`](Self::search_unsorted).
    pub fn segment_iter<'a>(
        &'a self,
        haystack: &'a str,
        threshold: f32,
    ) -> Result<impl Iterator<Item = Segment<'a>>, SearchError> {
        Ok(self
            .search_non_overlapping(haystack, threshold)?
            .segment_iter())
    }

    /// Convenience wrapper around [`segment_iter`](Self::segment_iter).
    ///
    /// # Errors
    /// Propagates [`SearchError`] when the haystack is too large to index — see
    /// [`search_unsorted`](Self::search_unsorted).
    pub fn segment_text(&self, haystack: &str, threshold: f32) -> Result<String, SearchError> {
        Ok(self
            .search_non_overlapping(haystack, threshold)?
            .segment_text())
    }
}
