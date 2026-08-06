# Migrating from 0.4.x to 0.5.0

Version 0.5.0 is a breaking release. Two themes drive the changes:

1. **One search entry point.** The seven `search_*` methods (and the threshold-taking
   segmentation/replace helpers) collapse into a single `search(haystack, &SearchOptions)`, where a
   `SearchOptions` bundles the threshold with a ranking `Order` and an `Overlap` resolver.
2. **Fallible instead of panicking.** Searching a haystack larger than `u32::MAX` grapheme clusters
   (~4 GiB of ASCII) used to panic; it now returns `Err(SearchError::HaystackTooLarge)`. `search`
   and the helpers built on it return `Result<_, SearchError>`.

Everything below is a mechanical change — no behavior changed for in-range inputs.

## At a glance

| 0.4.x | 0.5.0 |
|-------|-------|
| `engine.search(hay, 0.8)` | `engine.search(hay, &SearchOptions::new().threshold(0.8).sorted())?` |
| `engine.search_unsorted(hay, 0.8)` | `engine.search(hay, &SearchOptions::new().threshold(0.8))?` |
| `engine.search_non_overlapping(hay, 0.8)` | `.threshold(0.8).sorted().non_overlapping()` |
| `engine.strip_postfix(hay, 0.8)` | `engine.strip_suffix(hay, &SearchOptions::new().threshold(0.8))?` |
| `engine.replace(text, cb, 0.8)` | `engine.replace(text, &SearchOptions::new().threshold(0.8), cb)?` |
| `engine.replace_stream(r, w, cb, 0.8)` | `engine.replace_stream(r, w, 0.8, cb)?` |
| `Similarity::from_map(fx_hash_map)` | `Similarity::from_map([(('@','a'), 0.9), …])` |
| `m.notes` (debug builds) | *removed* |

## 1. `search` is now `search(haystack, &SearchOptions)` and fallible

`SearchOptions` carries three things: the `threshold`, an [`Order`] (how matches are ranked), and an
[`Overlap`] (how overlaps are resolved). Build it with chainable setters. Map each old method to the
options that reproduce it:

| 0.4.x method | 0.5.0 `SearchOptions` |
|--------------|-----------------------|
| `search_unsorted(hay, t)` | `.threshold(t)` |
| `search(hay, t)` | `.threshold(t).sorted()` |
| `search_greedy(hay, t)` | `.threshold(t).greedy()` |
| `search_coverage_weighted(hay, t)` | `.threshold(t).coverage_weighted()` |
| `search_non_overlapping(hay, t)` | `.threshold(t).sorted().non_overlapping()` |
| `search_non_overlapping_unique(hay, t)` | `.threshold(t).sorted().non_overlapping_unique()` |
| `search_non_overlapping_unique_coverage_weighted(hay, t)` | `.threshold(t).coverage_weighted().non_overlapping_unique()` |

```rust
// 0.4.x
let matches = engine.search_non_overlapping("helllo wolrd", 0.8);

// 0.5.0
use fuzzy_aho_corasick::SearchOptions;
let matches = engine
    .search("helllo wolrd", &SearchOptions::new().threshold(0.8).sorted().non_overlapping())
    .unwrap();
```

The default `SearchOptions::new()` is `Order::Unsorted` + `Overlap::Keep` — i.e. the old
`search_unsorted`, the fast raw-best-per-span result. `.sorted()` is the old default `search`.

### Handling the `Result`

