#![warn(clippy::pedantic)]
#![warn(missing_docs)]
// The automaton deliberately stores node indices and grapheme positions as `u32` to keep the
// hot-path structs compact; the corresponding `usize -> u32` casts are sound for any realistic
// input (fewer than ~4 billion nodes, haystacks under ~4 GiB — see `search_raw`).
#![allow(
    clippy::too_many_lines,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation
)]

//! Unicode-aware Aho–Corasick automaton with **fuzzy matching**: substitutions, insertions,
//! deletions, and transpositions, over grapheme clusters, with optional case-insensitive folding.
//!
//! Build an immutable engine with [`FuzzyAhoCorasickBuilder`], then query it. Similarity for a
//! candidate match against a length-`N` pattern is `(N - penalties) / N * weight`, and a match is
//! kept when it meets the caller's threshold. Edit limits can be set globally on the builder or
//! per [`Pattern`], and edit costs are tuned via [`FuzzyPenalties`].
//!
//! # Example
//! ```
//! use fuzzy_aho_corasick::{FuzzyAhoCorasickBuilder, FuzzyLimits, SearchOptions};
//!
//! let engine = FuzzyAhoCorasickBuilder::new()
//!     .fuzzy(FuzzyLimits::new().edits(1))
//!     .case_insensitive(true)
//!     .build(["hello", "world"]);
//!
//! // Two typos: an extra 'l' (insertion) and swapped 'lr' (transposition).
//! let matches = engine.search("helllo wolrd", &SearchOptions::new().threshold(0.8).sorted().non_overlapping()).unwrap();
//! let found: Vec<&str> = matches.iter().map(|m| m.pattern.as_str()).collect();
//! assert!(found.contains(&"hello") && found.contains(&"world"));
//! ```
//!
//! # Bounding worst-case work
//! The search is exact by default. When a high edit budget is combined with a low threshold the
//! state space can explode; [`FuzzyAhoCorasickBuilder::beam_width`] caps the active frontier, and
//! [`FuzzyAhoCorasickBuilder::auto_beam`] stays exact until a state budget is exceeded and only
//! then engages a beam — leaving ordinary searches unaffected.
//!
//! # Bit-parallel pre-filter
//! For large, mostly-non-matching inputs, [`FuzzyAhoCorasick::with_prefilter`] returns a
//! [`Prefiltered`] wrapper that runs a bit-parallel approximate scan to locate candidate regions and
//! only re-searches those with the full engine. Results are identical to [`FuzzyAhoCorasick::search`];
//! it falls back to a plain full search when the configuration can't be reduced to the bit model.
//!
//! See the [README](https://github.com/kakserpom/fuzzy-aho-corasick-rs) for a full guide.
mod builder;
mod error;
mod grapheme;
mod matches;
mod options;
mod prefilter;
mod query;
mod replacer;
mod search;
mod stream;
/// The crate's public data types (patterns, limits, penalties, matches, segments, …). Everything
/// here is also re-exported at the crate root, so `use fuzzy_aho_corasick::Pattern` and
/// `use fuzzy_aho_corasick::structs::Pattern` are equivalent.
pub mod structs;
#[cfg(test)]
mod tests;

// Compile-check every example in the mdBook guide as a doctest, so the docs can't drift from the
// API. `#[cfg(doctest)]` keeps these out of the generated docs and normal builds; they run under
// `cargo test --doc`. Blocks that can't actually execute (open files, etc.) are tagged `no_run` in
// the source markdown. (The README is not included here: it renders verbatim on GitHub/crates.io,
// where the hidden `#` lines and `no_run` tags doctests need would show as noise.)
#[cfg(doctest)]
mod book_doctests {
    macro_rules! chapter {
        ($name:ident, $path:literal) => {
            #[doc = include_str!($path)]
            mod $name {}
        };
    }
    chapter!(quick_start, "../book/src/getting-started/quick-start.md");
    chapter!(installation, "../book/src/getting-started/installation.md");
    chapter!(builder, "../book/src/building/builder.md");
    chapter!(patterns, "../book/src/building/patterns.md");
    chapter!(penalties, "../book/src/building/penalties.md");
    chapter!(scoring, "../book/src/concepts/scoring.md");
    chapter!(search, "../book/src/searching/search.md");
    chapter!(replacement, "../book/src/searching/replacement.md");
    chapter!(segmentation, "../book/src/searching/segmentation.md");
    chapter!(custom, "../book/src/similarity/custom.md");
    chapter!(floor, "../book/src/similarity/floor.md");
    chapter!(mappings, "../book/src/similarity/mappings.md");
    chapter!(bounding, "../book/src/performance/bounding.md");
    chapter!(prefilter, "../book/src/performance/prefilter.md");
    chapter!(stream_search, "../book/src/streaming/search.md");
    chapter!(stream_replace, "../book/src/streaming/replace.md");
}

pub use builder::FuzzyAhoCorasickBuilder;
pub use error::SearchError;
pub use options::{DEFAULT_THRESHOLD, Order, Overlap, SearchOptions};
pub use prefilter::Prefiltered;
pub use replacer::FuzzyReplacer;
pub use stream::{StreamMatch, StreamMatches};
/// Index of a pattern within the automaton's pattern list — the `pattern_index` on a
/// [`FuzzyMatch`], and the position of a pattern in the slice passed to `build`.
pub type PatternIndex = usize;
pub use structs::*;
