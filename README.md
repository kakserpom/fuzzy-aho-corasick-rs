# fuzzy-aho-corasick

[![CI](https://github.com/kakserpom/fuzzy-aho-corasick-rs/actions/workflows/ci.yml/badge.svg)](https://github.com/kakserpom/fuzzy-aho-corasick-rs/actions/workflows/ci.yml) [![crates.io](https://img.shields.io/crates/v/fuzzy-aho-corasick.svg)](https://crates.io/crates/fuzzy-aho-corasick) [![docs.rs](https://img.shields.io/docsrs/fuzzy-aho-corasick)](https://docs.rs/fuzzy-aho-corasick) [![MIT License](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

High-performance, Unicode-aware, safe Rust implementation of the Aho–Corasick automaton with **fuzzy matching**
(insertions, deletions, substitutions, transpositions).

## Key Features

- **Exact & Fuzzy Matching**: Match literal patterns or allow configurable approximate matching with edit operations
  (Levenshtein-style + transposition).
- **Unicode-Aware**: Operates over grapheme clusters, with optional case-insensitive matching.
- **Fine-Grained Limits**: Global or per-pattern caps on insertions, deletions, substitutions, swaps, and total edits.
- **Non-Overlapping Selection**: Choose a maximal set of non-overlapping matches with configurable heuristics.
- **Fuzzy Replacer**: Find-and-replace fuzzily while preserving surrounding context.
- **Segmentation API**: Split input into matched / unmatched segments via `segment_iter` / `segment_text`.
- **Customizable Scoring**: Weighting and penalty tuning for substitution, insertion, deletion, and swap.
- **Bounded Worst Case**: Optional beam search and an opt-in automatic beam keep pathological inputs from blowing up.

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
fuzzy-aho-corasick = "0.3"
```

Then in code:

```rust
use fuzzy_aho_corasick::{FuzzyAhoCorasickBuilder, FuzzyLimits};
```

## Quick Start

```rust
use fuzzy_aho_corasick::{FuzzyAhoCorasickBuilder, FuzzyLimits};

fn main() {
    // Build an engine allowing up to 1 edit per match, case-insensitive.
    let engine = FuzzyAhoCorasickBuilder::new()
        .fuzzy(FuzzyLimits::new().edits(1))
        .case_insensitive(true)
        .build(["hello", "world"]);

    // "H3llo W0rld!" contains two OCR-style typos ('3'→'e', '0'→'o').
    for m in engine.search_non_overlapping("H3llo W0rld!", 0.7).iter() {
        println!("matched '{}' as '{}' (score {:.2})", m.pattern, m.text, m.similarity);
    }
    // Output:
    //   matched 'hello' as 'H3llo' (score 0.71)
    //   matched 'world' as 'W0rld' (score 0.89)
    //
    // Note: 'W0rld' scores higher because the default similarity table treats '0'↔'o'
    // as a near-match, whereas '3'↔'e' is unrelated and costs a full substitution.
}
```

### How scoring works

For a candidate match against a pattern of length `N` graphemes with accumulated edit `penalties`, the similarity is:

```text
similarity = (N - penalties) / N * weight
```

Each edit adds a penalty scaled by how dissimilar the graphemes are (see [`FuzzyPenalties`] and the similarity table).
A match is kept only if `similarity >= threshold`. With the default `weight` of `1.0`, similarity is in `0.0..=1.0`;
pattern weights above `1.0` can push a match's score above `1.0` to prioritize it.

## Builder API

Customize fuzzy behavior, penalties, and case folding:

```rust
use fuzzy_aho_corasick::{FuzzyAhoCorasickBuilder, FuzzyLimits, FuzzyPenalties};

let engine = FuzzyAhoCorasickBuilder::new()
    .fuzzy(FuzzyLimits::new().edits(2)) // up to 2 total edits per pattern by default
    .penalties(
        FuzzyPenalties::default()
            .substitution(0.7)
            .insertion(0.9)
            .deletion(0.9)
            .swap(1.0),
    )
    .case_insensitive(true)
    .build(["pattern1", "pattern2"]);
```

Builder options:

| Method | Purpose |
| --- | --- |
| `fuzzy(FuzzyLimits)` | Global default edit limits for every pattern. |
| `penalties(FuzzyPenalties)` | Cost of each edit type; shapes which edits are “cheaper”. |
| `case_insensitive(bool)` | Unicode-aware case folding. |
| `beam_width(usize)` | Cap the active frontier to the K lowest-penalty states (approximate; faster). |
| `auto_beam(budget, width)` | Stay exact until `budget` states are explored, then beam to `width` (see below). |
| `similarity(&'static Similarity)` | Provide a custom grapheme similarity table (see [Custom Similarity](#custom-similarity)). |
| `minimize(f32)` | Merge near-equivalent automaton states within a tolerance to shrink the automaton. |
| `build(patterns)` | Build the immutable engine. |
| `build_replacer(pairs)` | Build a [`FuzzyReplacer`] from `(pattern, replacement)` pairs. |

## Pattern Weights & Direct Pattern Construction

By default, all patterns have weight `1.0`, but you can adjust per-pattern scoring and fuzzy limits directly via
`Pattern` before building the automaton. You can also pass tuples or fully constructed `Pattern` values to `build(...)`.

* `Pattern::from(&str | String)`: build a pattern with default weight and no per-pattern limits.
* `Pattern::weight(f32)`: set the pattern's weight (default `1.0`), scaling its similarity score.
* `Pattern::fuzzy(FuzzyLimits)`: apply per-pattern edit limits (override the global default).
* `Pattern::custom_unique_id(usize)`: give a stable identity for uniqueness-aware matching
  (`non_overlapping_unique`).

`build(...)` accepts anything convertible into `Pattern`, including:

* `&str` / `String`
* `(&str, f32)` — `(pattern, weight)`
* `(&str, f32, u8)` — `(pattern, weight, max_edits)`
* a fully built `Pattern`

### Examples

Weighting via tuples, and case-insensitive Greek:

```rust
use fuzzy_aho_corasick::FuzzyAhoCorasickBuilder;

// Each tuple is (pattern, weight).
let engine = FuzzyAhoCorasickBuilder::new()
    .case_insensitive(true)
    .build([("Γειά", 1.0), ("σου", 1.0)]);
assert!(!engine.search("γειά ΣΟΥ!", 0.8).is_empty());
```

Customizing patterns with per-pattern fuzzy limits, weights, and unique IDs:

```rust
use fuzzy_aho_corasick::{FuzzyAhoCorasickBuilder, FuzzyLimits, Pattern};

let important = Pattern::from("error")
    .weight(2.0) // boost this pattern's score
    .fuzzy(FuzzyLimits::new().edits(1))
    .custom_unique_id(42);

let normal = Pattern::from("warning").fuzzy(FuzzyLimits::new().edits(2));

let engine = FuzzyAhoCorasickBuilder::new()
    .case_insensitive(true)
    .build([important, normal]);
```

These allow fine-grained control over ranking, deduplication, and fuzzy tolerance on a per-pattern basis.

## Match Selection Strategies

`search_unsorted` returns the raw best match per span; the following orderings and overlap resolvers refine it. They
are methods on the returned [`FuzzyMatches`]:

* `default_sort()`: Prioritizes higher similarity, then longer patterns, then earlier position.
* `greedy_sort()`: Prefers longer patterns first, then similarity.
* `coverage_weighted_sort()`: Ranks by `similarity * covered_length`, favoring matches that cover more text.
* `non_overlapping()`: Greedily drops overlapping matches in the current order.
* `non_overlapping_unique()`: Same, but ensures each pattern (respecting `custom_unique_id`) is used at most once.

Convenience entrypoints on the engine (each takes `(haystack, threshold)`):

* `search(...)`: default sort.
* `search_greedy(...)`: greedy sort.
* `search_coverage_weighted(...)`: coverage-weighted sort.
* `search_non_overlapping(...)`: default sort + non-overlapping selection.
* `search_non_overlapping_unique(...)`: default sort + non-overlapping + pattern-unique.
* `search_non_overlapping_unique_coverage_weighted(...)`: coverage-weighted variant of the above.
* `search_unsorted(...)`: raw, unsorted best-per-span matches (build your own pipeline).

## Bounding Worst-Case Work

The core search is exact: it explores every viable edit path and returns the best match for each span. For most inputs
the built-in pruning keeps this fast, but combining a **high edit budget** with a **low similarity threshold** can
explode the state space (while usually yielding no additional matches). Two knobs bound this.

### Beam search — `beam_width(K)`

Whenever a search window's active frontier exceeds `2·K` states, it is sorted by penalty and truncated to the `K`
lowest-penalty candidates. This trades exactness for speed and bounded memory; a larger `K` is more accurate.

```rust
use fuzzy_aho_corasick::{FuzzyAhoCorasickBuilder, FuzzyLimits};

let engine = FuzzyAhoCorasickBuilder::new()
    .fuzzy(FuzzyLimits::new().edits(4))
    .case_insensitive(true)
    .beam_width(100) // keep the 100 best candidates when the frontier grows
    .build(["saddam", "hussein", "vestibulum"]);
```

### Automatic beam — `auto_beam(budget, width)`

`auto_beam` is a safety valve for pathological inputs. The search runs the **exact** unlimited exploration until it has
expanded more than `budget` states (counted across all start positions); only then does it beam the frontier to `width`
for the remainder. Ordinary searches never approach `budget`, so they stay **exact and unaffected**; only genuine
blow-ups get capped.

```rust
use fuzzy_aho_corasick::{FuzzyAhoCorasickBuilder, FuzzyLimits};

let engine = FuzzyAhoCorasickBuilder::new()
    .fuzzy(FuzzyLimits::new().edits(6))
    .case_insensitive(true)
    .auto_beam(200_000, 100) // exact under 200k states, then beam to width 100
    .build(["saddam", "hussein", "vestibulum"]);
```

An explicit `beam_width` always takes precedence over `auto_beam`.

## Segmentation and Reconstruction

Break text into matched/unmatched pieces and reassemble with intelligent spacing:

```rust
use fuzzy_aho_corasick::{FuzzyAhoCorasickBuilder, FuzzyLimits};

let engine = FuzzyAhoCorasickBuilder::new()
    .fuzzy(FuzzyLimits::new().edits(1))
    .build(["input", "more"]);
let matches = engine.search_non_overlapping("someinptandm0re", 0.75);
assert_eq!(matches.segment_text(), "some inpt and m0re");
```

### Splitting on Fuzzy Matches

Treat each fuzzy match as a delimiter and collect the unmatched pieces:

- **`FuzzyMatches::split()`** — splits the already-segmented stream, yielding the `Unmatched` parts (including empty
  ones if matches touch the ends).
- **`FuzzyAhoCorasick::split(haystack, threshold)`** — convenience: runs `search_non_overlapping` and calls `split()`.

```rust
use fuzzy_aho_corasick::{FuzzyAhoCorasickBuilder, FuzzyLimits};

let engine = FuzzyAhoCorasickBuilder::new()
    .fuzzy(FuzzyLimits::new().edits(1))
    .case_insensitive(true)
    .build(["FOO", "BAR"]);

// Treat each fuzzy match (>= 0.8) as a separator:
let parts: Vec<&str> = engine.split("xxFo0yyBAARzz", 0.8).collect();
assert_eq!(parts, vec!["xx", "yy", "zz"]);
```

## Post-Processing Utilities

Once you have a [`FuzzyMatches`] (for example, from `search_non_overlapping`), these methods trim, transform, or extract
from the matched set:

* **`replace(callback)`** — walks matches left-to-right (skipping overlaps) and calls
  `Fn(&FuzzyMatch) -> Option<S>` where `S: Into<Cow<str>>` (e.g. `&str` or `String`). `Some(repl)` replaces the span;
  `None` keeps the original text. See [Fuzzy Replacer](#fuzzy-replacer) for the turnkey version.
* **`strip_prefix()`** — drops leading fuzzy-matched (and whitespace-only) segments, trims the first kept segment, and
  returns the rest.
* **`strip_postfix()`** — the mirror image: removes trailing matched/whitespace segments and returns the leading text.
* **`matched_spans()` / `matched_strings()`** — the `(start, end)` byte ranges / the matched substrings.
* **`filter(pred)` / `retain(pred)`** — keep only matches satisfying a predicate (borrowing / in place).
* **`iter()` / `iter_mut()` / `len()` / `is_empty()`** — inspect the match set (also available via `Deref<[FuzzyMatch]>`).

### Fuzzy Replacer

Perform fuzzy find-and-replace with a mapping. Non-overlapping matches are chosen automatically using the default
heuristics.

```rust
use fuzzy_aho_corasick::{FuzzyAhoCorasickBuilder, FuzzyLimits};

let replacer = FuzzyAhoCorasickBuilder::new()
    .case_insensitive(true)
    .fuzzy(FuzzyLimits::new().edits(1))
    .build_replacer([("hello", "hi"), ("world", "earth")]);

// '0'↔'o' is a near-match in the default table, so both fuzzy tokens are replaced:
assert_eq!(replacer.replace("hell0 w0rld!", 0.8), "hi earth!");
```

## Custom Similarity

By default, substitutions between related graphemes (vowels, consonants, common OCR confusions like `0`↔`o`, `1`↔`l`)
carry a reduced penalty. Provide your own table for domain-specific confusions:

```rust
use fuzzy_aho_corasick::{FuzzyAhoCorasickBuilder, structs::{Similarity, FxHashMap}};
use std::sync::LazyLock;

static SIMILARITY: LazyLock<Similarity> = LazyLock::new(|| {
    let mut map = FxHashMap::default();
    map.insert(('@', 'a'), 0.9);
    map.insert(('a', '@'), 0.9);
    Similarity::from_map(map)
});

let engine = FuzzyAhoCorasickBuilder::new()
    .similarity(&SIMILARITY)
    .build(["cat"]);
```

## Performance

The engine is built once and is cheap to query repeatedly. Some tips:

* **Filter early with the similarity threshold** — a higher threshold prunes low-quality candidates before they expand.
* **Tune `FuzzyLimits` per pattern** when you know the expected error characteristics; tighter limits explore fewer
  states.
* **Shape ambiguity with `FuzzyPenalties`** — make substitutions or insertions cheaper/pricier to fit your domain.
* **Prefer `search_non_overlapping_*`** to avoid resolving overlaps yourself.
* **Guard against pathological inputs** with `beam_width` or `auto_beam` when edit limits are high and thresholds low.

Grapheme positions are represented as `u32` internally, so a single haystack is expected to be well under 4 GiB.

## Troubleshooting

* **Too many false positives** — raise the similarity threshold or tighten per-pattern limits.
* **Missing fuzzy matches** — lower the threshold, increase allowed edits, or make substitutions less punitive via
  penalties. If you enabled a beam, increase its width.
* **Overlapping matches unwanted** — use `search_non_overlapping` / `search_non_overlapping_unique` rather than raw
  `search_unsorted`.
* **A search is unexpectedly slow** — you are likely combining a high edit budget with a low threshold; add
  `auto_beam` (keeps common cases exact) or an explicit `beam_width`.

## Testing

Run the test suite with:

```sh
cargo test
```

Benchmarks (Criterion):

```sh
cargo bench
```

## License

Distributed under the **MIT License**. See `LICENSE` for details.

## Acknowledgements

Based on a research paper — [**Fuzzified Aho–Corasick Search Automata**](DOCS/ias10_horak.pdf) by Z. Horák, V. Snášel,
A. Abraham, and A. E. Hassanien.

[`FuzzyMatches`]: https://docs.rs/fuzzy-aho-corasick/latest/fuzzy_aho_corasick/struct.FuzzyMatches.html
[`FuzzyReplacer`]: https://docs.rs/fuzzy-aho-corasick/latest/fuzzy_aho_corasick/struct.FuzzyReplacer.html
[`FuzzyPenalties`]: https://docs.rs/fuzzy-aho-corasick/latest/fuzzy_aho_corasick/struct.FuzzyPenalties.html
