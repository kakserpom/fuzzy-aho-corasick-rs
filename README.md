# fuzzy-aho-corasick

[![crates.io](https://img.shields.io/crates/v/fuzzy-aho-corasick.svg)](https://crates.io/crates/fuzzy-aho-corasick) [![docs.rs](https://img.shields.io/docsrs/fuzzy-aho-corasick)](https://docs.rs/fuzzy-aho-corasick) [![MIT License](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

High-performance, Unicode-aware safe Rust implementation of the Aho–Corasick automaton with **fuzzy matching** (
insertions, deletions, substitutions, transpositions).

## Key Features

- **Exact & Fuzzy Matching**: Match literal patterns or allow configurable approximate matching with edit operations (
  Levenshtein-style + transposition).
- **Unicode-Aware**: Operates over grapheme clusters, with optional case-insensitive matching.
- **Fine-Grained Limits**: Per-pattern caps on insertions, deletions, substitutions, swaps, and total edits.
- **Non-Overlapping Selection**: Choose a maximal set of non-overlapping matches with configurable heuristics.
- **Fuzzy Replacer**: Find-and-replace fuzzily while preserving surrounding context.
- **Segmentation API**: Split input into matched / unmatched segments via `segment_iter` / `segment_text`.
- **Customizable Scoring**: Weighting and penalty tuning for substitution, insertion, deletion, swap.

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
fuzzy-aho-corasick = "0.3"
````

Then in code:

```rust
use fuzzy_aho_corasick::{FuzzyAhoCorasickBuilder, FuzzyLimits};
```

## Quick Start

```rust
use fuzzy_aho_corasick::{FuzzyAhoCorasickBuilder, FuzzyLimits};

fn main() {
    // Build engine allowing up to 1 edit per match, case-insensitive.
    let engine = FuzzyAhoCorasickBuilder::new()
        .fuzzy(FuzzyLimits::new().edits(1))
        .case_insensitive(true)
        .build(["hello", "world"]);

    let text = "H3llo W0rld!";
    let matches = engine.search_non_overlapping(text, 0.8);

    for m in matches.iter() {
        println!(
            "matched pattern '{}' as '{}' (score {:.2})",
            m.pattern, m.text, m.similarity
        );
    }
    // Expected output (approximate):
    // matched pattern 'hello' as 'H3llo' (score 0.90)
}
```

## Builder API

Customize fuzzy behavior, penalties, and case folding:

```rust
use fuzzy_aho_corasick::{FuzzyAhoCorasickBuilder, FuzzyLimits, FuzzyPenalties};
fn engine() {
    FuzzyAhoCorasickBuilder::new()
        .fuzzy(FuzzyLimits::new().edits(2)) // up to 2 total edits per pattern by default
        .penalties(
            FuzzyPenalties::default()
                .substitution(0.7)
                .insertion(0.9)
                .deletion(0.9)
                .swap(1.0),
        )
        .case_insensitive(true)
        .build(["pattern1", "pattern2"])
}
```

## Pattern Weights & Direct Pattern Construction

By default, all patterns have weight `1.0`, but you can adjust per-pattern scoring and fuzzy limits directly via
`Pattern` before building the automaton. You can also pass tuples or fully constructed `Pattern` instances to
`build(...)`.

* `Pattern::weight(f32)`: set the pattern’s weight (default `1.0`), affecting its effective similarity score.
* `Pattern::fuzzy(FuzzyLimits)`: apply per-pattern edit limits.
* `Pattern::custom_unique_id(usize)`: give a stable identity to the pattern for uniqueness-aware matching (
  `non_overlapping_unique`).

### Examples

Weighting (e.g., give importance to certain patterns) and case-insensitive Greek:

```rust
fn main() {
    use fuzzy_aho_corasick::{FuzzyAhoCorasickBuilder, Pattern};

    // Build from tuple list; each tuple is (pattern, weight).
    let engine = FuzzyAhoCorasickBuilder::new()
        .case_insensitive(true)
        .build([("Γειά", 1.0), ("σου", 1.0)]);
}
````

Customizing patterns with fuzzy limits, unique IDs, and weights:

```rust
fn main() {
    use fuzzy_aho_corasick::{FuzzyAhoCorasickBuilder, Pattern, FuzzyLimits};

    let p1 = Pattern::new("error")
        .weight(2.0) // boost this pattern
        .fuzzy(FuzzyLimits::new().edits(1))
        .custom_unique_id(42);

    let p2 = Pattern::new("warning")
        .weight(1.0)
        .fuzzy(FuzzyLimits::new().edits(2));

    // Build engine from explicit Pattern objects.
    let engine = FuzzyAhoCorasickBuilder::new()
        .build([p1, p2]);
}
```

These allow fine-grained control over ranking, deduplication, and fuzzy tolerance on a per-pattern basis.

## Match Selection Strategies

There are helpers to control ordering and overlap resolution:

* `default_sort()`: Prioritizes higher similarity, longer patterns, then earlier position.
* `greedy_sort()`: Prefers longer patterns first, then similarity.
* `non_overlapping()`: Drops overlapping matches greedily in the current order.
* `non_overlapping_unique()`: Same as above but ensures each pattern (respecting `custom_unique_id`) is used at most
  once.

Convenience entrypoints:

* `search(...)`: Applies default sort and returns non-overlapping matches.
* `search_greedy(...)`: Applies greedy sort.
* `search_non_overlapping(...)` / `search_non_overlapping_unique(...)`: Variants that combine sorting + deduplication
  semantics.

## Segmentation and Reconstruction

Break text into matched/unmatched pieces and reassemble with intelligent spacing:

```rust
fn main() {
    let engine = FuzzyAhoCorasickBuilder::new()
        .fuzzy(FuzzyLimits::new().edits(1))
        .build(["input", "more"]);
    let matches = engine.search_non_overlapping("someinptandm0re", 0.75);
    let segmented_text = matches.segment_text();
    assert_eq!(segmented_text, "some inpt and m0re");
}
```

### Splitting on Fuzzy Matches

You can treat each fuzzy match as a delimiter and collect the unmatched pieces:

- **`FuzzyMatches::split()`**  
  Splits the already‐segmented stream, returning a `Vec<String>` of all `Unmatched` parts (including empty ones if
  matches touch the ends).

- **`FuzzyAhoCorasick::split(haystack, threshold)`**  
  Convenience: runs `search_non_overlapping(haystack, threshold)` and immediately calls `split()` on the result.

**Example**

```rust
use fuzzy_aho_corasick::{FuzzyAhoCorasickBuilder, FuzzyLimits};
fn main() {
    let engine = FuzzyAhoCorasickBuilder::new()
        .fuzzy(FuzzyLimits::new().edits(1))
        .case_insensitive(true)
        .build(["FOO", "BAR"]);

    // Treat each fuzzy match (≥0.8) as a separator:
    let parts = engine.split("xxFOOyyBARzz", 0.8);
    assert_eq!(parts, vec!["xx", "yy", "zz"]);
}
```

### Post-Processing Utilities

Once you have a `FuzzyMatches` (for example, from `search_non_overlapping`), these handy methods let you trim or
transform the matched segments:

* **`replace(callback)`**
  Walks through matches left-to-right (skipping overlaps) and invokes your `Fn(&FuzzyMatch) -> Option<&str>` for each
  one; if the callback returns `Some(repl)`, that span is replaced with `repl`, otherwise the original matched text is
  preserved. See **Fuzzy Replacer** section for a basic replacer.

* **`strip_prefix()`**
  Drops all leading fuzzy-matched segments and any whitespace-only unmatched segments, then trims leading spaces from
  the first kept unmatched segment and returns the rest of the text.

* **`strip_postfix()`**
  Removes all trailing fuzzy-matched segments and any whitespace-only unmatched segments, then trims trailing spaces
  from the last kept unmatched segment and returns the leading portion of the text.

#### Fuzzy Replacer

Perform fuzzy find-and-replace with a mapping. Non-overlapping matches are chosen automatically based on default
heuristics.

```rust
fn main() {
    let replacer = FuzzyAhoCorasickBuilder::new().build_replacer([
        ("foo", "bar"),
        ("baz", "qux"),
    ]);

    let out = replacer.replace("F00 and BAZ!", 0.8);
    assert_eq!(out, "bar and qux!");
}
```

## Performance Tips

* Prefer filtering by similarity threshold early to prune low-quality candidates.
* Tune `FuzzyLimits` per pattern when you know expected error characteristics.
* Custom `FuzzyPenalties` can shape whether substitutions or insertions are “cheaper” in ambiguous regions.
* Use `search_non_overlapping_*` variants to avoid post-processing overlapping match resolution.

## Troubleshooting

* **Too many false positives**: Raise the similarity threshold or tighten per-pattern limits.
* **Missing fuzzy matches**: Lower the threshold, increase allowed edits, or adjust penalties to make substitutions less
  punitive.
* **Overlapping matches unwanted**: Use `search_non_overlapping` or `search_non_overlapping_unique` instead of raw
  `search_unsorted`.

## Examples

See `examples/` (if present) for real-world usage patterns:

* Fuzzy deduplication pipelines
* Fuzzy find-and-replace in human-entered text
* Entity extraction with per-entity error tolerance

## Testing

Run the test suite with:

```sh
cargo test
```

## License

Distributed under the **MIT License**. See `LICENSE` for details.

## Acknowledgements

Based on a research paper — [**Fuzzified Aho–Corasick Search Automata**](DOCS/ias10_horak.pdf) by Z. Horák, V. Snášel,
A. Abraham, and A. E. Hassanien.


