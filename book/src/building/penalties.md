# Penalties

[`FuzzyPenalties`] sets the cost each edit operation adds to a candidate's total penalty, which in
turn drives the [score](../concepts/scoring.md). Shaping these costs lets you express what kinds of
error are likely in your domain.

```rust
use fuzzy_aho_corasick::{FuzzyAhoCorasickBuilder, FuzzyLimits, FuzzyPenalties};

let engine = FuzzyAhoCorasickBuilder::new()
    .fuzzy(FuzzyLimits::new().edits(2))
    .penalties(
        FuzzyPenalties::default()
            .substitution(0.7)
            .insertion(0.9)
            .deletion(0.9)
            .swap(1.0),
    )
    .build(["pattern"]);
```

## The four costs

| Field | Applies to | Notes |
| --- | --- | --- |
| `substitution` | replacing one symbol with another | **scaled by similarity**: the added penalty is `substitution * (1 - sim)`, so a near-miss costs little and an exact match costs nothing. |
| `insertion` | an extra symbol in the text | flat cost. |
| `deletion` | a missing pattern symbol | flat cost. |
| `swap` | transposing two adjacent symbols | flat cost, counted as a single operation. |

## Defaults

The defaults are tuned so that a substitution is the most expensive edit, insertions the cheapest,
with deletions and swaps in between (roughly `substitution ≈ 1.43`, `deletion ≈ 0.91`,
`insertion ≈ 0.52`, `swap ≈ 0.52`). This reflects that an inserted or transposed character usually
preserves more of the intended word than an outright wrong character does.

You rarely need to change these, but doing so is the right tool when you know your errors: for OCR,
substitutions between look-alike glyphs should be cheap (do that via the
[similarity table](../similarity/custom.md) rather than the flat substitution cost); for
speech-to-text, insertions/deletions of small words might dominate.

## How penalties become a score

The costs accumulate over the edits in a candidate, then feed the score:

```text
similarity = (N - Σ penalties) / N * weight
```

Because the substitution cost is multiplied by `(1 - sim)`, two symbols the
[similarity table](../similarity/custom.md) rates as 0.7-similar incur only 30% of the full
substitution penalty. That interplay — flat costs for insert/delete/swap, similarity-scaled cost for
substitution — is what lets the engine treat `0`↔`o` as a near-match while treating an unrelated
substitution as a real error.

> **Tip:** penalties and the [threshold](../concepts/scoring.md) work together. If you find yourself
> pushing a penalty very high just to exclude a certain match, consider whether an
> [edit limit](builder.md) or the [weakest-link floor](../similarity/floor.md) expresses your intent
> more directly.

[`FuzzyPenalties`]: https://docs.rs/fuzzy-aho-corasick/latest/fuzzy_aho_corasick/structs/struct.FuzzyPenalties.html
