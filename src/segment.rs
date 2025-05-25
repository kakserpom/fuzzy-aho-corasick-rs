use crate::{FuzzyAhoCorasick, Segment};

impl FuzzyAhoCorasick {
    /// Returns an **iterator** that yields interleaving [`Segment::Matched`]
    /// [`Segment::Unmatched`] items for the given text.
    pub fn segment_iter<'b>(
        &self,
        haystack: &'b str,
        threshold: f32,
    ) -> impl Iterator<Item = Segment<'b>> {
        let mut matches = self.search(haystack, threshold);
        #[cfg(test)]
        println!("matches: {:?}", matches);
        matches.sort_by_key(|m| m.start);
        let mut segments = Vec::new();
        let mut last = 0;
        for m in matches {
            if m.start >= last {
                if m.start > last {
                    segments.push(Segment::Unmatched(&haystack[last..m.start]));
                }
                last = m.end;
                segments.push(Segment::Matched(m));
            }
        }
        if last < haystack.len() {
            segments.push(Segment::Unmatched(&haystack[last..]));
        }
        segments.into_iter()
    }

    /// Convenience wrapper around [`segment_iter`](Self::segment_iter).
    #[must_use]
    pub fn segment_text(&self, haystack: &str, threshold: f32) -> String {
        let mut result = String::new();
        let mut prev_matched = false;
        for segment in self.segment_iter(haystack, threshold) {
            #[cfg(test)]
            println!("segment: {:?}", segment);
            match segment {
                Segment::Matched(m) => {
                    if prev_matched {
                        result.push(' ');
                    }
                    prev_matched = true;
                    result.push_str(&m.text);
                }
                Segment::Unmatched(s) => {
                    if prev_matched {
                        result.push(' ');
                    }
                    prev_matched = false;
                    result.push_str(s.trim());
                }
            }
        }
        result
    }
}
