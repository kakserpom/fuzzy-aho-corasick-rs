# Patterns & Weights

`build(..)` accepts anything convertible into a [`Pattern`]. For simple cases you pass strings; for
finer control you construct `Pattern` values with per-pattern weight, limits, and identity.

## Convenient conversions

```rust
use fuzzy_aho_corasick::FuzzyAhoCorasickBuilder;

// &str / String
let e1 = FuzzyAhoCorasickBuilder::new().build(["alpha", "beta"]);

// (pattern, weight)
let e2 = FuzzyAhoCorasickBuilder::new().build([("alpha", 2.0), ("beta", 1.0)]);

// (pattern, weight, max_edits)
let e3 = FuzzyAhoCorasickBuilder::new().build([("alpha", 1.0, 2u8), ("beta", 1.0, 1u8)]);
```

## Weights

A pattern's **weight** scales its score:

```text
similarity = (N - penalties) / N * weight
```

Default weight is `1.0`. Raising it above `1.0` boosts a pattern so it ranks ahead of others (and can
even score above `1.0`); lowering it demotes a pattern. Weights are how you say "if two patterns both
match here, prefer this one" without changing thresholds.

```rust
use fuzzy_aho_corasick::{FuzzyAhoCorasickBuilder, FuzzyLimits, Pattern};

let important = Pattern::from("error")
    .weight(2.0)                          // boost this pattern's score
    .fuzzy(FuzzyLimits::new().edits(1))   // per-pattern limits override the global default
    .custom_unique_id(42);                // stable identity for uniqueness-aware selection

let normal = Pattern::from("warning").fuzzy(FuzzyLimits::new().edits(2));

let engine = FuzzyAhoCorasickBuilder::new()
    .case_insensitive(true)
    .build([important, normal]);
```

## `Pattern` builder methods

| Method | Effect |
| --- | --- |
| `Pattern::from(&str \| String)` | Default weight `1.0`, no per-pattern limits. |
| `.weight(f32)` | Scale this pattern's similarity score. |
| `.fuzzy(FuzzyLimits)` | Per-pattern edit limits, overriding the global default. |
| `.custom_unique_id(usize)` | Stable identity used by uniqueness-aware selection. |

## Unique ids

`custom_unique_id` matters for [`search_non_overlapping_unique`](../searching/search.md): patterns
sharing an id (or, absent an id, the same pattern index) count as "the same thing", so only one match
per id is kept. This is useful when you register several spellings/aliases of one entity and want at
most one hit for it.

## `Display`

`Pattern` implements `Display`, so `m.pattern` formats as the underlying pattern string in `println!`
and friends — handy when reporting matches.

[`Pattern`]: https://docs.rs/fuzzy-aho-corasick/latest/fuzzy_aho_corasick/structs/struct.Pattern.html
