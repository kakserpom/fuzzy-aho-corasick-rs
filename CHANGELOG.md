# Changelog

All notable changes to this project are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/), and the project follows
[Semantic Versioning](https://semver.org/).

## [Unreleased]

### Fixed

- **Exact matches are now reported at their own span.** A pattern that is a *suffix* of the text
  the search consumed was reported a second time, at the consuming walker's whole span instead of
  the pattern's own. For the patterns `["abcd", "cd"]` on `"abcd"`, `"cd"` came back as `[0,4)`
  with `text` `"abcd"` in addition to the correct `[2,4)`. Such a match was self-contradictory: it
  reported `edits == 0` while its span was longer than the pattern it matched. Visible in
  `search`'s output; the segmentation helpers were unaffected because overlap resolution happened to
  discard the extra match. Applies to exact searches; fuzzy searches still report it (see below).
  A match with insertions legitimately spans more than its pattern, so the invariant is
  "zero edits ⇒ span length == pattern grapheme length".

### Performance

- **Exact search is ~6x faster** (15.8 → 2.6 ns/byte on 250 KiB with 4 patterns; −85% end-to-end on
  the full `search` call). A search with no edit budget no longer restarts a BFS at every start
  position — it makes a single left-to-right Aho–Corasick pass, using the failure links the builder
  already computed. That closes most of the gap to the `aho-corasick` crate (1.6 ns/byte) and puts
  it far ahead of the `eregex` engine (318 ns/byte). Fuzzy searches are unchanged.

### Known issues

- Fuzzy search still reports a suffix pattern at the consuming walker's span, and scores it with
  that walker's penalties rather than its own alignment. Fixing it needs the builder to keep a
  node's own output separate from what it inherits along failure links, so the search can tell
  "this pattern ended here" from "this pattern is a suffix of what I consumed".

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
