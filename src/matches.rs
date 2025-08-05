use crate::{FuzzyMatch, FuzzyMatches, Segment, UniqueId};
use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet, VecDeque};

impl<'a> FuzzyMatches<'a> {
    /// Default ranking: prefers higher similarity, then longer pattern, then
    /// longer matched text, then earlier occurrence.
    #[inline]
    pub fn default_sort(&mut self) {
        self.inner.sort_by(|left, right| {
            right
                .similarity
                .total_cmp(&left.similarity)
                .then_with(|| right.pattern.len().cmp(&left.pattern.len()))
                .then_with(|| right.text.len().cmp(&left.text.len()))
                .then_with(|| left.start.cmp(&right.start))
        });
    }

    /// Greedy ranking: prefers longer pattern first, then higher similarity,
    /// then earlier position. Used when one wants to favor breadth of match over
    /// score tie-breaking.
    #[inline]
    pub fn greedy_sort(&mut self) {
        self.inner.sort_by(|left, right| {
            right
                .pattern
                .len()
                .cmp(&left.pattern.len())
                .then_with(|| right.similarity.total_cmp(&left.similarity))
                .then_with(|| left.start.cmp(&right.start))
        });
    }

    /// Retain a set of non-overlapping matches in place. Traverses in current
    /// order and keeps a match only if its span does not intersect any already
    /// accepted span. The kept matches are finally re-sorted by `start`.
    pub fn non_overlapping(&mut self) {
        let mut occupied_intervals: BTreeMap<usize, usize> = BTreeMap::new();
        self.inner.retain(|m| {
            if occupied_intervals
                .range(..=m.start)
                .next_back()
                .is_none_or(|(_, &end)| end <= m.start)
                && occupied_intervals
                    .range(m.start..)
                    .next()
                    .is_none_or(|(&start, _)| start >= m.end)
            {
                occupied_intervals.insert(m.start, m.end);
                #[cfg(test)]
                println!("ACCEPTING: \t{:?}", m);
                true
            } else {
                #[cfg(test)]
                println!("DISCARDING OVERLAPPING: {m:?}");
                false
            }
        });
        self.inner.sort_by_key(|m| m.start);
    }

    /// Like `non_overlapping`, but also enforces that each pattern (by its
    /// `custom_unique_id` if present, otherwise by index) is used at most once.
    pub fn non_overlapping_unique(&mut self) {
        let mut used_patterns = BTreeSet::new();
        let mut occupied_intervals: BTreeMap<usize, usize> = BTreeMap::new();
        self.inner.retain(|m| {
            let unique_id = if let Some(custom_unique_id) = m.pattern.custom_unique_id {
                UniqueId::Custom(custom_unique_id)
            } else {
                UniqueId::Automatic(m.pattern_index)
            };
            if !used_patterns.contains(&unique_id)
                && occupied_intervals
                    .range(..=m.start)
                    .next_back()
                    .is_none_or(|(_, &end)| end <= m.start)
                && occupied_intervals
                    .range(m.start..)
                    .next()
                    .is_none_or(|(&start, _)| start >= m.end)
            {
                used_patterns.insert(unique_id);
                occupied_intervals.insert(m.start, m.end);
                #[cfg(test)]
                println!("ACCEPTING: \t{:?}", m);
                true
            } else {
                #[cfg(test)]
                println!("DISCARDING OVERLAPPING: {m:?}");
                false
            }
        });
        self.inner.sort_by_key(|m| m.start);
    }

