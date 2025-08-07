use crate::PatternIndex;
use std::collections::BTreeMap;
use std::fmt;
use unicode_segmentation::UnicodeSegmentation;

pub type NumEdits = usize;
#[derive(Clone)]
pub(crate) struct State {
    pub(crate) node: usize,
    pub(crate) j: usize,
    pub(crate) matched_start: usize,
    pub(crate) matched_end: usize,
    pub(crate) penalties: f32,
    pub(crate) edits: NumEdits,
    pub(crate) insertions: NumEdits,
    pub(crate) deletions: NumEdits,
    pub(crate) substitutions: NumEdits,
    pub(crate) swaps: NumEdits,
    #[cfg(debug_assertions)]
    pub(crate) notes: Vec<String>,
}

/// A single node inside the internal Aho–Corasick automaton.
#[derive(Clone, Debug)]
pub(crate) struct Node {
    pub(crate) pattern_index: Option<PatternIndex>,
    pub(crate) epsilon: Option<usize>,
    /// Outgoing edges keyed by the next character.
    pub(crate) transitions: BTreeMap<String, usize>,
    /// Failure link (classic AC fallback state).
    pub(crate) fail: usize,
    /// All patterns that end in this state.
    pub(crate) output: Vec<usize>,
    /// Pre‑computed prefix weight (see [`FuzzyAhoCorasickBuilder::pmf`]).
    pub(crate) weight: f32,
    /// Index of the parent state – only present in *debug* builds to make
    /// visualising / debugging the trie easier.
    #[cfg(debug_assertions)]
    #[allow(dead_code)]
    pub(crate) parent: usize,
    /// Character that leads from `parent` to this node – stored only in
    /// *debug* builds for introspection.
    #[allow(dead_code)]
    #[cfg(debug_assertions)]
    pub(crate) grapheme: Option<String>,
}

#[derive(Debug, Default, Clone, PartialEq)]
pub struct FuzzyLimits {
    pub(crate) insertions: Option<NumEdits>,
    pub(crate) deletions: Option<NumEdits>,
    pub(crate) substitutions: Option<NumEdits>,
    pub(crate) swaps: Option<NumEdits>,
    pub(crate) edits: Option<NumEdits>,
}

impl FuzzyLimits {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
    #[must_use]
    pub fn insertions(mut self, num: NumEdits) -> Self {
        self.insertions = Some(num);
        self
    }
    #[must_use]
    pub(crate) fn finalize(mut self) -> Self {
        if self.edits.is_none() {
            if self.insertions.is_none() {
                self.insertions = Some(0);
            }
            if self.deletions.is_none() {
                self.deletions = Some(0);
            }
            if self.substitutions.is_none() {
                self.substitutions = Some(0);
            }
            if self.swaps.is_none() {
                self.swaps = Some(0);
            }
        }
        self
    }
    #[must_use]
    pub fn deletions(mut self, num: NumEdits) -> Self {
        self.deletions = Some(num);
        self
    }
    #[must_use]
    pub fn substitutions(mut self, num: NumEdits) -> Self {
        self.substitutions = Some(num);
        self
    }
    #[must_use]
    pub fn swaps(mut self, num: NumEdits) -> Self {
        self.swaps = Some(num);
        self
    }

    #[must_use]
    pub fn edits(mut self, num: NumEdits) -> Self {
        self.edits = Some(num);
        self
    }
}

#[derive(Debug, Clone)]
pub struct FuzzyPenalties {
    pub insertion: f32,
    pub deletion: f32,
    pub substitution: f32,
    pub swap: f32,
}

impl Default for FuzzyPenalties {
    fn default() -> Self {
        let m = 1.;
        Self {
            substitution: 1. * m,
            insertion: 0.4 * m,
            deletion: 0.7 * m,
            swap: 0.4 * m,
        }
    }
}

impl FuzzyPenalties {
    #[must_use]
    pub fn insertion(mut self, penalty: f32) -> Self {
        self.insertion = penalty;
        self
    }
    #[must_use]
    pub fn deletion(mut self, penalty: f32) -> Self {
        self.deletion = penalty;
        self
    }
    #[must_use]
    pub fn substitution(mut self, penalty: f32) -> Self {
        self.substitution = penalty;
        self
    }
    #[must_use]
    pub fn swap(mut self, penalty: f32) -> Self {
        self.swap = penalty;
        self
    }
}

