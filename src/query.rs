//! Public search / replace / segmentation entry points, driven by [`SearchOptions`].
use crate::{
    FuzzyAhoCorasick, FuzzyMatch, FuzzyMatches, Order, Overlap, SearchError, SearchOptions, Segment,
};
use std::borrow::Cow;

impl FuzzyAhoCorasick {
    /// Search `haystack`, returning the matches at or above `opts.threshold`, ranked and
    /// overlap-resolved per [`opts`](SearchOptions). With [`SearchOptions::default`] this is the raw
    /// best-per-span result in no particular order (fastest); set the [`order`](SearchOptions::order)
    /// and [`overlap`](SearchOptions::overlap) to sort and/or resolve overlaps.
    ///
    /// # Errors
    /// Returns [`SearchError::HaystackTooLarge`] if `haystack` has more than `u32::MAX` grapheme
    /// clusters (roughly a 4 GiB ASCII input): positions are indexed with `u32`, so larger inputs
    /// must use the [streaming API](crate::StreamMatches).
    ///
    /// # Example
    /// ```
    /// use fuzzy_aho_corasick::{FuzzyAhoCorasickBuilder, FuzzyLimits, SearchOptions};
    /// let engine = FuzzyAhoCorasickBuilder::new()
    ///     .fuzzy(FuzzyLimits::new().edits(1))
    ///     .case_insensitive(true)
    ///     .build(["hello", "world"]);
    /// let opts = SearchOptions::new().threshold(0.8).non_overlapping();
    /// let matches = engine.search("helllo wolrd", &opts).unwrap();
    /// let found: Vec<&str> = matches.iter().map(|m| m.pattern.as_str()).collect();
    /// assert!(found.contains(&"hello") && found.contains(&"world"));
    /// ```
    pub fn search<'a>(
        &'a self,
        haystack: &'a str,
        opts: &SearchOptions,
    ) -> Result<FuzzyMatches<'a>, SearchError> {
        let mut matches = self.search_raw(haystack, opts.threshold)?;
        matches.apply(opts.order, opts.overlap);
        Ok(matches)
    }

    /// Search for the segmentation-style helpers (`replace`, `strip_*`, `split`, `segment_*`), which
    /// require a non-overlapping, deterministically-ordered match set. Honors `opts.threshold` and
    /// `opts.order` (falling back to [`Order::Default`] when it's left [`Order::Unsorted`], so
    /// results are deterministic) and always resolves overlaps (`Keep` is upgraded to
    /// [`Overlap::NonOverlapping`]; an explicit unique mode is preserved).
    #[allow(clippy::trivially_copy_pass_by_ref)] // uniform with the `&SearchOptions` public API
    fn segmented<'a>(
        &'a self,
        haystack: &'a str,
        opts: &SearchOptions,
    ) -> Result<FuzzyMatches<'a>, SearchError> {
        let order = if opts.order == Order::Unsorted {
            Order::Default
        } else {
            opts.order
        };
        let overlap = if opts.overlap == Overlap::Keep {
            Overlap::NonOverlapping
        } else {
            opts.overlap
        };
        let mut matches = self.search_raw(haystack, opts.threshold)?;
        matches.apply(order, overlap);
        Ok(matches)
    }

    /// Fuzzy find-and-replace: for each non-overlapping match (per [`opts`](SearchOptions)),
    /// `callback` returns `Some(replacement)` to substitute the span or `None` to keep it. Overlaps
    /// are always resolved (see [`segmented`](Self::segmented)).
    ///
    /// # Errors
    /// Propagates [`SearchError`] when the haystack is too large to index — see
    /// [`search`](Self::search).
    ///
    /// # Example
    /// ```
    /// use fuzzy_aho_corasick::{FuzzyAhoCorasickBuilder, SearchOptions};
    /// let automaton = FuzzyAhoCorasickBuilder::new().build(["FOO", "BAR", "BAZ"]);
    /// let result = automaton
    ///     .replace("FOO BAR BAZ", &SearchOptions::new().threshold(0.8), |m| {
    ///         (m.pattern.pattern == "BAR").then_some("###")
    ///     })
    ///     .unwrap();
    /// assert_eq!(result, "FOO ### BAZ");
    /// ```
    pub fn replace<'a, F, S: Into<Cow<'a, str>>>(
        &'a self,
        text: &'a str,
        opts: &SearchOptions,
        callback: F,
    ) -> Result<String, SearchError>
    where
        F: Fn(&FuzzyMatch<'a>) -> Option<S>,
    {
        Ok(self.segmented(text, opts)?.replace(callback))
    }

    /// Strip a leading fuzzy-matched prefix from `haystack` (matches resolved per
    /// [`opts`](SearchOptions)) and return the remainder, with leading whitespace trimmed.
    ///
    /// # Errors
    /// Propagates [`SearchError`] when the haystack is too large to index — see
    /// [`search`](Self::search).
    ///
    /// # Example
    /// ```
    /// use fuzzy_aho_corasick::{FuzzyAhoCorasickBuilder, FuzzyLimits, SearchOptions};
    /// let f = FuzzyAhoCorasickBuilder::new()
    ///     .fuzzy(FuzzyLimits::new().edits(1))
    ///     .case_insensitive(true)
    ///     .build(["LOREM", "IPSUM"]);
    /// let result = f.strip_prefix("LrEM ISuM Lorm ZZZ", &SearchOptions::new().threshold(0.8)).unwrap();
    /// assert_eq!(result, "ZZZ");
    /// ```
    pub fn strip_prefix<'a>(
        &'a self,
        haystack: &'a str,
        opts: &SearchOptions,
    ) -> Result<String, SearchError> {
        Ok(self.segmented(haystack, opts)?.strip_prefix())
    }

    /// Strip a trailing fuzzy-matched suffix from `haystack` (matches resolved per
    /// [`opts`](SearchOptions)) and return the leading portion, with trailing whitespace trimmed.
    ///
    /// # Errors
    /// Propagates [`SearchError`] when the haystack is too large to index — see
    /// [`search`](Self::search).
    ///
    /// # Example
    /// ```
    /// use fuzzy_aho_corasick::{FuzzyAhoCorasickBuilder, FuzzyLimits, SearchOptions};
    /// let f = FuzzyAhoCorasickBuilder::new()
    ///     .fuzzy(FuzzyLimits::new().edits(1))
    ///     .case_insensitive(true)
    ///     .build(["LOREM", "IPSUM"]);
    /// let result = f.strip_suffix("ZZZ LrEM ISuM", &SearchOptions::new().threshold(0.8)).unwrap();
    /// assert_eq!(result, "ZZZ");
    /// ```
    pub fn strip_suffix<'a>(
        &'a self,
        haystack: &'a str,
        opts: &SearchOptions,
    ) -> Result<String, SearchError> {
        Ok(self.segmented(haystack, opts)?.strip_suffix())
    }

    /// Split `haystack` on each fuzzy match (resolved per [`opts`](SearchOptions)), yielding the
    /// unmatched substrings between matches.
    ///
    /// # Errors
    /// Propagates [`SearchError`] when the haystack is too large to index — see
    /// [`search`](Self::search).
    ///
    /// # Example
    /// ```
    /// use fuzzy_aho_corasick::{FuzzyAhoCorasickBuilder, FuzzyLimits, SearchOptions};
    /// let engine = FuzzyAhoCorasickBuilder::new()
    ///     .fuzzy(FuzzyLimits::new().edits(1))
    ///     .case_insensitive(true)
    ///     .build(["FOO", "BAR"]);
    /// let parts: Vec<&str> = engine.split("xxFo0yyBAARzz", &SearchOptions::new().threshold(0.8))
    ///     .unwrap()
    ///     .collect();
    /// assert_eq!(parts, vec!["xx", "yy", "zz"]);
    /// ```
    pub fn split<'a>(
        &'a self,
        haystack: &'a str,
        opts: &SearchOptions,
    ) -> Result<impl Iterator<Item = &'a str> + 'a, SearchError> {
        Ok(self.segmented(haystack, opts)?.split())
    }

    /// Return an iterator of interleaving [`Segment::Matched`] / [`Segment::Unmatched`] items
    /// (matches resolved per [`opts`](SearchOptions)).
    ///
    /// # Errors
    /// Propagates [`SearchError`] when the haystack is too large to index — see
    /// [`search`](Self::search).
    pub fn segment_iter<'a>(
        &'a self,
        haystack: &'a str,
        opts: &SearchOptions,
    ) -> Result<impl Iterator<Item = Segment<'a>>, SearchError> {
        Ok(self.segmented(haystack, opts)?.segment_iter())
    }

    /// Convenience wrapper around [`segment_iter`](Self::segment_iter) that renders the segments
    /// back to a single normalized string.
    ///
    /// # Errors
    /// Propagates [`SearchError`] when the haystack is too large to index — see
    /// [`search`](Self::search).
    pub fn segment_text(
        &self,
        haystack: &str,
        opts: &SearchOptions,
    ) -> Result<String, SearchError> {
        Ok(self.segmented(haystack, opts)?.segment_text())
    }
}
