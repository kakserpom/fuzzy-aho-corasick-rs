# Changelog

All notable changes to this project are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/), and the project follows
[Semantic Versioning](https://semver.org/).

## [Unreleased]

### Fixed

- **Matches are reported at the span of the pattern that matched, in fuzzy searches too.** A
  pattern that is a *suffix* of the text a match consumed was reported a second time, at the
  consuming walk's whole span rather than the pattern's own. For the patterns `["abcd", "cd"]` on
  `"abcd"`, `"cd"` came back as `[0,4)` with `text` `"abcd"` in addition to the correct `[2,4)`.
  Such a match was self-contradictory: it reported `edits == 0` while its span was longer than the
  pattern it matched. With insertions a span legitimately *is* longer, so the invariant is
  "zero edits ⇒ span length == pattern grapheme length". Visible in `search`'s output; the
  segmentation helpers were unaffected because overlap resolution happened to discard the extra
  match. A node's `output` now holds only the patterns that end at it, and the exact scan reaches
  suffix patterns by walking the failure chain.

### Performance

- **Exact search is ~6x faster** (15.8 → 2.6 ns/byte on 250 KiB with 4 patterns; −85% end-to-end on
  the full `search` call). A search with no edit budget no longer restarts a BFS at every start
  position — it makes a single left-to-right Aho–Corasick pass, using the failure links the builder
  already computed. That closes most of the gap to the `aho-corasick` crate (1.6 ns/byte) and puts
  it far ahead of the `eregex` engine (318 ns/byte). Fuzzy searches are unchanged.

### Known issues

- A fuzzy match is scored with the penalties of the whole walk that reached its node. That is
  correct for a pattern ending at that node, but the walk may have covered a longer pattern, so the
  score can be understated for a shorter one reached along the same path.

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
