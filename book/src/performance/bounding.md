# Bounding Worst-Case Work

The core search is **exact**: it explores every viable edit path and returns the best match for each
span. Built-in pruning (edit limits, the threshold, and per-node ceilings) keeps this fast for typical
inputs. But combining a **high edit budget** with a **low threshold** can explode the state space — a
lot of insertion/deletion paths become viable while yielding few if any extra matches. Two knobs bound
that.

## Beam search — `beam_width(K)`

Whenever a search window's active frontier exceeds `2·K` states, it is sorted by penalty and truncated
to the `K` lowest-penalty candidates. This trades exactness for bounded time and memory; a larger `K`
is more accurate but slower.

```rust
use fuzzy_aho_corasick::{FuzzyAhoCorasickBuilder, FuzzyLimits};

let engine = FuzzyAhoCorasickBuilder::new()
    .fuzzy(FuzzyLimits::new().edits(4))
    .case_insensitive(true)
    .beam_width(100) // keep the 100 best candidates when the frontier grows
    .build(["saddam", "hussein", "vestibulum"]);
```

## Automatic beam — `auto_beam(budget, width)`

`auto_beam` is a safety valve rather than an always-on approximation. The search runs the **exact**
unlimited exploration until it has expanded more than `budget` states (counted across all start
positions); only then does it beam the frontier to `width` for the remainder. Ordinary searches never
approach `budget`, so they stay **exact and unaffected** — only genuine blow-ups get capped.

```rust
use fuzzy_aho_corasick::{FuzzyAhoCorasickBuilder, FuzzyLimits};

let engine = FuzzyAhoCorasickBuilder::new()
    .fuzzy(FuzzyLimits::new().edits(6))
    .case_insensitive(true)
    .auto_beam(200_000, 100) // exact under 200k states, then beam to width 100
    .build(["saddam", "hussein", "vestibulum"]);
```

An explicit `beam_width` always takes precedence over `auto_beam`.

## Which to choose

- **Neither** — the default. Correct and fast for reasonable limits/thresholds. Start here.
- **`auto_beam`** — the recommended safety net for untrusted or highly variable input: exact in the
  common case, bounded in the pathological one. Set `budget` generously (hundreds of thousands) so
  real searches never trip it.
- **`beam_width`** — when you *always* run with aggressive limits and want a hard, predictable bound
  every time, accepting that some low-ranked fuzzy matches may be missed.

If a search is unexpectedly slow, you are almost always combining a high edit budget with a low
threshold. Raising the threshold or tightening [limits](../building/builder.md) is the first fix;
`auto_beam` is the belt-and-braces one.
