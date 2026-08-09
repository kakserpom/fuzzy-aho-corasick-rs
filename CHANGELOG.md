# Changelog

All notable changes to this project are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/), and the project follows
[Semantic Versioning](https://semver.org/).

## [0.5.0] - 2026-08-09

A breaking release. See [`MIGRATING.md`](MIGRATING.md) for a step-by-step upgrade guide from 0.4.x.

### Changed

- **Unified search API.** The seven `search_*` methods collapse into a single
  `search(haystack, &SearchOptions)`. A [`SearchOptions`] bundles the similarity threshold with an
  [`Order`] (ranking) and an [`Overlap`] (overlap resolution); its builders are `const fn`, so a
  configuration can be defined once as a `const`.
- **Fallible searching.** `search` — and the helpers built on it (`split`, `strip_prefix`,
  `strip_suffix`, `segment_iter`, `segment_text`, `replace`) — now return `Result<_, SearchError>`
  instead of panicking on a haystack larger than `u32::MAX` grapheme clusters (~4 GiB of ASCII).
- **`replace` argument order** is now `(text, &SearchOptions, callback)` (callback last).
  `replace_stream` / `replace_stream_parallel` take the threshold before the callback.
- **Renamed `strip_postfix` to `strip_suffix`** to match the standard library.
- **`Similarity::from_map`** now accepts any `IntoIterator<Item = ((char, char), f32)>`; the internal
  `FxHashMap` / `FxHasher` are no longer public.

### Added

- [`SearchOptions`], [`Order`], [`Overlap`], `DEFAULT_THRESHOLD`, and the [`SearchError`]
  (`#[non_exhaustive]`) error type.
- Complete public-API documentation (enforced with `#![warn(missing_docs)]`) and an mdBook guide.

### Removed

- The debug-only `FuzzyMatch::notes` field.

---

Releases before 0.5.0 are not itemized here; see the
[GitHub releases](https://github.com/kakserpom/fuzzy-aho-corasick-rs/releases) and commit history.

[`SearchOptions`]: https://docs.rs/fuzzy-aho-corasick/latest/fuzzy_aho_corasick/struct.SearchOptions.html
[`Order`]: https://docs.rs/fuzzy-aho-corasick/latest/fuzzy_aho_corasick/enum.Order.html
[`Overlap`]: https://docs.rs/fuzzy-aho-corasick/latest/fuzzy_aho_corasick/enum.Overlap.html
[`SearchError`]: https://docs.rs/fuzzy-aho-corasick/latest/fuzzy_aho_corasick/enum.SearchError.html
[0.5.0]: https://github.com/kakserpom/fuzzy-aho-corasick-rs/releases/tag/v0.5.0