impl Node {
    /// Helper used by the builder to create a brand‑new node.
    pub(crate) fn new(
        #[cfg(debug_assertions)] parent: usize,
        #[cfg(debug_assertions)] grapheme: Option<&str>,
    ) -> Node {
        Self {
            pattern_index: None,
            transitions: BTreeMap::new(),
            fail: 0,
            output: Vec::new(),
            weight: 0.0,
            #[cfg(debug_assertions)]
            parent,
            #[cfg(debug_assertions)]
            grapheme: grapheme.map(str::to_string),
            epsilon: None,
        }
    }
}

#[derive(Clone)]
pub struct FuzzyAhoCorasick {
    /// Nodes
    pub(crate) nodes: Vec<Node>,
    /// Patterns
    pub(crate) patterns: Vec<Pattern>,
    /// Similarity map
    pub(crate) similarity: &'static BTreeMap<(char, char), f32>,
    /// Limits of errors
    pub(crate) limits: Option<FuzzyLimits>,
    /// Weight
    pub(crate) penalties: FuzzyPenalties,
    /// Case insensitivity
    pub(crate) case_insensitive: bool,
}

#[allow(clippy::missing_fields_in_debug)]
impl fmt::Debug for FuzzyAhoCorasick {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut s = &mut f.debug_struct("FuzzyAhoCorasick");
        if let Some(limits) = &self.limits {
            s = s.field("limits", limits);
        }
        if self.case_insensitive {
            s = s.field("case_insensitive", &self.case_insensitive);
        }
        s.field("patterns", &self.patterns).finish()
    }
}

#[derive(Debug, Clone, Ord, PartialOrd, Eq, PartialEq)]
pub enum UniqueId {
    Automatic(usize),
    Custom(usize),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Pattern {
    pub grapheme_len: usize,
    pub pattern: String,
    pub custom_unique_id: Option<usize>,
    pub weight: f32,
    pub limits: Option<FuzzyLimits>,
}

impl Pattern {
    #[must_use]
    pub fn as_str(&self) -> &str {
        self.pattern.as_str()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.pattern.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Set pattern weight. Default is 1.0
    #[must_use]
    pub fn weight(mut self, weight: f32) -> Self {
        self.weight = weight;
        self
    }

    /// Set Fuzzy limits per-pattern pattern
    #[must_use]
    pub fn fuzzy(mut self, limits: FuzzyLimits) -> Self {
        self.limits = Some(limits.finalize());
        self
    }

    #[must_use]
    pub fn custom_unique_id(mut self, id: usize) -> Self {
        self.custom_unique_id = Some(id);
        self
    }
}

impl From<&str> for Pattern {
    fn from(s: &str) -> Self {
        Pattern {
            pattern: s.to_owned(),
            grapheme_len: s.graphemes(true).count(),
            weight: 1.,
            limits: None,
            custom_unique_id: None,
        }
    }
}

impl From<String> for Pattern {
    fn from(s: String) -> Self {
        Pattern {
            grapheme_len: s.graphemes(true).count(),
            pattern: s,
            custom_unique_id: None,
            weight: 1.,
            limits: None,
        }
    }
}

impl From<&String> for Pattern {
    fn from(s: &String) -> Self {
        Pattern {
            pattern: s.clone(),
            grapheme_len: s.graphemes(true).count(),
            weight: 1.,
            limits: None,
            custom_unique_id: None,
        }
    }
}

impl From<(&str, f32)> for Pattern {
    fn from((s, w): (&str, f32)) -> Self {
        Pattern {
            pattern: s.to_string(),
            grapheme_len: s.graphemes(true).count(),
            weight: w,
            limits: None,
            custom_unique_id: None,
        }
    }
}

impl From<(String, f32)> for Pattern {
    fn from((s, w): (String, f32)) -> Self {
        Pattern {
            grapheme_len: s.graphemes(true).count(),
            pattern: s,
            custom_unique_id: None,
            weight: w,
            limits: None,
        }
    }
}

impl From<(&String, f32)> for Pattern {
    fn from((s, w): (&String, f32)) -> Self {
        Pattern {
            pattern: s.clone(),
            grapheme_len: s.graphemes(true).count(),
            weight: w,
            limits: None,
            custom_unique_id: None,
        }
    }
}

impl<'a> From<(&'a str, f32, usize)> for Pattern {
    fn from((s, w, max_edits): (&'a str, f32, usize)) -> Self {
        Pattern {
            pattern: s.to_owned(),
            grapheme_len: s.graphemes(true).count(),
            weight: w,
            limits: Some(
                FuzzyLimits::default()
                    .edits(max_edits as NumEdits)
                    .finalize(),
            ),
            custom_unique_id: None,
        }
    }
}

impl<'a> From<(String, f32, usize)> for Pattern {
    fn from((s, w, max_edits): (String, f32, usize)) -> Self {
        Pattern {
            grapheme_len: s.graphemes(true).count(),
            pattern: s,
            weight: w,
            limits: Some(
                FuzzyLimits::default()
                    .edits(max_edits as NumEdits)
                    .finalize(),
            ),
            custom_unique_id: None,
        }
    }
}

/// Result returned by [`FuzzyAhoCorasick::search`].
#[derive(Debug, Clone, PartialEq)]
pub struct FuzzyMatch<'a> {
    /// Number of insertions.
    pub insertions: NumEdits,
    /// Number of deletions.
    pub deletions: NumEdits,
    /// Number of substitutions.
    pub substitutions: NumEdits,
    /// Number of swaps (transpositions)
    pub swaps: NumEdits,
    /// Total number of edits
    pub edits: NumEdits,
    /// Pattern indexed (0-based)
    pub pattern_index: usize,
    /// Pattern that has been matched.
    pub pattern: &'a Pattern,
    /// Inclusive start byte index.
    pub start: usize,
    /// Exclusive end byte index.
    pub end: usize,
    /// Final similarity score ∈ `[0,1]`.
    pub similarity: f32,
    /// Slice of the original text that produced the match.
    pub text: &'a str,
    #[cfg(debug_assertions)]
    pub notes: Vec<String>,
}

