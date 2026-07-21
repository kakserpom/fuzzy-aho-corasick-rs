# Segmentation & Splitting

Beyond "where are the matches", the engine can slice a string into matched and unmatched pieces and
reassemble it — useful for tokenization, cleanup, and redaction-style tasks. These build on
[`search_non_overlapping`](search.md).

## Segments

A `Segment` is either a `Matched` span or the `Unmatched` gap between matches. `segment_iter` yields
them in order:

```rust
use fuzzy_aho_corasick::{FuzzyAhoCorasickBuilder, FuzzyLimits, Segment};

let engine = FuzzyAhoCorasickBuilder::new()
    .fuzzy(FuzzyLimits::new().edits(1))
    .build(["input", "more"]);

for seg in engine.segment_iter("someinptandm0re", 0.75) {
    match seg {
        Segment::Matched(m)   => println!("match: {:?} (as {})", m.text, m.pattern),
        Segment::Unmatched(u) => println!("gap:   {:?}", u.text),
    }
}
```

## Reconstruction with spacing

`segment_text` reassembles the input, inserting spacing so that matched tokens are separated from the
surrounding text — a quick way to "tokenize" run-together text:

```rust
# use fuzzy_aho_corasick::{FuzzyAhoCorasickBuilder, FuzzyLimits};
let engine = FuzzyAhoCorasickBuilder::new()
    .fuzzy(FuzzyLimits::new().edits(1))
    .build(["input", "more"]);
let matches = engine.search_non_overlapping("someinptandm0re", 0.75);
assert_eq!(matches.segment_text(), "some inpt and m0re");
```

## Splitting on matches

Treat each fuzzy match as a delimiter and collect the pieces in between:

```rust
use fuzzy_aho_corasick::{FuzzyAhoCorasickBuilder, FuzzyLimits};

let engine = FuzzyAhoCorasickBuilder::new()
    .fuzzy(FuzzyLimits::new().edits(1))
    .case_insensitive(true)
    .build(["FOO", "BAR"]);

let parts: Vec<&str> = engine.split("xxFo0yyBAARzz", 0.8).collect();
assert_eq!(parts, vec!["xx", "yy", "zz"]);
```

`FuzzyMatches::split()` does the same on an already-computed result set (including empty pieces when
matches touch the ends).

## Stripping affixes

`strip_prefix` and `strip_postfix` remove leading/trailing fuzzy-matched (and whitespace-only)
segments and return the remainder — handy for peeling boilerplate off a field:

```rust
use fuzzy_aho_corasick::{FuzzyAhoCorasickBuilder, FuzzyLimits};

let f = FuzzyAhoCorasickBuilder::new()
    .fuzzy(FuzzyLimits::new().edits(1))
    .case_insensitive(true)
    .build(["LOREM", "IPSUM"]);

// "LrEM ISuM" fuzzily matches "LOREM IPSUM"; it and the leading space are stripped.
assert_eq!(f.strip_prefix("LrEM ISuM Lorm ZZZ", 0.8), "ZZZ");
assert_eq!(f.strip_postfix("ZZZ LrEM ISuM", 0.8), "ZZZ");
```

All of these are convenience wrappers over `search_non_overlapping` followed by a method on the
[`FuzzyMatches`](search.md) result, so you can mix and match with your own filtering.