    /// Performs a **fuzzy** find-and-replace using the current match list.
    /// You may return either a borrowed `&str` or an owned `String` from your callback.
    ///
    /// # Parameters
    ///
    /// - `callback`: `Fn(&FuzzyMatch<'a>) -> Option<Cow<'a, str>>`.
    ///    - Return `Some(Cow::Borrowed("foo"))` to substitute with a `&'static` or haystack slice.
    ///    - Return `Some(Cow::Owned(my_string))` to substitute with a freshly-allocated `String`.
    ///    - Return `None` to keep the original matched text.
    ///
    /// # Returns
    ///
    /// A new `String` with each fuzzy match replaced according to your callback.
    #[must_use]
    pub fn replace<F, S>(&self, callback: F) -> String
    where
        F: Fn(&FuzzyMatch<'a>) -> Option<S>,
        S: Into<Cow<'a, str>>,
    {
        let mut result = String::new();
        let mut last = 0;

        for matched in &self.inner {
            if matched.start >= last {
                // append the slice between the end of the last match and the start of this one
                result.push_str(&self.haystack[last..matched.start]);
                last = matched.end;

                // callback may return either borrowed or owned string
                match callback(matched) {
                    Some(cow) => result.push_str(&cow.into()),
                    None => result.push_str(matched.text),
                }
            }
        }
        result.push_str(&self.haystack[last..]);
        result
    }

    /// Strip any leading fuzzy‐matched prefix from the sequence of segments,
    /// returning the concatenated remainder.
    ///
    /// # Behavior
    /// - Skips all initial `Segment::Matched` segments.
    /// - Skips any `Segment::Unmatched` segments containing only whitespace.
    /// - On the first `Unmatched` with non‐whitespace text:
    ///   - Trims its leading whitespace (`trim_start`) before appending.
    ///   - Disables skipping so that all subsequent segments are included.
    /// - Appends all remaining segments (both `Matched` and `Unmatched`) in full.
    ///
    /// # Returns
    /// A `String` made of the text from segments after removing the leading
    /// matched portion and trimming leading whitespace from the first kept segment.
    ///
    /// # Examples
    /// ```rust
    /// use fuzzy_aho_corasick::{FuzzyAhoCorasickBuilder, FuzzyLimits};
    ///
    /// let f = FuzzyAhoCorasickBuilder::new()
    ///     .fuzzy(FuzzyLimits::new().edits(1))
    ///     .case_insensitive(true)
    ///     .build(["LOREM", "IPSUM"]);
    ///
    /// let matches = f.search_non_overlapping("LrEM ISuM Lorm ZZZ", 0.8);
    /// assert_eq!(matches.strip_prefix(), "ZZZ");
    /// ```
    #[must_use]
    pub fn strip_prefix(self) -> String {
        let mut result = String::new();
        let mut skipping = true;

        for segment in self.segment_iter() {
            match segment {
                Segment::Matched(m) => {
                    if skipping {
                        continue;
                    }
                    result.push_str(m.text);
                }
                Segment::Unmatched(u) => {
                    if skipping {
                        if u.text.trim().is_empty() {
                            continue;
                        }
                        skipping = false;
                        result.push_str(u.text.trim_start());
                    } else {
                        result.push_str(u.text);
                    }
                }
            }
        }

        result
    }

    /// Strip any trailing fuzzy‐matched suffix from the sequence of segments,
    /// returning the concatenated leading portion.
    ///
    /// # Behavior
    /// - Buffers all segments, tracking the position of the last
    ///   non‐whitespace `Unmatched` segment.
    /// - Skips all trailing `Segment::Matched` segments.
    /// - Skips any trailing `Segment::Unmatched` segments consisting only of whitespace.
    /// - Builds the result from the first segment up to that cutoff:
    ///   - Trims trailing whitespace (`trim_end`) on the last kept `Unmatched`.
    ///   - Includes all other segments in full.
    ///
    /// # Returns
    /// A `String` made of the text from segments before removing the trailing
    /// matched portion and trimming trailing whitespace from the last kept segment.
    ///
    /// # Examples
    /// ```rust
    /// use fuzzy_aho_corasick::{FuzzyAhoCorasickBuilder, FuzzyLimits};
    ///
    /// let f = FuzzyAhoCorasickBuilder::new()
    ///     .fuzzy(FuzzyLimits::new().edits(1))
    ///     .case_insensitive(true)
    ///     .build(["LOREM", "IPSUM"]);
    ///
    /// let matches = f.search_non_overlapping("ZZZ LrEM ISuM Lorm", 0.8);
    /// assert_eq!(matches.strip_postfix(), "ZZZ");
    /// ```
    #[must_use]
    pub fn strip_postfix(self) -> String {
        let mut buf: VecDeque<Segment<'a>> = VecDeque::new();
        let mut keep = 0;

        for seg in self.segment_iter() {
            buf.push_back(seg);
            if let Some(Segment::Unmatched(u)) = buf.back() {
                if !u.text.trim().is_empty() {
                    keep = buf.len();
                }
            }
        }

        let mut result = String::new();
        for (i, seg) in buf.into_iter().take(keep).enumerate() {
            let is_last = i + 1 == keep;
            match seg {
                Segment::Matched(m) => {
                    result.push_str(m.text);
                }
                Segment::Unmatched(u) => {
                    if is_last {
                        result.push_str(u.text.trim_end());
                    } else {
                        result.push_str(u.text);
                    }
                }
            }
        }

        result
    }

    /// Splits the sequence of segments into a vector of unmatched substrings,
    /// using each fuzzy‐matched segment as a delimiter.
    ///
    /// # Behavior
    ///
    /// - Iterates through `segment_iter()`, which yields `Segment::Matched` and `Segment::Unmatched`.
    /// - On each `Segment::Matched`, pushes the current buffer into the result `Vec<String>` and resets it.
    /// - On each `Segment::Unmatched(u)`, appends `u.text` to the current buffer.
    /// - After processing all segments, pushes any remaining buffer (which may be empty).
    ///
    /// # Returns
    ///
    /// A `Vec<String>` containing all the unmatched pieces of the original text,
    /// in order, split at each fuzzy match.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use fuzzy_aho_corasick::{FuzzyAhoCorasickBuilder, FuzzyLimits};
    /// let engine = FuzzyAhoCorasickBuilder::new()
    ///     .fuzzy(FuzzyLimits::new().edits(1))
    ///     .case_insensitive(true)
    ///     .build(["FOO", "BAR"]);
    ///
    /// let parts: Vec<String> = engine
    ///     .search_non_overlapping("xxFoOyyBAARzz", 0.8)
    ///     .split()
    ///     .collect();
    ///
    /// assert_eq!(parts, vec![
    ///     "xx".to_string(),
    ///     "yy".to_string(),
    ///     "zz".to_string(),
    /// ]);
    /// ```
    #[must_use]
    pub fn split(self) -> impl Iterator<Item = String> + 'a {
        let mut segments = self.segment_iter();
        let mut buf = String::new();
        let mut done = false;
        std::iter::from_fn(move || {
            if done {
                return None;
            }
            while let Some(segment) = segments.next() {
                match segment {
                    Segment::Unmatched(u) => buf.push_str(u.text),
                    Segment::Matched(_) => {
                        return Some(std::mem::take(&mut buf));
                    }
                }
            }
            done = true;
            Some(std::mem::take(&mut buf))
        })
    }

