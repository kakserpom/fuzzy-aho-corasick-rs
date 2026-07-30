# Search & Selection

A search returns `Result<`[`FuzzyMatches`]`, `[`SearchError`]`>` — the matches found at or above the
threshold, or an error if the haystack is too large to index (see [Fallibility](#fallibility)). A
single entry point, `search`, covers every case; how the results are **ordered** and whether
**overlaps** are resolved is chosen through [`SearchOptions`].

## `search(haystack, &SearchOptions)`

There is one search method. [`SearchOptions`] bundles the similarity threshold with an **order** and
an **overlap resolver**, built with chainable setters:

```rust
use fuzzy_aho_corasick::{FuzzyAhoCorasickBuilder, FuzzyLimits, SearchOptions};

let engine = FuzzyAhoCorasickBuilder::new()
    .fuzzy(FuzzyLimits::new().edits(1))
    .case_insensitive(true)
    .build(["hello", "world"]);

let opts = SearchOptions::new()
    .threshold(0.8)      // minimum similarity (default 0.0 — keep everything)
    .sorted()            // Order::Default: similarity, then longer patterns, then position
    .non_overlapping();  // Overlap::NonOverlapping: greedily drop overlaps in the chosen order
let matches = engine.search("helllo wolrd", &opts).unwrap();
```

`SearchOptions::default()` (or `SearchOptions::new()`) is the primitive: it returns the single
best-scoring match for each distinct `(start, end, pattern)` span, unordered and with overlaps kept —
the fastest result to build. Everything else is that primitive plus an order and/or an overlap
resolver, which **compose** independently.

### Order (`SearchOptions::order`, an [`Order`])

| Setter | `Order` | Ranking |
| --- | --- | --- |
| *(none)* | `Unsorted` (default) | raw best-per-span, no ordering (fastest) |
| `.sorted()` | `Default` | higher similarity, then longer patterns, then earlier position |
| `.greedy()` | `Greedy` | longer patterns first, then similarity |
| `.coverage_weighted()` | `CoverageWeighted` | by `similarity × covered length` |

### Overlap (`SearchOptions::overlap`, an [`Overlap`])

| Setter | `Overlap` | Effect |
| --- | --- | --- |
| *(none)* | `Keep` (default) | keep every match, including overlapping spans |
| `.non_overlapping()` | `NonOverlapping` | greedily drop overlapping matches in the current order |
| `.non_overlapping_unique()` | `NonOverlappingUnique` | as above, and use each pattern id at most once |

Overlap resolution is greedy in the current order, so **choose an order whenever you resolve
overlaps** — e.g. `.sorted().non_overlapping()` yields a default-ranked non-overlapping set, and
`.coverage_weighted().non_overlapping_unique()` yields a coverage-ranked set with at most one match
per pattern id.

## Fallibility

Every entry point returns `Result<_, `[`SearchError`]`>`. The only failure is a haystack with more
than `u32::MAX` grapheme clusters (~4 GiB ASCII): the engine indexes positions with `u32`, so a
larger haystack returns `Err(SearchError::HaystackTooLarge { graphemes })` instead of silently
truncating to wrong offsets. Reach for the [streaming API](../streaming/search.md) for inputs that
large. The examples here `.unwrap()` for brevity; in real code propagate with `?` or handle the
error.

## Ordering strategies

The orderings are methods on the returned [`FuzzyMatches`]; the convenience entry points above just
call them for you:

- **`default_sort()`** — higher similarity first, then longer patterns, then earlier position. A good
  general default.
- **`greedy_sort()`** — longer patterns first, then similarity. Prefers covering more text with
  larger patterns.
- **`coverage_weighted_sort()`** — ranks by `similarity × covered_length`, so a slightly-lower-scoring
  long match can beat a short perfect one. Useful when short high-similarity fragments would otherwise
  win over the longer pattern you actually care about.

## Non-overlapping selection

Raw results can overlap (several patterns, or several spellings, matching the same region).
`non_overlapping()` greedily keeps matches in the current sort order, dropping any that overlap one
already kept — so **sort first, then resolve**. `non_overlapping_unique()` additionally enforces one
match per pattern identity (see [unique ids](../building/patterns.md)).

```rust
use fuzzy_aho_corasick::{FuzzyAhoCorasickBuilder, FuzzyLimits, SearchOptions};

let engine = FuzzyAhoCorasickBuilder::new()
    .fuzzy(FuzzyLimits::new().edits(1))
    .case_insensitive(true)
    .build(["hello", "world"]);

let matches = engine
    .search("helllo wolrd", &SearchOptions::new().threshold(0.8).sorted().non_overlapping())
    .unwrap();
let found: Vec<&str> = matches.iter().map(|m| m.pattern.as_str()).collect();
assert!(found.contains(&"hello") && found.contains(&"world"));
```

## Working with the results

[`FuzzyMatches`] derefs to `&[FuzzyMatch]` and supports `iter()`, `iter_mut()`, `len()`,
`is_empty()`, and `IntoIterator`. It also offers post-processing helpers:

- `filter(pred)` / `retain(pred)` — keep matches satisfying a predicate.
- `matched_spans()` / `matched_strings()` — the `(start, end)` byte ranges / matched substrings.
- `replace(callback)` — see [Replacement](replacement.md).
- `segment_iter()`, `split()`, `strip_prefix()`, `strip_suffix()` — see
  [Segmentation & Splitting](segmentation.md).

Each [`FuzzyMatch`] carries `pattern_index`, `pattern`, `start`/`end` (byte offsets), `text`,
`similarity`, and the per-type edit counts (`insertions`, `deletions`, `substitutions`, `swaps`,
`edits`).

[`FuzzyMatches`]: https://docs.rs/fuzzy-aho-corasick/latest/fuzzy_aho_corasick/structs/struct.FuzzyMatches.html
[`FuzzyMatch`]: https://docs.rs/fuzzy-aho-corasick/latest/fuzzy_aho_corasick/structs/struct.FuzzyMatch.html
[`SearchError`]: https://docs.rs/fuzzy-aho-corasick/latest/fuzzy_aho_corasick/enum.SearchError.html
[`SearchOptions`]: https://docs.rs/fuzzy-aho-corasick/latest/fuzzy_aho_corasick/structs/struct.SearchOptions.html
[`Order`]: https://docs.rs/fuzzy-aho-corasick/latest/fuzzy_aho_corasick/structs/enum.Order.html
[`Overlap`]: https://docs.rs/fuzzy-aho-corasick/latest/fuzzy_aho_corasick/structs/enum.Overlap.html
