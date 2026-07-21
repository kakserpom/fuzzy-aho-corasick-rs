# Custom Similarity Tables

Substitutions are scored by a **similarity table**: for each ordered pair of symbols it gives a
similarity in `0.0..=1.0`, and the substitution penalty is `substitution_cost * (1 - sim)`. Identical
symbols are `1.0` (no penalty); unrelated symbols default to `0.0` (full penalty).

## The default table

Out of the box the engine ships a general-purpose table that gives a reduced penalty to
substitutions between related symbols:

- vowel ↔ vowel (e.g. `a`↔`e`) — moderately similar,
- consonant ↔ consonant — mildly similar,
- common OCR/typo confusions such as `0`↔`o`, `1`↔`l`, `1`↔`i`, `5`↔`s`.

This is why, with the default configuration, `hell0` fuzzily matches `hello` and `w0rld` matches
`world`.

## Providing your own

Supply a `&'static Similarity` built from a map of `(char, char) → similarity`. A `LazyLock` is the
usual way to get a `'static`:

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

`Similarity::from_map` sets the diagonal (identical pairs) to `1.0` for you and precomputes a fast
lookup table for ASCII pairs, falling back to the map for non-ASCII.

## Notes

- **Directionality.** Entries are ordered pairs. If you want `@`↔`a` to behave symmetrically, insert
  both `('@','a')` and `('a','@')`, as above.
- **Replacing vs. extending.** Providing a table *replaces* the default entirely — you get exactly
  the pairs you insert (plus the identity diagonal). If you want the default confusions too,
  reproduce them in your map.
- **Single symbols only.** The table maps one symbol to one symbol. For equivalences spanning several
  graphemes (ligatures, transliterations), use [multi-character mappings](mappings.md).
- **Interaction with the floor.** A high similarity makes a substitution cheap; the
  [weakest-link floor](floor.md) can still reject substitutions whose similarity is below a threshold
  regardless of how the overall score comes out.
