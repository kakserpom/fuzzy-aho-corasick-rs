#![warn(clippy::pedantic)]
// The automaton deliberately stores node indices and grapheme positions as `u32` to keep the
// hot-path structs compact; the corresponding `usize -> u32` casts are sound for any realistic
// input (fewer than ~4 billion nodes, haystacks under ~4 GiB — see `search_unsorted`).
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
//! use fuzzy_aho_corasick::{FuzzyAhoCorasickBuilder, FuzzyLimits};
//!
//! let engine = FuzzyAhoCorasickBuilder::new()
//!     .fuzzy(FuzzyLimits::new().edits(1))
//!     .case_insensitive(true)
//!     .build(["hello", "world"]);
//!
//! // Two typos: an extra 'l' (insertion) and swapped 'lr' (transposition).
//! let matches = engine.search_non_overlapping("helllo wolrd", 0.8).unwrap();
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
mod prefilter;
mod query;
mod replacer;
mod search;
mod stream;
pub mod structs;
#[cfg(test)]
mod tests;

pub use builder::FuzzyAhoCorasickBuilder;
pub use error::SearchError;
pub use prefilter::Prefiltered;
pub use replacer::FuzzyReplacer;
pub use stream::{StreamMatch, StreamMatches};
pub type PatternIndex = usize;
pub use structs::*;
