# The Weakest-Link Floor

The default [scoring](../concepts/scoring.md) is *additive*: each edit contributes independently, so a
single very-dissimilar substitution can be diluted by an otherwise-excellent long match. One `sim = 0`
substitution in a 20-grapheme pattern still scores ~0.93 and would pass a 0.8 threshold.

Sometimes that's wrong: you want to say *no individual substitution may be too weak*, no matter how
good the rest of the match is. That is the "weakest link" bound from the underlying
[research](../reference/acknowledgements.md), exposed as `min_symbol_similarity`.

## Setting a floor

```rust
use fuzzy_aho_corasick::{FuzzyAhoCorasickBuilder, FuzzyLimits};

let engine = FuzzyAhoCorasickBuilder::new()
    .fuzzy(FuzzyLimits::new().edits(1))
    .case_insensitive(true)
    .min_symbol_similarity(0.3) // reject any substitution below 0.3 similarity
    .build(["vestibulum"]);

// e↔x has similarity 0 -> the substitution is rejected outright.
assert!(engine.search("vxstibulum", 0.8).is_empty());

// u↔o has similarity 0.6 -> allowed.
assert_eq!(engine.search("vestibulom", 0.8).len(), 1);
```

Any character-level substitution whose [similarity](custom.md) is below the floor is discarded
immediately, before it can contribute to a score. A candidate that would need such a substitution
simply isn't produced.

## What it applies to

- **Only character-level substitutions.** Exact matches are unaffected (similarity `1.0`), and
  explicit [mappings](mappings.md) carry their own scores and bypass the floor.
- **Independent of the threshold.** The floor is a per-symbol gate; the
  [threshold](../concepts/scoring.md) is a whole-match gate. A match must pass both.

## When to use it

Reach for the floor when a wrong-but-diluted character would be a *semantic* error, not just a lower
score — for instance in name/entity matching, where turning `Petr` into `Pxtr` should not count as a
near-match no matter how long the surrounding name is. The default is `0.0` (no floor), preserving the
purely additive behavior.
