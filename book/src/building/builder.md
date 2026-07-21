# Builder & Edit Limits

You configure everything through [`FuzzyAhoCorasickBuilder`], then call `build(patterns)` to get an
immutable [`FuzzyAhoCorasick`].

```rust
use fuzzy_aho_corasick::{FuzzyAhoCorasickBuilder, FuzzyLimits, FuzzyPenalties};

let engine = FuzzyAhoCorasickBuilder::new()
    .fuzzy(FuzzyLimits::new().edits(2))          // global edit limits
    .penalties(FuzzyPenalties::default().substitution(0.7))
    .case_insensitive(true)
    .build(["pattern1", "pattern2"]);
```

## Builder options

| Method | Purpose |
| --- | --- |
| `fuzzy(FuzzyLimits)` | Global default edit limits for every pattern. |
| `penalties(FuzzyPenalties)` | Cost of each edit type. See [Penalties](penalties.md). |
| `case_insensitive(bool)` | Unicode-aware case folding. |
| `similarity(&'static Similarity)` | Custom symbol similarity table. See [Custom Similarity](../similarity/custom.md). |
| `min_symbol_similarity(f32)` | Reject substitutions below a per-symbol floor. See [Weakest-Link Floor](../similarity/floor.md). |
| `mapping(a, b)` / `mapping_scored(a, b, s)` | Multi-character equivalences. See [Mappings](../similarity/mappings.md). |
| `beam_width(usize)` | Cap the active frontier (approximate; faster). See [Bounding](../performance/bounding.md). |
| `auto_beam(budget, width)` | Stay exact until a state budget, then beam. See [Bounding](../performance/bounding.md). |
| `build(patterns)` | Build the immutable engine. |
| `build_replacer(pairs)` | Build a [`FuzzyReplacer`] from `(pattern, replacement)` pairs. |

`build` accepts anything convertible into a [`Pattern`] — `&str`, `String`, `(&str, weight)`,
`(&str, weight, max_edits)`, or a fully built `Pattern`. See [Patterns & Weights](patterns.md).

## Edit limits with `FuzzyLimits`

[`FuzzyLimits`] caps how many edits a match may contain. You can cap the total and/or each type
individually:

```rust
use fuzzy_aho_corasick::FuzzyLimits;

FuzzyLimits::new().edits(2);                       // at most 2 edits, any mix
FuzzyLimits::new().substitutions(1).deletions(1);  // 1 substitution AND 1 deletion, no others
FuzzyLimits::new().edits(3).swaps(1);              // up to 3 edits total, at most 1 of them a swap
```

The semantics:

- **`edits(n)`** caps the *total* number of edits. When set alone, each individual edit type is left
  unbounded (bounded only by the total).
- **`insertions(n)` / `deletions(n)` / `substitutions(n)` / `swaps(n)`** cap that specific type.
- If you set *only* per-type limits (no `edits`), the unset types default to `0` — i.e. they are
  forbidden. This lets you say "substitutions only" with `FuzzyLimits::new().substitutions(2)`.
- With **no `fuzzy(..)` at all**, the engine is exact: zero edits of every kind.

Limits are a hard filter applied before the threshold, and they bound the worst-case search space —
tighter limits explore fewer states. A candidate that would exceed any applicable limit is never
produced.

## Global vs. per-pattern limits

`fuzzy(..)` on the builder sets the **global** default. Individual patterns can override it with
their own limits (see [Patterns & Weights](patterns.md)); a pattern's own limits take precedence over
the global default for that pattern.

[`FuzzyAhoCorasickBuilder`]: https://docs.rs/fuzzy-aho-corasick/latest/fuzzy_aho_corasick/struct.FuzzyAhoCorasickBuilder.html
[`FuzzyAhoCorasick`]: https://docs.rs/fuzzy-aho-corasick/latest/fuzzy_aho_corasick/struct.FuzzyAhoCorasick.html
[`FuzzyLimits`]: https://docs.rs/fuzzy-aho-corasick/latest/fuzzy_aho_corasick/struct.FuzzyLimits.html
[`FuzzyReplacer`]: https://docs.rs/fuzzy-aho-corasick/latest/fuzzy_aho_corasick/struct.FuzzyReplacer.html
[`Pattern`]: https://docs.rs/fuzzy-aho-corasick/latest/fuzzy_aho_corasick/structs/struct.Pattern.html
