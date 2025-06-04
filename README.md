# fuzzy-aho-corasick

A high-performance, Unicode-aware Rust implementation of the Aho–Corasick automaton with **fuzzy matching** (insertions,
deletions, substitutions, transpositions) and optional SIMD acceleration.

This library is rooted in the scientific work “Fuzzified Aho–Corasick Search Automata” by Z. Horák, V. Snášel, A.
Abraham, and A. E. Hassanien (see the [PDF](DOCS/ias10_horak.pdf)).

## Features

- **Exact & Fuzzy Matching**: Locate exact patterns or allow configurable edit operations (Levenshtein distance +
  transposition).
- **Unicode‐Aware**: Operates on Unicode grapheme clusters and supports case‐insensitive matching.
- **SIMD‐Accelerated**: Uses [`std::simd`] on supported targets (ARM NEON, x86\_64 AVX2/AVX512) for batch output
  scoring.
- **Fine‐Grained Limits**: Per‐pattern caps on insertions, deletions, substitutions, swaps, or total edits.
- **Non‐Overlapping Option**: Greedily choose longest non‐overlapping matches from left to right.
- **Fuzzy Replacer**: Perform fuzzy find‐and‐replace, preserving unmatched segments.

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
fuzzy-aho-corasick = "0.2"
```

Enable SIMD acceleration (optional, off by default):

```toml
[dependencies]
fuzzy-aho-corasick = { version = "0.1", features = ["simd"] }
```

## Quick Start

```rust
use fuzzy_aho_corasick::{FuzzyAhoCorasickBuilder, FuzzyLimits};

fn main() {
    // Build an engine allowing up to 1 edit per match
    let engine = FuzzyAhoCorasickBuilder::new()
        .fuzzy(FuzzyLimits::new().edits(1))
        .case_insensitive(true)
        .non_overlapping(true)
        .build(["hello", "world"]);

    let text = "H3llo W0rld!";
    let matches = engine.search(text, 0.8);

    for m in matches {
        println!(
            "matched pattern '{}' as '{}' (score {:.2})",
            m.pattern, m.text, m.similarity
        );
    }
    // Output: matched pattern 'hello' as 'H3llo' (score 0.90)
}
```

## Builder API

```rust
let builder = FuzzyAhoCorasickBuilder::new()
// Maximum total edits (ins+del+sub+swap) per match
.fuzzy(FuzzyLimits::new().edits(2))
// Custom penalties
.penalties(FuzzyPenalties::default())
.substitution(0.7)
.insertion(0.9)
.deletion(0.9)
.swap(1))
// Unicode case folding
.case_insensitive(true)
// No overlapping matches
.non_overlapping(true);

let engine = builder.build(["pattern1", "pattern2"]);
```

## Fuzzy Replacer

```rust
let replacer = FuzzyAhoCorasickBuilder::new().build_replacer([
("foo", "bar"),
("baz", "qux"),
]);

let out = replacer.replace("F00 and BAZ!", 0.8);
assert_eq!(out, "bar and qux!");
```

## SIMD Acceleration

When compiled with the `simd` feature on supported architectures, output scoring batches are vectorized via [
`std::simd`], offering significant speedups for large pattern sets.

## License

This project is distributed under the MIT License. See [LICENSE](LICENSE) for details.

