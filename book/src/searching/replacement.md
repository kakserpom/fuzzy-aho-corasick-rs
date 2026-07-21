# Replacement

Fuzzy find-and-replace substitutes matched spans with text you choose, copying everything else
through unchanged. Non-overlapping matches are selected automatically (via
[`search_non_overlapping`](search.md)) and applied left-to-right.

## `replace` with a callback

The most flexible form is [`FuzzyAhoCorasick::replace`], which calls your closure for each match. Return
`Some(replacement)` to substitute, or `None` to keep the original text:

```rust
use fuzzy_aho_corasick::FuzzyAhoCorasickBuilder;

let engine = FuzzyAhoCorasickBuilder::new().build(["FOO", "BAR", "BAZ"]);
let result = engine.replace("FOO BAR BAZ", |m| {
    (m.pattern.pattern == "BAR").then_some("###")
}, 0.8);
assert_eq!(result, "FOO ### BAZ");
```

The closure receives the full [`FuzzyMatch`](search.md), so the replacement can depend on which
pattern matched, the matched text, the score, or the edit counts. The return type is
`Into<Cow<str>>`, so you can return a `&str`, a `String`, or a borrowed slice of the haystack.

## `FuzzyReplacer` for table-driven replacement

When you just have a `(pattern → replacement)` table, build a [`FuzzyReplacer`]:

```rust
use fuzzy_aho_corasick::{FuzzyAhoCorasickBuilder, FuzzyLimits};

let replacer = FuzzyAhoCorasickBuilder::new()
    .case_insensitive(true)
    .fuzzy(FuzzyLimits::new().edits(1))
    .build_replacer([("hello", "hi"), ("world", "earth")]);

// '0'↔'o' is a near-match in the default table, so both fuzzy tokens are replaced.
assert_eq!(replacer.replace("hell0 w0rld!", 0.8), "hi earth!");
```

`build_replacer` takes `(pattern, replacement)` pairs; the pattern side accepts the same conversions
as [`build`](../building/patterns.md), so you can attach weights and per-pattern limits. Reach the
underlying engine with `replacer.engine()`.

## Which non-overlapping match wins?

Replacement uses the default sort before resolving overlaps, so where several matches compete for a
region the higher-similarity (then longer, then earlier) one is applied. If that isn't the behavior
you want, run a search yourself with a different [ordering](search.md), then call
`FuzzyMatches::replace(callback)` on the result.

## Streaming replacement

For inputs too large to hold in memory, or arriving incrementally, use the streaming variants
`replace_stream` and `replace_stream_parallel`, which write the transformed output to any `Write`
sink in constant memory. See [Streaming Replace](../streaming/replace.md).

[`FuzzyAhoCorasick::replace`]: https://docs.rs/fuzzy-aho-corasick/latest/fuzzy_aho_corasick/struct.FuzzyAhoCorasick.html#method.replace
[`FuzzyReplacer`]: https://docs.rs/fuzzy-aho-corasick/latest/fuzzy_aho_corasick/struct.FuzzyReplacer.html
