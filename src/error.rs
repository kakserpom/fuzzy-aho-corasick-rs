//! Error type returned by the fallible search entry points.

/// An error from a search call.
///
/// Currently, the only failure mode is an over-large haystack; the enum is `#[non_exhaustive]` so
/// further fallible cases can be added later without another breaking change.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum SearchError {
    /// The haystack has more grapheme clusters than the `u32` position space the engine indexes
    /// with (roughly a 4 GiB ASCII input). Use the streaming API (`search_stream` / `stream_matches`
    /// / `replace_stream`), which windows the input and reports absolute `u64` offsets.
    HaystackTooLarge {
        /// The haystack's grapheme-cluster count (which exceeded `u32::MAX`).
        graphemes: usize,
    },
}

impl core::fmt::Display for SearchError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            SearchError::HaystackTooLarge { graphemes } => write!(
                f,
                "haystack has {graphemes} grapheme clusters, exceeding the u32 position space this \
                 engine indexes with; use the streaming API for inputs larger than ~4 GiB"
            ),
        }
    }
}

impl std::error::Error for SearchError {}