/// Result of [`FuzzyAhoCorasick::segment_iter`]: either a successful match or
/// an "unmatched" gap between them.
#[derive(Debug, Clone, PartialEq)]
pub enum Segment<'a> {
    Matched(FuzzyMatch<'a>),
    Unmatched(UnmatchedSegment<'a>),
}
impl<'a> Segment<'a> {
    #[must_use]
    pub fn matched(&'a self) -> Option<&'a FuzzyMatch<'a>> {
        if let Segment::Matched(matched) = self {
            Some(&matched)
        } else {
            None
        }
    }
    #[must_use]
    pub fn unmatched(&'a self) -> Option<&'a UnmatchedSegment<'a>> {
        if let Segment::Unmatched(unmatched) = self {
            Some(&unmatched)
        } else {
            None
        }
    }
}
#[derive(Debug, Clone, PartialEq)]
pub struct UnmatchedSegment<'a> {
    /// Inclusive start byte index.
    pub start: usize,
    /// Exclusive end byte index.
    pub end: usize,
    /// Slice of the original text
    pub text: &'a str,
}

impl Segment<'_> {
    #[must_use]
    pub fn len(&self) -> usize {
        match self {
            Segment::Matched(m) => m.text.len(),
            Segment::Unmatched(u) => u.text.len(),
        }
    }
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    #[must_use]
    pub fn as_str(&self) -> &str {
        match self {
            Segment::Matched(m) => m.text,
            Segment::Unmatched(u) => u.text,
        }
    }
}

#[derive(Debug)]
pub struct FuzzyMatches<'a> {
    pub(crate) haystack: &'a str,
    pub inner: Vec<FuzzyMatch<'a>>,
}
impl<'a, 'b> IntoIterator for &'b FuzzyMatches<'a> {
    type Item = &'b FuzzyMatch<'a>;
    type IntoIter = std::slice::Iter<'b, FuzzyMatch<'a>>;
    fn into_iter(self) -> Self::IntoIter {
        self.inner.iter()
    }
}
impl<'a, 'b> IntoIterator for &'b mut FuzzyMatches<'a> {
    type Item = &'b mut FuzzyMatch<'a>;
    type IntoIter = std::slice::IterMut<'b, FuzzyMatch<'a>>;
    fn into_iter(self) -> Self::IntoIter {
        self.inner.iter_mut()
    }
}
impl<'a> IntoIterator for FuzzyMatches<'a> {
    type Item = FuzzyMatch<'a>;
    type IntoIter = std::vec::IntoIter<FuzzyMatch<'a>>;
    fn into_iter(self) -> Self::IntoIter {
        self.inner.into_iter()
    }
}
impl<'a> std::ops::Deref for FuzzyMatches<'a> {
    type Target = [FuzzyMatch<'a>];
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}
impl std::ops::DerefMut for FuzzyMatches<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}
