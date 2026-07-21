# Multi-Character Mappings

The [similarity table](custom.md) maps single graphemes to single graphemes. For equivalences that
span **several graphemes** — ligatures and transliterations like `æ`↔`ae`, `ß`↔`ss`, `ks`↔`x` —
register a **mapping**.

```rust
use fuzzy_aho_corasick::{FuzzyAhoCorasickBuilder, FuzzyLimits};

let engine = FuzzyAhoCorasickBuilder::new()
    .case_insensitive(true)
    .fuzzy(FuzzyLimits::new().edits(1))
    .mapping("æ", "ae")               // exact equivalence (score 1.0, penalty-free)
    .mapping("ks", "x")
    .mapping_scored("ph", "f", 0.9)   // near-equivalence carrying a small penalty
    .build(["encyclopaedia", "alexander"]);

// 'æ' in the haystack matches "ae" in the pattern (and vice versa):
assert_eq!(engine.search("encyclopædia", 0.95).len(), 1);
// 'x' in the pattern matches "ks" in the haystack:
assert_eq!(engine.search("aleksander", 0.95).len(), 1);
```

## Semantics

- **Bidirectional.** `mapping("æ", "ae")` lets either side stand in for the other, in the pattern or
  the haystack.
- **Counts as one substitution.** A mapping is a single substitution against the
  [edit limits](../building/builder.md), regardless of how many graphemes each side has. With
  `edits(0)` even a free mapping like `æ`↔`ae` is rejected, exactly like an ordinary substitution.
- **Scored.** [`mapping(a, b)`] is an exact equivalence (score `1.0`, no penalty).
  [`mapping_scored(a, b, s)`] is a near-equivalence; the applied penalty is `substitution * (1 - s)`,
  just like a similarity-scaled substitution.
- **Case-folded like patterns.** Both sides are grapheme-split and case-folded the same way as
  patterns, so they line up with the folded haystack at search time.

## Cost and when to use it

Mappings are precomputed at build time and stored out-of-line, so configuring **none** leaves the
search hot path completely unchanged — you pay nothing for the feature unless you use it.

Use mappings for script- and orthography-level equivalences that a single-symbol table can't express:
German `ß`↔`ss`, Nordic `æ`/`ø`/`å` transliterations, Cyrillic↔Latin name variants, or domain
shorthands. For plain look-alike single characters (`0`↔`o`), the [similarity table](custom.md) is
the lighter-weight tool.

> **Note:** mappings are one of the features the [bit-parallel pre-filter](../performance/prefilter.md)
> cannot model, so an engine configured with mappings falls back to the full search when pre-filtered.
> Correctness is unaffected; only the pre-filter speedup is forgone.

[`mapping(a, b)`]: https://docs.rs/fuzzy-aho-corasick/latest/fuzzy_aho_corasick/struct.FuzzyAhoCorasickBuilder.html#method.mapping
[`mapping_scored(a, b, s)`]: https://docs.rs/fuzzy-aho-corasick/latest/fuzzy_aho_corasick/struct.FuzzyAhoCorasickBuilder.html#method.mapping_scored
