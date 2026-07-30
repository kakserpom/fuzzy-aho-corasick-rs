//! Options controlling a [`search`](crate::FuzzyAhoCorasick::search) / related call: the similarity
//! threshold, how matches are ranked, and how overlaps are resolved.

/// Default similarity threshold used when [`SearchOptions`] doesn't set one. `0.0` keeps every match
/// the configured edit limits allow — the limits are the real quality gate, so an unset threshold
/// adds no *extra* similarity filter. Set [`SearchOptions::threshold`] to filter more aggressively.
pub const DEFAULT_THRESHOLD: f32 = 0.0;

/// How the raw matches are ranked before they're returned (and before overlap resolution).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Order {
    /// No ranking — the raw best-per-span matches in no particular order (fastest).
    #[default]
    Unsorted,
    /// Higher similarity first, then longer pattern, then longer matched text, then earlier span.
    Default,
    /// Longer pattern first, then higher similarity — favors covering more text with larger patterns.
    Greedy,
    /// By `similarity² × pattern length` — a longer good match can beat a short perfect one.
    CoverageWeighted,
}

/// How overlapping matches are resolved after ranking.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Overlap {
    /// Keep every match, including ones whose spans overlap.
    #[default]
    Keep,
    /// Greedily keep matches in ranked order, dropping any that overlap one already kept.
    NonOverlapping,
    /// Like [`NonOverlapping`](Overlap::NonOverlapping), and additionally at most one match per
    /// pattern identity (its `custom_unique_id`, else its index).
    NonOverlappingUnique,
}

/// Configuration for a search: the similarity `threshold`, the ranking `order`, and the `overlap`
/// resolution. Construct with [`SearchOptions::new`] (all defaults) and refine with the chainable
/// setters; or build one literally.
///
/// ```
/// use fuzzy_aho_corasick::SearchOptions;
/// let opts = SearchOptions::new().threshold(0.8).greedy().non_overlapping_unique();
/// ```
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SearchOptions {
    /// Minimum similarity a match must reach to be kept (`0.0..=1.0`). Defaults to
    /// [`DEFAULT_THRESHOLD`].
    pub threshold: f32,
    /// How matches are ranked.
    pub order: Order,
    /// How overlaps are resolved.
    pub overlap: Overlap,
}

impl Default for SearchOptions {
    fn default() -> Self {
        Self {
            threshold: DEFAULT_THRESHOLD,
            order: Order::Unsorted,
            overlap: Overlap::Keep,
        }
    }
}

impl SearchOptions {
    /// All defaults: [`DEFAULT_THRESHOLD`], [`Order::Unsorted`], [`Overlap::Keep`].
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the similarity threshold (`0.0..=1.0`).
    #[must_use]
    pub fn threshold(mut self, threshold: f32) -> Self {
        self.threshold = threshold;
        self
    }

    /// Set the ranking order explicitly.
    #[must_use]
    pub fn order(mut self, order: Order) -> Self {
        self.order = order;
        self
    }

    /// Set the overlap-resolution mode explicitly.
    #[must_use]
    pub fn overlap(mut self, overlap: Overlap) -> Self {
        self.overlap = overlap;
        self
    }

    /// Shortcut for [`Order::Default`].
    #[must_use]
    pub fn sorted(self) -> Self {
        self.order(Order::Default)
    }

    /// Shortcut for [`Order::Greedy`].
    #[must_use]
    pub fn greedy(self) -> Self {
        self.order(Order::Greedy)
    }

    /// Shortcut for [`Order::CoverageWeighted`].
    #[must_use]
    pub fn coverage_weighted(self) -> Self {
        self.order(Order::CoverageWeighted)
    }

    /// Shortcut for [`Overlap::NonOverlapping`].
    #[must_use]
    pub fn non_overlapping(self) -> Self {
        self.overlap(Overlap::NonOverlapping)
    }

    /// Shortcut for [`Overlap::NonOverlappingUnique`].
    #[must_use]
    pub fn non_overlapping_unique(self) -> Self {
        self.overlap(Overlap::NonOverlappingUnique)
    }
}
