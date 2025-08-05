use crate::{FuzzyAhoCorasick, FuzzyMatches, Segment, UnmatchedSegment};
const SPACE: [char; 2] = ['\x20', '\t'];
const NO_LEADING_SPACE_PUNCTUATION: [char; 9] = [',', '.', '?', '!', ';', ':', '—', '-', '…'];

impl<'a> FuzzyMatches<'a> {
    /// Returns an iterator over the haystack split into interleaved segments:
    /// `Segment::Unmatched` for the gaps and `Segment::Matched` for accepted
    /// fuzzy matches. Matches are first sorted by their `start` so the output is
    /// left-to-right and non-overlapping in the order they appear.
    ///
    /// # Behavior
    /// - Skips overlaps by virtue of assuming `self.inner` already contains the
    ///   desired non-overlapping set (it does not dedupe here).
    /// - Emits unmatched prefix/suffix pieces as `Unmatched`.
    pub fn segment_iter(self) -> impl Iterator<Item = Segment<'a>> {
        #[cfg(test)]
        println!("matches: {:?}", self.inner);
        let mut segments = Vec::new();
        let mut last = 0;
        for m in self.inner {
            if m.start >= last {
                if m.start > last {
                    segments.push(Segment::Unmatched(UnmatchedSegment {
                        start: last,
                        end: m.start,
                        text: &self.haystack[last..m.start],
                    }));
                }
                last = m.end;
                segments.push(Segment::Matched(m));
            }
        }
        let len = self.haystack.len();
        if last < len {
            segments.push(Segment::Unmatched(UnmatchedSegment {
                start: last,
                end: len,
                text: &self.haystack[last..],
            }));
        }
        segments.into_iter()
    }

    /// Reconstructs a cleaned-up version of the haystack by concatenating
    /// segments from `segment_iter`, inserting spaces intelligently to avoid
    /// unwanted joins or extra whitespace around punctuation.
    ///
    /// Rules:
    /// - Inserts a space before a matched segment if the previous was also
    ///   matched or if the accumulated result doesn’t already end with a space.
    /// - Inserts a space before an unmatched segment only if the previous segment
    ///   was matched and the unmatched text does not start with a punctuation
    ///   character that should not have a leading space.
    #[must_use]
    pub fn segment_text(self) -> String {
        let mut result = String::new();
        let mut prev_matched = false;
        for segment in self.segment_iter() {
            #[cfg(test)]
            println!("segment: {:?}", segment);
            match segment {
                Segment::Matched(m) => {
                    if prev_matched || (!result.is_empty() && !result.ends_with(SPACE)) {
                        result.push(' ');
                    }
                    prev_matched = true;
                    result.push_str(m.text);
                }
                Segment::Unmatched(u) => {
                    if prev_matched && !u.text.starts_with(NO_LEADING_SPACE_PUNCTUATION) {
                        result.push(' ');
                    }
                    prev_matched = false;
                    result.push_str(u.text);
                }
            }
        }
        result
    }
}
impl FuzzyAhoCorasick {
    /// Returns an **iterator** that yields interleaving [`Segment::Matched`]
    /// [`Segment::Unmatched`] items for the given text.
    pub fn segment_iter<'a>(
        &'a self,
        haystack: &'a str,
        threshold: f32,
    ) -> impl Iterator<Item = Segment<'a>> {
        self.search_non_overlapping(haystack, threshold)
            .segment_iter()
    }
    /// Convenience wrapper around [`segment_iter`](Self::segment_iter).
    #[must_use]
    pub fn segment_text(&self, haystack: &str, threshold: f32) -> String {
        self.search_non_overlapping(haystack, threshold)
            .segment_text()
    }
}
