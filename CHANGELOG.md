# Changelog

All notable changes to this project are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/), and the project follows
[Semantic Versioning](https://semver.org/).

## [Unreleased]

### Fixed

- **A pattern's own limits no longer gate longer patterns that share its prefix.** The search gates
  each step of a walk on the limits of *the pattern ending at the current automaton node*. A walk
  there may instead be on its way to a longer pattern whose trie path runs through that node, and
  that pattern may have a far larger budget. With `["ab"` capped at 0 substitutions, `"abc"` allowed
  1], the substitution that leaves the `"ab"` node was refused, so **`"abc"` did not match `"abx"` at
  all**. Each node now carries the element-wise maximum of the budgets of every pattern whose path
  includes it, which is sound — if any one pattern admits a walk, the maximum admits it too. The
  per-pattern check when a match is reported is unchanged and stays authoritative. This also fixes
  duplicate patterns that carry different limits, where only the first one's applied.
- **A pattern with no limits of its own is exact again, even in a set where others have limits.**
  When no engine-wide limits were set, the builder derived a permissive set from the patterns that did
  have limits, and a pattern carrying none fell back to it. That derived set was never `finalize`d,
  so its unset fields read as *unconstrained* rather than `0`: mixing one limited pattern with a
  plain one made the plain one match with **unbounded** edits. `FuzzyLimits::default` is documented as
  "no fuzziness", so a pattern that sets no limits now stays exact. The reason the derivation existed
  — walks being blocked at a node no pattern ends — is now handled by the per-node budgets above.
- **Equally-cheap alignments of one pattern at different spans are no longer conflated.** The search
  deduplicates equivalent states per window, and the dedup key briefly omitted the matched span's
  end, on the reasoning that it is recoverable from the text position and the insertion count. That
  is false: the span end sits just past the *last pattern grapheme that was aligned*, so it is the
  text position minus the insertions taken *after* that alignment. Two alignments can therefore
  reach the same automaton node, text position, span start, edit counts **and penalty** at different
  span ends — for pattern `"yx"` on `"ybyyya"`, an exact-then-insert-then-substitute run ends at 6
  while exact-then-substitute-then-insert ends at 5 — and the dedup merged them, dropping one match.
  Caught by a new property test that checks the search against a brute-force reference.
- **Matches are reported at the span of the pattern that matched, in fuzzy searches too.** A
  pattern that is a *suffix* of the text a match consumed was reported a second time, at the
  consuming walk's whole span rather than the pattern's own. For the patterns `["abcd", "cd"]` on
  `"abcd"`, `"cd"` came back as `[0,4)` with `text` `"abcd"` in addition to the correct `[2,4)`.
  Such a match was self-contradictory: it reported `edits == 0` while its span was longer than the
  pattern it matched. With insertions a span legitimately *is* longer, so the invariant is
  "zero edits ⇒ span length == pattern grapheme length". Visible in `search`'s output; the
  segmentation helpers were unaffected because overlap resolution happened to discard the extra
  match. A node's `output` now holds only the patterns that end at it, and the exact scan reaches
  suffix patterns by walking the failure chain. The same change also fixed a related scoring bug: a
  suffix pattern used to be scored with the penalties of the longer walk that reached its node, so
  its `similarity` could be understated. Since a reported match's node is now always the node where
  its own pattern ends, the walk that reaches it *is* that pattern's alignment, and every match is
  scored on the edit sequence that actually produced it.

### Performance

- **Exact search is ~6x faster** (15.8 → 2.6 ns/byte on 250 KiB with 4 patterns; −85% end-to-end on
  the full `search` call). A search with no edit budget no longer restarts a BFS at every start
  position — it makes a single left-to-right Aho–Corasick pass, using the failure links the builder
  already computed. That closes most of the gap to the `aho-corasick` crate (1.6 ns/byte) and puts
  it far ahead of the `eregex` engine (318 ns/byte). Fuzzy searches are unchanged.
- **A SIMD scan replaces the byte-at-a-time skip** over haystack that cannot start a match, and the
  set of match-starting bytes is precomputed once per automaton rather than per search. A pattern
  set whose first graphemes are rare (`z`/`q`/`x`…) skips long runs, which is where SIMD pays:
  ~22% faster on such a corpus. Dense first-grapheme sets are unaffected — the skip stays scalar
  there anyway, because `memchr` has a fixed per-call setup that only pays beyond a ~30-byte run.
- **New dependency:** `memchr`, for that scan.

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
