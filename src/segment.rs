use crate::{FuzzyAhoCorasick, FuzzyMatches, Segment};
const SPACE: [char; 2] = ['\x20', '\t'];
const NO_LEADING_SPACE_PUNCTUATION: [char; 9] = [',', '.', '?', '!', ';', ':', '—', '-', '…'];

impl<'a> FuzzyMatches<'a> {
    /// Returns an **iterator** that yields interleaving [`Segment::Matched`]
    /// [`Segment::Unmatched`] items for the given text.
    pub fn segment_iter(mut self) -> impl Iterator<Item = Segment<'a>> {
        #[cfg(test)]
        println!("matches: {:?}", self.inner);
        self.inner.sort_by_key(|m| m.start);
        let mut segments = Vec::new();
        let mut last = 0;
        for m in self.inner {
            if m.start >= last {
                if m.start > last {
                    segments.push(Segment::Unmatched(&self.haystack[last..m.start]));
                }
                last = m.end;
                segments.push(Segment::Matched(m));
            }
        }
        if last < self.haystack.len() {
            segments.push(Segment::Unmatched(&self.haystack[last..]));
        }
        segments.into_iter()
    }

    /// Convenience wrapper around [`segment_iter`](Self::segment_iter).
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
                Segment::Unmatched(s) => {
                    if prev_matched && !s.starts_with(NO_LEADING_SPACE_PUNCTUATION) {
                        result.push(' ');
                    }
                    prev_matched = false;
                    result.push_str(s);
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
        &self,
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