    /// Returns an iterator over immutable references to the contained [`FuzzyMatch`] items.
    #[inline]
    pub fn iter(&self) -> impl Iterator<Item = &FuzzyMatch<'a>> {
        self.inner.iter()
    }

    /// Returns an iterator over mutable references to the contained [`FuzzyMatch`] items.
    #[inline]
    pub fn iter_mut(&mut self) -> impl Iterator<Item = &mut FuzzyMatch<'a>> {
        self.inner.iter_mut()
    }

    /// Returns a mutable reference to the underlying vector of [`FuzzyMatch`] items.
    ///
    /// This can be used to manipulate the contents directly (e.g. push or remove elements), which
    /// is useful before calling `segment_iter()`
    #[inline]
    pub fn inner_mut(&mut self) -> &mut Vec<FuzzyMatch<'a>> {
        &mut self.inner
    }

    /// Returns the number of matches stored in this collection.
    #[inline]
    #[must_use]
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// Returns `true` if the collection contains no matches.
    #[inline]
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Filters the fuzzy matches by a predicate, returning a new `FuzzyMatches`
    /// containing only those matches for which the predicate returns `true`.
    ///
    /// # Parameters
    /// - `pred`: A closure `Fn(&FuzzyMatch<'a>) -> bool` applied to each match.
    ///
    /// # Returns
    /// A `FuzzyMatches<'a>` with only the matches that satisfy `pred`.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use fuzzy_aho_corasick::{FuzzyAhoCorasickBuilder, FuzzyLimits, FuzzyMatch};
    ///
    /// let engine = FuzzyAhoCorasickBuilder::new()
    ///     .fuzzy(FuzzyLimits::new().edits(1))
    ///     .case_insensitive(true)
    ///     .build(["ipsum", "lorem"]);
    ///
    /// assert_eq!(engine.search_non_overlapping("ipsum and l0rem", 0.5)
    ///     .filter(|m| m.text.contains("0"))
    ///     .replace(|m| Some(format!("**{}**", m.text))), "ipsum and **l0rem**");
    /// ```
    #[must_use]
    pub fn filter<F>(&mut self, pred: F) -> &mut Self
    where
        F: Fn(&FuzzyMatch<'a>) -> bool,
    {
        self.inner.retain(pred);
        self
    }
}
