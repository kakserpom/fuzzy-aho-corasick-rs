# Search & Selection

A search returns a [`FuzzyMatches`] collection — the matches found at or above the threshold. The
different entry points differ only in how the results are **ordered** and whether **overlaps** are
resolved.

## Entry points

Each takes `(haystack, threshold)`:

| Method | Ordering | Overlaps |
| --- | --- | --- |
| `search_unsorted` | none (raw best-per-span) | kept |
| `search` | default sort | kept |
| `search_greedy` | greedy (longer patterns first) | kept |
| `search_coverage_weighted` | by `similarity × covered length` | kept |
| `search_non_overlapping` | default sort | resolved |
| `search_non_overlapping_unique` | default sort | resolved + one match per pattern id |
| `search_non_overlapping_unique_coverage_weighted` | coverage-weighted | resolved + unique |

`search_unsorted` is the primitive: it returns the single best-scoring match for each distinct
`(start, end, pattern)` span, in no particular order. Everything else is `search_unsorted` plus a
sort and/or an overlap resolver, so you can also build your own pipeline.

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
use fuzzy_aho_corasick::{FuzzyAhoCorasickBuilder, FuzzyLimits};

let engine = FuzzyAhoCorasickBuilder::new()
    .fuzzy(FuzzyLimits::new().edits(1))
    .case_insensitive(true)
    .build(["hello", "world"]);

let matches = engine.search_non_overlapping("helllo wolrd", 0.8);
let found: Vec<&str> = matches.iter().map(|m| m.pattern.as_str()).collect();
assert!(found.contains(&"hello") && found.contains(&"world"));
```

## Working with the results

[`FuzzyMatches`] derefs to `&[FuzzyMatch]` and supports `iter()`, `iter_mut()`, `len()`,
`is_empty()`, and `IntoIterator`. It also offers post-processing helpers:

- `filter(pred)` / `retain(pred)` — keep matches satisfying a predicate.
- `matched_spans()` / `matched_strings()` — the `(start, end)` byte ranges / matched substrings.
- `replace(callback)` — see [Replacement](replacement.md).
- `segment_iter()`, `split()`, `strip_prefix()`, `strip_postfix()` — see
  [Segmentation & Splitting](segmentation.md).

Each [`FuzzyMatch`] carries `pattern_index`, `pattern`, `start`/`end` (byte offsets), `text`,
`similarity`, and the per-type edit counts (`insertions`, `deletions`, `substitutions`, `swaps`,
`edits`).

[`FuzzyMatches`]: https://docs.rs/fuzzy-aho-corasick/latest/fuzzy_aho_corasick/structs/struct.FuzzyMatches.html
[`FuzzyMatch`]: https://docs.rs/fuzzy-aho-corasick/latest/fuzzy_aho_corasick/structs/struct.FuzzyMatch.html
