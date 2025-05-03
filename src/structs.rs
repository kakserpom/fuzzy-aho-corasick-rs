use crate::PatternIndex;
use std::collections::BTreeMap;
use std::fmt;

pub type NumEdits = usize;
#[derive(Clone)]
pub(crate) struct State {
    pub(crate) node: usize,
    pub(crate) j: usize,
    pub(crate) matched_start: usize,
    pub(crate) matched_end: usize,
    pub(crate) score: f32,
    pub(crate) edits: NumEdits,
    pub(crate) insertions: NumEdits,
    pub(crate) deletions: NumEdits,
    pub(crate) substitutions: NumEdits,
    pub(crate) swaps: NumEdits,
}

/// A single node inside the internal Aho–Corasick automaton.
#[derive(Debug)]
pub(crate) struct Node {
    pub(crate) pattern_index: Option<PatternIndex>,
    //epsilon: Option<usize>,
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

#[derive(Debug, Default)]
pub struct FuzzyLimits {
    pub insertions: Option<NumEdits>,
    pub deletions: Option<NumEdits>,
    pub substitutions: Option<NumEdits>,
    pub swaps: Option<NumEdits>,
    pub edits: Option<NumEdits>,
}

impl FuzzyLimits {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insertions(mut self, num: NumEdits) -> Self {
        self.insertions = Some(num);
        self
    }

    pub fn deletions(mut self, num: NumEdits) -> Self {
        self.deletions = Some(num);
        self
    }

    pub fn substitutions(mut self, num: NumEdits) -> Self {
        self.substitutions = Some(num);
        self
    }

    pub fn swaps(mut self, num: NumEdits) -> Self {
        self.swaps = Some(num);
        self
    }

    pub fn edits(mut self, num: NumEdits) -> Self {
        self.edits = Some(num);
        self
    }
}

#[derive(Debug)]
pub struct FuzzyPenalties {
    pub insertion: f32,
    pub deletion: f32,
    pub substitution: f32,
    pub swap: f32,
}

impl Default for FuzzyPenalties {
    fn default() -> Self {
        Self {
            substitution: 0.6,
            insertion: 0.9,
            deletion: 0.9,
            swap: 0.9,
        }
    }
}

impl FuzzyPenalties {
    pub fn insertion(mut self, penalty: f32) -> Self {
        self.insertion = penalty;
        self
    }
    pub fn deletion(mut self, penalty: f32) -> Self {
        self.deletion = penalty;
        self
    }
    pub fn substitution(mut self, penalty: f32) -> Self {
        self.substitution = penalty;
        self
    }
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
            grapheme: grapheme.map(|s| s.to_string()),
        }
    }
}

pub struct FuzzyAhoCorasick {
    pub(crate) nodes: Vec<Node>,
    pub(crate) patterns: Vec<Pattern>,
    pub(crate) similarity: &'static BTreeMap<(char, char), f32>,
    pub(crate) limits: Option<FuzzyLimits>,
    pub(crate) penalties: FuzzyPenalties,
    pub(crate) non_overlapping: bool,
    pub(crate) case_insensitive: bool,
}

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

#[derive(Debug)]
pub struct Pattern {
    pub pattern: String,
    pub weight: f32,
    pub limits: Option<FuzzyLimits>,
}

impl Pattern {
    pub fn weight(mut self, weight: f32) -> Self {
        self.weight = weight;
        self
    }

    pub fn fuzzy(mut self, limits: FuzzyLimits) -> Self {
        self.limits = Some(limits);
        self
    }
}
impl From<&str> for Pattern {
    fn from(s: &str) -> Self {
        Pattern {
            pattern: s.to_owned(),
            weight: 1.0,
            limits: None,
        }
    }
}

impl From<String> for Pattern {
    fn from(s: String) -> Self {
        Pattern {
            pattern: s,
            weight: 1.0,
            limits: None,
        }
    }
}

impl From<&String> for Pattern {
    fn from(s: &String) -> Self {
        Pattern {
            pattern: s.clone(),
            weight: 1.0,
            limits: None,
        }
    }
}

impl From<(&str, f32)> for Pattern {
    fn from((s, w): (&str, f32)) -> Self {
        Pattern {
            pattern: s.to_string(),
            weight: w,
            limits: None,
        }
    }
}

impl From<(String, f32)> for Pattern {
    fn from((s, w): (String, f32)) -> Self {
        Pattern {
            pattern: s,
            weight: w,
            limits: None,
        }
    }
}

impl From<(&String, f32)> for Pattern {
    fn from((s, w): (&String, f32)) -> Self {
        Pattern {
            pattern: s.clone(),
            weight: w,
            limits: None,
        }
    }
}

impl<'a> From<(&'a str, f32, usize)> for Pattern {
    fn from((s, w, max_edits): (&'a str, f32, usize)) -> Self {
        Pattern {
            pattern: s.to_owned(),
            weight: w,
            limits: Some(FuzzyLimits::default().edits(max_edits as NumEdits)),
        }
    }
}

/// Result returned by [`FuzzyAhoCorasick::search`].
#[derive(Debug, Clone, PartialEq)]
pub struct FuzzyMatch {
    /// Number of insertions.
    pub insertions: NumEdits,
    pub deletions: NumEdits,
    pub substitutions: NumEdits,
    pub swaps: NumEdits,
    pub pattern_index: usize,
    /// Inclusive start byte index.
    pub start: usize,
    /// Exclusive end byte index.
    pub end: usize,
    /// Pattern that has been matched.
    pub pattern: String,
    /// Final similarity score ∈ `[0,1]`.
    pub similarity: f32,
    /// Slice of the original text that produced the match.
    pub text: String,
}

/// Result of [`FuzzyAhoCorasick::segment_iter`]: either a successful match or
/// an "unmatched" gap between them.
#[derive(Debug, Clone, PartialEq)]
pub enum Segment<'a> {
    Matched(FuzzyMatch),
    Unmatched(&'a str),
}
