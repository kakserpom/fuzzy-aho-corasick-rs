# Quick Start

Build an engine once, then query it as many times as you like. The engine is immutable and cheap to
share (`&FuzzyAhoCorasick`) across threads.

```rust
use fuzzy_aho_corasick::{FuzzyAhoCorasickBuilder, FuzzyLimits};

fn main() {
    // Allow up to 1 edit per match, case-insensitive.
    let engine = FuzzyAhoCorasickBuilder::new()
        .fuzzy(FuzzyLimits::new().edits(1))
        .case_insensitive(true)
        .build(["hello", "world"]);

    // "helllo wolrd" has two typos: an extra 'l' (insertion) and swapped 'lr' (transposition).
    for m in engine.search_non_overlapping("helllo wolrd", 0.8).iter() {
        println!("matched '{}' as '{}' (score {:.2})", m.pattern, m.text, m.similarity);
    }
    // matched 'hello' as 'helllo' (score 0.90)
    // matched 'world' as 'wolrd' (score 0.90)
}
```

Three things are happening here:

1. **`fuzzy(FuzzyLimits::new().edits(1))`** — without this the engine only matches exactly. `edits(1)`
   lets each match differ from its pattern by at most one edit operation.
2. **`0.8`** — the *similarity threshold*. A candidate is only returned if its score is at least this
   high. See [Scoring & Thresholds](../concepts/scoring.md).
3. **`search_non_overlapping`** — returns a ranked set of matches whose spans don't overlap. There
   are several [search entry points](../searching/search.md) for different needs.

## Inspecting a match

Each [`FuzzyMatch`] tells you what was found and how:

```rust
# use fuzzy_aho_corasick::{FuzzyAhoCorasickBuilder, FuzzyLimits};
# let engine = FuzzyAhoCorasickBuilder::new().fuzzy(FuzzyLimits::new().edits(1)).build(["needle"]);
for m in engine.search("find the neeedle", 0.8).iter() {
    println!(
        "pattern #{} ({}) matched bytes {}..{} = {:?}",
        m.pattern_index, m.pattern, m.start, m.end, m.text,
    );
    println!(
        "  score {:.2}, edits {} (ins {}, del {}, sub {}, swap {})",
        m.similarity, m.edits, m.insertions, m.deletions, m.substitutions, m.swaps,
    );
}
```

`start`/`end` are **byte** offsets into the haystack, and `text` is the matched slice.

## Next steps

- Understand the [edit model](../concepts/model.md) and [scoring](../concepts/scoring.md).
- Learn the [builder options](../building/builder.md).
- For find-and-replace, jump to [Replacement](../searching/replacement.md).

[`FuzzyMatch`]: https://docs.rs/fuzzy-aho-corasick/latest/fuzzy_aho_corasick/structs/struct.FuzzyMatch.html