`search` (and `split` / `strip_prefix` / `strip_suffix` / `segment_iter` / `segment_text` /
`replace`) now return `Result<_, SearchError>`. If you were relying on the old panic, `.unwrap()`
reproduces it; otherwise propagate with `?`. The only error is
[`SearchError::HaystackTooLarge`] (haystack over `u32::MAX` graphemes) — for inputs beyond that,
use the [streaming API](https://docs.rs/fuzzy-aho-corasick/latest/fuzzy_aho_corasick/struct.StreamMatches.html).

### Reuse options as a `const`

The builder methods are `const fn`, so a fixed configuration can be defined once:

```rust
use fuzzy_aho_corasick::SearchOptions;
const OPTS: SearchOptions = SearchOptions::new().threshold(0.8).non_overlapping();
let matches = engine.search("helllo wolrd", &OPTS).unwrap();
```

## 2. Segmentation helpers take `&SearchOptions`; `strip_postfix` → `strip_suffix`

`split`, `strip_prefix`, `strip_suffix`, `segment_iter`, and `segment_text` now take a
`&SearchOptions` instead of a bare threshold, and are fallible. And `strip_postfix` was **renamed to
`strip_suffix`** to match `std` vocabulary.

```rust
// 0.4.x
let rest  = engine.strip_prefix("LrEM ISuM ZZZ", 0.8);
let start = engine.strip_postfix("ZZZ LrEM ISuM", 0.8);

// 0.5.0
use fuzzy_aho_corasick::SearchOptions;
let rest  = engine.strip_prefix("LrEM ISuM ZZZ", &SearchOptions::new().threshold(0.8)).unwrap();
let start = engine.strip_suffix("ZZZ LrEM ISuM", &SearchOptions::new().threshold(0.8)).unwrap();
```

## 3. `replace`: options before the callback

The callback now comes **last**, with the options in the middle (so the closure reads cleanly at the
call site), and it's fallible.

```rust
// 0.4.x — (text, callback, threshold)
let out = engine.replace("FOO BAR", |m| /* … */ Some("###"), 0.8);

// 0.5.0 — (text, &SearchOptions, callback)
use fuzzy_aho_corasick::SearchOptions;
let out = engine
    .replace("FOO BAR", &SearchOptions::new().threshold(0.8), |m| /* … */ Some("###"))
    .unwrap();
```

## 4. Streaming replace: threshold before the callback

`replace_stream` and `replace_stream_parallel` moved the threshold ahead of the callback (matching
the closure-last convention). These still take a bare `f32` threshold (not `SearchOptions`).

```rust
// 0.4.x
engine.replace_stream(reader, writer, |m| Some("X"), 0.8)?;
engine.replace_stream_parallel(reader, writer, threads, |m| Some("X"), 0.8)?;

// 0.5.0
engine.replace_stream(reader, writer, 0.8, |m| Some("X"))?;
engine.replace_stream_parallel(reader, writer, threads, 0.8, |m| Some("X"))?;
```

`FuzzyReplacer::replace` also takes `&SearchOptions` now:

```rust
// 0.4.x
replacer.replace("hell0 w0rld", 0.8)?;
// 0.5.0
replacer.replace("hell0 w0rld", &SearchOptions::new().threshold(0.8))?;
```

## 5. `Similarity::from_map` takes an iterator; `FxHashMap` is no longer public

`from_map` now accepts any `IntoIterator<Item = ((char, char), f32)>`, so you no longer construct
(and can no longer import) the crate's `FxHashMap` — pass an array, `Vec`, or any map directly.

```rust
// 0.4.x
use fuzzy_aho_corasick::structs::{Similarity, FxHashMap};
let mut map = FxHashMap::default();
map.insert(('@', 'a'), 0.9);
map.insert(('a', '@'), 0.9);
let sim = Similarity::from_map(map);

// 0.5.0
use fuzzy_aho_corasick::structs::Similarity;
let sim = Similarity::from_map([
    (('@', 'a'), 0.9),
    (('a', '@'), 0.9),
]);
```

## 6. `FuzzyMatch::notes` was removed

The debug-only `notes` field on `FuzzyMatch` (present only in debug builds) is gone. It was a
footgun — code that read it wouldn't compile in release. If you were using it for diagnostics, enable
the crate's tracing during development instead.

## Not breaking, but new

- **`SearchError`** — the new public error type returned by the fallible methods.
- **`SearchOptions` / `Order` / `Overlap`** — the new options types, re-exported at the crate root.
- **`const fn` builders** on `SearchOptions`, so options can be `const`/`static`.

If something isn't covered here, the compiler is your guide: every removed/renamed method is a hard
error pointing at the call site, and the mappings above cover each one.

[`Order`]: https://docs.rs/fuzzy-aho-corasick/latest/fuzzy_aho_corasick/enum.Order.html
[`Overlap`]: https://docs.rs/fuzzy-aho-corasick/latest/fuzzy_aho_corasick/enum.Overlap.html
[`SearchError::HaystackTooLarge`]: https://docs.rs/fuzzy-aho-corasick/latest/fuzzy_aho_corasick/enum.SearchError.html
