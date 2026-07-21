# The Bit-Parallel Pre-Filter

The core search is thorough but pays a per-position cost. When you search large inputs that are mostly
non-matching text, `with_prefilter()` adds an opt-in fast lane: a bit-parallel
([Bitap](https://en.wikipedia.org/wiki/Bitap_algorithm) / Wu–Manber) approximate scan runs first, at
hundreds of MB/s, to locate *candidate regions*, and the full weighted engine then re-searches only
those regions.

Results are **identical** to [`search`](../searching/search.md) / `search_unsorted` — the filter is a
conservative over-approximation (a necessary condition), so it never drops a real match; it only
spares the engine from scanning text that cannot contain one.

```rust
use fuzzy_aho_corasick::{FuzzyAhoCorasickBuilder, FuzzyLimits};

let engine = FuzzyAhoCorasickBuilder::new()
    .fuzzy(FuzzyLimits::new().edits(1))
    .build(["vestibulum", "consectetur"]);

let pf = engine.with_prefilter(); // build once, reuse across searches
let hits = pf.search("… lorem vestibulm ipsum …", 0.85);
// Same matches as engine.search(…), just faster on large, sparse inputs.
```

## Why it's sound

A match the engine accepts has a bounded number of edits: the [threshold](../concepts/scoring.md) caps
the total penalty a kept match may carry (`P_max = N·(1 − θ/weight)`), and each edit costs at least
some minimum penalty. That bound becomes the bit-parallel scan's Levenshtein budget `k` (a
transposition counts as two unit edits), so every match the full engine could accept is guaranteed to
survive the filter.

## Graceful fallback

When the configuration can't be reduced to the bit model, the wrapper transparently runs the full
search instead — always correct, merely without the speedup. That happens when:

- [multi-character mappings](../similarity/mappings.md) are configured (block edits don't map to unit
  Levenshtein),
- a pattern is longer than 63 graphemes,
- a penalty is so low that an edit is effectively free (the budget becomes unbounded), or
- the derived budget is too large to stay selective.

`pf.is_active()` reports whether a usable filter was built.

## When it helps

The win scales inversely with match density: on sparse inputs the engine sees only a small fraction
of the text (an ~13× end-to-end speedup on a 16 MiB sample after transcode optimizations), while
match-saturated inputs gain little — a wasted scan, then roughly baseline. It degrades gracefully
rather than ever going wrong.

See [`examples/bitap_prototype.rs`](https://github.com/kakserpom/fuzzy-aho-corasick-rs/blob/master/examples/bitap_prototype.rs)
for the standalone algorithm, a brute-force correctness verifier, and a throughput comparison.
