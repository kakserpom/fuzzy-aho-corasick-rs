# fuzzy-aho-corasick

[![CI](https://github.com/kakserpom/fuzzy-aho-corasick-rs/actions/workflows/ci.yml/badge.svg)](https://github.com/kakserpom/fuzzy-aho-corasick-rs/actions/workflows/ci.yml) [![crates.io](https://img.shields.io/crates/v/fuzzy-aho-corasick.svg)](https://crates.io/crates/fuzzy-aho-corasick) [![docs.rs](https://img.shields.io/docsrs/fuzzy-aho-corasick)](https://docs.rs/fuzzy-aho-corasick) [![MIT License](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

High-performance, Unicode-aware, safe Rust implementation of the Aho–Corasick automaton with **fuzzy matching**
(insertions, deletions, substitutions, transpositions).

## Key Features

- **Exact & Fuzzy Matching**: Match literal patterns or allow configurable approximate matching with edit operations
  (Levenshtein-style + transposition).
- **Unicode-Aware**: Operates over grapheme clusters, with optional case-insensitive matching.
- **Multi-Character Mappings**: Register equivalences like `æ`↔`ae`, `ß`↔`ss`, `ks`↔`x` (bidirectional, scored).
- **Fine-Grained Limits**: Global or per-pattern caps on insertions, deletions, substitutions, swaps, and total edits.
- **Non-Overlapping Selection**: Choose a maximal set of non-overlapping matches with configurable heuristics.
- **Fuzzy Replacer**: Find-and-replace fuzzily while preserving surrounding context.
- **Segmentation API**: Split input into matched / unmatched segments via `segment_iter` / `segment_text`.
- **Customizable Scoring**: Weighting and penalty tuning for substitution, insertion, deletion, and swap.
- **Bounded Worst Case**: Optional beam search and an opt-in automatic beam keep pathological inputs from blowing up.
- **Streaming**: Search a `Read` source incrementally in constant memory (files, sockets, pipes — any size) via callback, iterator, or parallel APIs, with absolute `u64` offsets — or stream fuzzy find-and-replace straight to a `Write` sink.
- **Bit-Parallel Pre-Filter**: Opt-in fast lane that skips regions that provably can't match, with **identical results** — a multiple-× speedup on large, sparse inputs.

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
fuzzy-aho-corasick = "0.4"
```

Then in code:

```rust
use fuzzy_aho_corasick::{FuzzyAhoCorasickBuilder, FuzzyLimits};
```

## Quick Start

```rust
use fuzzy_aho_corasick::{FuzzyAhoCorasickBuilder, FuzzyLimits};

fn main() {
    // Build an engine allowing up to 1 edit per match, case-insensitive.
    let engine = FuzzyAhoCorasickBuilder::new()
        .fuzzy(FuzzyLimits::new().edits(1))
        .case_insensitive(true)
        .build(["hello", "world"]);

    // "helllo wolrd" has two typos: an extra 'l' (insertion) and swapped 'lr' (transposition).
    for m in engine.search_non_overlapping("helllo wolrd", 0.8).iter() {
        println!("matched '{}' as '{}' (score {:.2})", m.pattern, m.text, m.similarity);
    }
    // Output:
    //   matched 'hello' as 'helllo' (score 0.90)
    //   matched 'world' as 'wolrd' (score 0.90)
}
```

### How scoring works

For a candidate match against a pattern of length `N` graphemes with accumulated edit `penalties`, the similarity is:

```text
similarity = (N - penalties) / N * weight
```

Each edit adds a penalty scaled by how dissimilar the graphemes are (see [`FuzzyPenalties`] and the similarity table).
A match is kept only if `similarity >= threshold`. With the default `weight` of `1.0`, similarity is in `0.0..=1.0`;
pattern weights above `1.0` can push a match's score above `1.0` to prioritize it.

## Builder API

Customize fuzzy behavior, penalties, and case folding:

```rust
use fuzzy_aho_corasick::{FuzzyAhoCorasickBuilder, FuzzyLimits, FuzzyPenalties};

let engine = FuzzyAhoCorasickBuilder::new()
    .fuzzy(FuzzyLimits::new().edits(2)) // up to 2 total edits per pattern by default
    .penalties(
        FuzzyPenalties::default()
            .substitution(0.7)
            .insertion(0.9)
            .deletion(0.9)
            .swap(1.0),
    )
    .case_insensitive(true)
    .build(["pattern1", "pattern2"]);
```

Builder options:

| Method | Purpose |
| --- | --- |
| `fuzzy(FuzzyLimits)` | Global default edit limits for every pattern. |
| `penalties(FuzzyPenalties)` | Cost of each edit type; shapes which edits are “cheaper”. |
| `case_insensitive(bool)` | Unicode-aware case folding. |
| `beam_width(usize)` | Cap the active frontier to the K lowest-penalty states (approximate; faster). |
| `auto_beam(budget, width)` | Stay exact until `budget` states are explored, then beam to `width` (see below). |
| `similarity(&'static Similarity)` | Provide a custom grapheme similarity table (see [Custom Similarity](#custom-similarity)). |
| `min_symbol_similarity(f32)` | Reject substitutions below a per-character similarity floor (see [Weakest-link floor](#weakest-link-floor)). |
| `build(patterns)` | Build the immutable engine. |
| `build_replacer(pairs)` | Build a [`FuzzyReplacer`] from `(pattern, replacement)` pairs. |

## Pattern Weights & Direct Pattern Construction

By default, all patterns have weight `1.0`, but you can adjust per-pattern scoring and fuzzy limits directly via
`Pattern` before building the automaton. You can also pass tuples or fully constructed `Pattern` values to `build(...)`.

* `Pattern::from(&str | String)`: build a pattern with default weight and no per-pattern limits.
* `Pattern::weight(f32)`: set the pattern's weight (default `1.0`), scaling its similarity score.
* `Pattern::fuzzy(FuzzyLimits)`: apply per-pattern edit limits (override the global default).
* `Pattern::custom_unique_id(usize)`: give a stable identity for uniqueness-aware matching
  (`non_overlapping_unique`).

`build(...)` accepts anything convertible into `Pattern`, including:

* `&str` / `String`
* `(&str, f32)` — `(pattern, weight)`
* `(&str, f32, u8)` — `(pattern, weight, max_edits)`
* a fully built `Pattern`

### Examples

Weighting via tuples, and case-insensitive Greek:

```rust
use fuzzy_aho_corasick::FuzzyAhoCorasickBuilder;

// Each tuple is (pattern, weight).
let engine = FuzzyAhoCorasickBuilder::new()
    .case_insensitive(true)
    .build([("Γειά", 1.0), ("σου", 1.0)]);
assert!(!engine.search("γειά ΣΟΥ!", 0.8).is_empty());
```

Customizing patterns with per-pattern fuzzy limits, weights, and unique IDs:

```rust
use fuzzy_aho_corasick::{FuzzyAhoCorasickBuilder, FuzzyLimits, Pattern};

let important = Pattern::from("error")
    .weight(2.0) // boost this pattern's score
    .fuzzy(FuzzyLimits::new().edits(1))
    .custom_unique_id(42);

let normal = Pattern::from("warning").fuzzy(FuzzyLimits::new().edits(2));

let engine = FuzzyAhoCorasickBuilder::new()
    .case_insensitive(true)
    .build([important, normal]);
```

These allow fine-grained control over ranking, deduplication, and fuzzy tolerance on a per-pattern basis.

## Match Selection Strategies

`search_unsorted` returns the raw best match per span; the following orderings and overlap resolvers refine it. They
are methods on the returned [`FuzzyMatches`]:

* `default_sort()`: Prioritizes higher similarity, then longer patterns, then earlier position.
* `greedy_sort()`: Prefers longer patterns first, then similarity.
* `coverage_weighted_sort()`: Ranks by `similarity * covered_length`, favoring matches that cover more text.
* `non_overlapping()`: Greedily drops overlapping matches in the current order.
* `non_overlapping_unique()`: Same, but ensures each pattern (respecting `custom_unique_id`) is used at most once.

Convenience entrypoints on the engine (each takes `(haystack, threshold)`):

* `search(...)`: default sort.
* `search_greedy(...)`: greedy sort.
* `search_coverage_weighted(...)`: coverage-weighted sort.
* `search_non_overlapping(...)`: default sort + non-overlapping selection.
* `search_non_overlapping_unique(...)`: default sort + non-overlapping + pattern-unique.
* `search_non_overlapping_unique_coverage_weighted(...)`: coverage-weighted variant of the above.
* `search_unsorted(...)`: raw, unsorted best-per-span matches (build your own pipeline).

## Bounding Worst-Case Work

The core search is exact: it explores every viable edit path and returns the best match for each span. For most inputs
the built-in pruning keeps this fast, but combining a **high edit budget** with a **low similarity threshold** can
explode the state space (while usually yielding no additional matches). Two knobs bound this.

### Beam search — `beam_width(K)`

Whenever a search window's active frontier exceeds `2·K` states, it is sorted by penalty and truncated to the `K`
lowest-penalty candidates. This trades exactness for speed and bounded memory; a larger `K` is more accurate.

```rust
use fuzzy_aho_corasick::{FuzzyAhoCorasickBuilder, FuzzyLimits};

let engine = FuzzyAhoCorasickBuilder::new()
    .fuzzy(FuzzyLimits::new().edits(4))
    .case_insensitive(true)
    .beam_width(100) // keep the 100 best candidates when the frontier grows
    .build(["saddam", "hussein", "vestibulum"]);
```

### Automatic beam — `auto_beam(budget, width)`

`auto_beam` is a safety valve for pathological inputs. The search runs the **exact** unlimited exploration until it has
expanded more than `budget` states (counted across all start positions); only then does it beam the frontier to `width`
for the remainder. Ordinary searches never approach `budget`, so they stay **exact and unaffected**; only genuine
blow-ups get capped.

```rust
use fuzzy_aho_corasick::{FuzzyAhoCorasickBuilder, FuzzyLimits};

let engine = FuzzyAhoCorasickBuilder::new()
    .fuzzy(FuzzyLimits::new().edits(6))
    .case_insensitive(true)
    .auto_beam(200_000, 100) // exact under 200k states, then beam to width 100
    .build(["saddam", "hussein", "vestibulum"]);
```

An explicit `beam_width` always takes precedence over `auto_beam`.

## Streaming

Search a `Read` source incrementally instead of loading it all into memory. The streaming API
consumes the reader in bounded, overlapping windows, so it runs in **constant memory** regardless of
input size, works on files/sockets/pipes as data arrives, and — as one consequence — handles inputs
beyond the ~4 GiB a single `search` call supports (grapheme positions are `u32` internally). Matches
are reported at **absolute `u64` byte offsets**. The overlap is derived automatically from the
patterns and edit limits (the longest possible match), so no match is ever split at a window
boundary and each is emitted exactly once — no configuration, no deduplication on your side.

Three entry points, all yielding `StreamMatch { start, end, pattern_index, similarity, edits…, text }`:

```rust
use fuzzy_aho_corasick::{FuzzyAhoCorasickBuilder, FuzzyLimits};
use std::fs::File;

let engine = FuzzyAhoCorasickBuilder::new()
    .fuzzy(FuzzyLimits::new().edits(1))
    .case_insensitive(true)
    .build(["needle"]);

// 1) Callback (single-threaded)
engine.search_stream(File::open("huge.txt")?, 0.8, |m| {
    println!("{}..{}: pattern #{} ({:.2})", m.start, m.end, m.pattern_index, m.similarity);
})?;

// 2) Iterator
for m in engine.stream_matches(File::open("huge.txt")?, 0.8) {
    let m = m?; // io::Result<StreamMatch>
    println!("{}..{}", m.start, m.end);
}

// 3) Parallel (fans windows across a thread pool; dependency-free `std::thread`)
let threads = std::thread::available_parallelism().map_or(1, |n| n.get());
engine.search_stream_parallel(File::open("huge.txt")?, 0.8, threads, |m| { /* ... */ })?;
```

The search itself is CPU-bound (a BFS from every position, ~independent of window size), so the
parallel form is how you go fast: windows are independent and share the immutable engine, scaling
close to linearly with cores. `max_match_graphemes()` exposes the auto-computed overlap if you want
to window the input yourself. See [`examples/streaming.rs`](examples/streaming.rs) for a full
multi-GiB demo with a progress bar.

### Streaming replace

`replace_stream` is the streaming counterpart of [`replace`](#fuzzy-replacer): it reads from a `Read`,
writes the transformed stream to a `Write` in **constant memory**, substituting matches as they are
found and copying everything else through verbatim. It returns the number of bytes written.

```rust
use fuzzy_aho_corasick::{FuzzyAhoCorasickBuilder, FuzzyLimits};

let engine = FuzzyAhoCorasickBuilder::new()
    .fuzzy(FuzzyLimits::new().edits(1))
    .case_insensitive(true)
    .build(["needle"]);

let mut out = Vec::new();
// "neeedle" has one extra 'e' (an insertion); it is replaced, the rest copied through.
engine.replace_stream("a neeedle b".as_bytes(), &mut out, |_m| Some("X"), 0.8).unwrap();
assert_eq!(String::from_utf8(out).unwrap(), "a X b");
```

Matches are selected per window, so at a window boundary overlap is resolved left-to-right (the
earlier-starting match wins) rather than by the global ranking a whole-input `replace` would use; for
inputs where matches are separated by non-matching text the two agree exactly. The replacement may
borrow external data but not the transient matched text — return an owned `String` if you need to
derive it from `m.text`. `FuzzyReplacer` exposes the turnkey `replace_stream(reader, writer, threshold)`
using its configured `(pattern → replacement)` table. Wrap the writer in a `BufWriter` for throughput.

`replace_stream_parallel(reader, writer, threads, callback, threshold)` fans the (CPU-bound) search
across a thread pool while reassembling the output **in stream order** on the calling thread — so it
produces byte-identical output to `replace_stream`, and the callback and writer stay single-threaded
(no `Send`/`Sync` bounds). Because output is inherently ordered, only the search is parallelised.

## Bit-Parallel Pre-Filter

The core search is thorough but pays a per-position cost. When you search large inputs that are mostly
non-matching text, `with_prefilter()` adds an opt-in fast lane: a bit-parallel
([Bitap](https://en.wikipedia.org/wiki/Bitap_algorithm) / Wu–Manber) approximate scan runs first at
hundreds of MB/s to locate *candidate regions*, and the full weighted engine then re-searches only
those. Results are **identical** to `search` / `search_unsorted` — the filter is a conservative
over-approximation, so it never drops a real match; it only spares the engine from scanning text that
cannot contain one.

```rust
use fuzzy_aho_corasick::{FuzzyAhoCorasickBuilder, FuzzyLimits};

let engine = FuzzyAhoCorasickBuilder::new()
    .fuzzy(FuzzyLimits::new().edits(1))
    .build(["vestibulum", "consectetur"]);

let pf = engine.with_prefilter(); // build once, reuse across searches
let hits = pf.search("… lorem vestibulm ipsum …", 0.85);
// Same matches as engine.search(…), just faster on large, sparse inputs.
```

**How the budget is derived.** The score threshold bounds the total penalty a kept match may carry
(`P_max = N·(1 − θ/weight)`), and every edit costs at least some minimum penalty, so the number of edit
operations is bounded. That bound becomes the bit-parallel scan's Levenshtein budget `k` (a
transposition counts as two), guaranteeing every match the engine would accept survives the filter.

**Graceful fallback.** When the configuration can't be reduced to the bit model — multi-character
mappings present, a pattern longer than 63 graphemes, a penalty so low an edit is effectively free, or
a budget too large to stay selective — the wrapper transparently runs the full search instead. Check
`pf.is_active()` to see whether the filter was built. Either way results are correct; the fallback
merely forgoes the speedup.

**When it helps.** The win scales inversely with match density: on sparse inputs the engine sees only a
small fraction of the text (an ~8× end-to-end speedup on a 16 MiB sample), while match-saturated inputs
gain little (a wasted scan, then roughly baseline). See
[`examples/bitap_prototype.rs`](examples/bitap_prototype.rs) for the standalone algorithm, a
brute-force correctness verifier, and a throughput comparison.

## Segmentation and Reconstruction

Break text into matched/unmatched pieces and reassemble with intelligent spacing:

```rust
use fuzzy_aho_corasick::{FuzzyAhoCorasickBuilder, FuzzyLimits};

let engine = FuzzyAhoCorasickBuilder::new()
    .fuzzy(FuzzyLimits::new().edits(1))
    .build(["input", "more"]);
let matches = engine.search_non_overlapping("someinptandm0re", 0.75);
assert_eq!(matches.segment_text(), "some inpt and m0re");
```

### Splitting on Fuzzy Matches

Treat each fuzzy match as a delimiter and collect the unmatched pieces:

- **`FuzzyMatches::split()`** — splits the already-segmented stream, yielding the `Unmatched` parts (including empty
  ones if matches touch the ends).
- **`FuzzyAhoCorasick::split(haystack, threshold)`** — convenience: runs `search_non_overlapping` and calls `split()`.

```rust
use fuzzy_aho_corasick::{FuzzyAhoCorasickBuilder, FuzzyLimits};

let engine = FuzzyAhoCorasickBuilder::new()
    .fuzzy(FuzzyLimits::new().edits(1))
    .case_insensitive(true)
    .build(["FOO", "BAR"]);

// Treat each fuzzy match (>= 0.8) as a separator:
let parts: Vec<&str> = engine.split("xxFo0yyBAARzz", 0.8).collect();
assert_eq!(parts, vec!["xx", "yy", "zz"]);
```

## Post-Processing Utilities

Once you have a [`FuzzyMatches`] (for example, from `search_non_overlapping`), these methods trim, transform, or extract
from the matched set:

* **`replace(callback)`** — walks matches left-to-right (skipping overlaps) and calls
  `Fn(&FuzzyMatch) -> Option<S>` where `S: Into<Cow<str>>` (e.g. `&str` or `String`). `Some(repl)` replaces the span;
  `None` keeps the original text. See [Fuzzy Replacer](#fuzzy-replacer) for the turnkey version.
* **`strip_prefix()`** — drops leading fuzzy-matched (and whitespace-only) segments, trims the first kept segment, and
  returns the rest.
* **`strip_postfix()`** — the mirror image: removes trailing matched/whitespace segments and returns the leading text.
* **`matched_spans()` / `matched_strings()`** — the `(start, end)` byte ranges / the matched substrings.
* **`filter(pred)` / `retain(pred)`** — keep only matches satisfying a predicate (borrowing / in place).
* **`iter()` / `iter_mut()` / `len()` / `is_empty()`** — inspect the match set (also available via `Deref<[FuzzyMatch]>`).

### Fuzzy Replacer

Perform fuzzy find-and-replace with a mapping. Non-overlapping matches are chosen automatically using the default
heuristics.

```rust
use fuzzy_aho_corasick::{FuzzyAhoCorasickBuilder, FuzzyLimits};

let replacer = FuzzyAhoCorasickBuilder::new()
    .case_insensitive(true)
    .fuzzy(FuzzyLimits::new().edits(1))
    .build_replacer([("hello", "hi"), ("world", "earth")]);

// '0'↔'o' is a near-match in the default table, so both fuzzy tokens are replaced:
assert_eq!(replacer.replace("hell0 w0rld!", 0.8), "hi earth!");
```

## Custom Similarity

By default, substitutions between related graphemes (vowels, consonants, common OCR confusions like `0`↔`o`, `1`↔`l`)
carry a reduced penalty. Provide your own table for domain-specific confusions:

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

### Weakest-link floor

The default scoring is *additive*: a single very-dissimilar character costs a fixed penalty that a
long pattern dilutes (one `sim=0` substitution in a 20-grapheme pattern still scores ~0.93). If you
instead want *no* substitution to be too weak — the "weakest link" bound from the underlying
[paper](DOCS/ias10_horak.pdf) — set a per-character floor:

```rust
use fuzzy_aho_corasick::{FuzzyAhoCorasickBuilder, FuzzyLimits};

let engine = FuzzyAhoCorasickBuilder::new()
    .fuzzy(FuzzyLimits::new().edits(1))
    .case_insensitive(true)
    .min_symbol_similarity(0.3) // reject any substitution below 0.3 similarity
    .build(["vestibulum"]);

assert!(engine.search("vxstibulum", 0.8).is_empty()); // e↔x has similarity 0 -> rejected
assert_eq!(engine.search("vestibulom", 0.8).len(), 1); // u↔o (0.6) is fine
```

The floor applies only to character-level substitutions; exact matches and explicit mappings (which
carry their own scores) are unaffected. Default is `0.0` (no floor).

## Multi-Character Mappings

The similarity table maps single graphemes to single graphemes. For equivalences that span **several
graphemes** — ligatures and transliterations like `æ`↔`ae`, `ß`↔`ss`, `ks`↔`x` — register a mapping.
Either side may stand in for the other (mappings are **bidirectional**), and a mapping counts as one
substitution against the edit limits, exactly like a single-character similarity substitution.

```rust
use fuzzy_aho_corasick::{FuzzyAhoCorasickBuilder, FuzzyLimits};

let engine = FuzzyAhoCorasickBuilder::new()
    .case_insensitive(true)
    .fuzzy(FuzzyLimits::new().edits(1))
    .mapping("æ", "ae")          // exact equivalence (score 1.0, penalty-free)
    .mapping("ks", "x")
    .mapping_scored("ph", "f", 0.9) // a near-equivalence that carries a small penalty
    .build(["encyclopaedia", "alexander"]);

// 'æ' in the haystack matches the "ae" in the pattern (and vice versa):
assert_eq!(engine.search("encyclopædia", 0.95).len(), 1);
// 'x' in the pattern matches "ks" in the haystack:
assert_eq!(engine.search("aleksander", 0.95).len(), 1);
```

- **[`mapping(a, b)`](https://docs.rs/fuzzy-aho-corasick/latest/fuzzy_aho_corasick/struct.FuzzyAhoCorasickBuilder.html#method.mapping)** — exact equivalence (score `1.0`, no penalty).
- **`mapping_scored(a, b, score)`** — near-equivalence; the applied penalty is `substitution * (1 - score)`.

Because a mapping counts as a substitution, it obeys `.edits()` / `.substitutions()`: with `edits(0)`
even `æ`↔`ae` is rejected, just like `0`↔`o`. Mappings are precomputed at build time and stored
out-of-line, so configuring none leaves the search hot path completely unchanged.

## Performance

The engine is built once and is cheap to query repeatedly. Some tips:

* **Filter early with the similarity threshold** — a higher threshold prunes low-quality candidates before they expand.
* **Tune `FuzzyLimits` per pattern** when you know the expected error characteristics; tighter limits explore fewer
  states.
* **Shape ambiguity with `FuzzyPenalties`** — make substitutions or insertions cheaper/pricier to fit your domain.
* **Prefer `search_non_overlapping_*`** to avoid resolving overlaps yourself.
* **Guard against pathological inputs** with `beam_width` or `auto_beam` when edit limits are high and thresholds low.

Grapheme positions are represented as `u32` internally, so a single haystack is expected to be well under 4 GiB.

## Troubleshooting

* **Too many false positives** — raise the similarity threshold or tighten per-pattern limits.
* **Missing fuzzy matches** — lower the threshold, increase allowed edits, or make substitutions less punitive via
  penalties. If you enabled a beam, increase its width.
* **Overlapping matches unwanted** — use `search_non_overlapping` / `search_non_overlapping_unique` rather than raw
  `search_unsorted`.
* **A search is unexpectedly slow** — you are likely combining a high edit budget with a low threshold; add
  `auto_beam` (keeps common cases exact) or an explicit `beam_width`.

## Testing

Run the test suite with:

```sh
cargo test
```

Benchmarks (Criterion):

```sh
cargo bench
```

## License

Distributed under the **MIT License**. See `LICENSE` for details.

## Acknowledgements

Based on a research paper — [**Fuzzified Aho–Corasick Search Automata**](DOCS/ias10_horak.pdf) by Z. Horák, V. Snášel,
A. Abraham, and A. E. Hassanien.

[`FuzzyMatches`]: https://docs.rs/fuzzy-aho-corasick/latest/fuzzy_aho_corasick/struct.FuzzyMatches.html
[`FuzzyReplacer`]: https://docs.rs/fuzzy-aho-corasick/latest/fuzzy_aho_corasick/struct.FuzzyReplacer.html
[`FuzzyPenalties`]: https://docs.rs/fuzzy-aho-corasick/latest/fuzzy_aho_corasick/struct.FuzzyPenalties.html
