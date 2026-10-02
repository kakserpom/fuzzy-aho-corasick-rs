# Changelog

All notable changes to this project are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/), and the project follows
[Semantic Versioning](https://semver.org/).

## [Unreleased]

## [0.5.1] - 2026-09-29

### Changed

- **Three behaviour changes worth knowing about when upgrading.** Everything else below is either a
  fix for something that was outright wrong or a pure speedup; these three can change what a program
  *observes*, so they are called out here rather than left for the reader to assemble:
  - *Ranking counts pattern length in grapheme clusters, not bytes.* A multi-byte pattern can now
    rank differently against an otherwise equal candidate. ASCII is unaffected. See **Fixed**.
  - *A pattern that sets no limits of its own is now exact*, even in a set where other patterns do
    set limits. Previously it inherited a permissive set derived from its neighbours and could match
    with unbounded edits. Engines that relied on that inheritance should set limits explicitly. See
    **Fixed**.
  - *The pre-filter now declines to run* when it cannot pay for itself, transparently falling back to
    the plain search. Results are identical either way, so this is only observable as a difference in
    timing. See **Performance**.
- **Documented what the streaming search entry points actually return.** `search_stream`,
  `stream_matches` and `search_stream_parallel` resolve overlapping matches per window, as
  `sorted().non_overlapping()` does, and offer no option to ask for anything else — a match set is
  handed to the caller in pieces, and resolving overlaps is what makes those pieces disjoint. That
  was not written down anywhere, so comparing their output against `search` under the default
  `Unsorted + Keep` options — which returns every overlapping match — looks like a dropped-match bug
  when it is in fact the intended selection. All three doc comments now say so.

### Fixed

- **Match ranking counts pattern length in grapheme clusters, not bytes.** Ranking used
  `Pattern::len()`, which is documented as a *byte* count, while scoring uses `grapheme_len` — the
  crate's stated unit throughout ("Pattern length N, which drives scoring, is measured in grapheme
  clusters"). For ASCII the two agree, so the inconsistency was invisible; for anything else the
  ranking depended on how a pattern happened to be encoded rather than on what it contains. It was
  most consequential in `Order::CoverageWeighted`, whose score is `similarity² × pattern length`:
  similarity is a per-grapheme fraction, so multiplying by bytes scores a 4-grapheme Cyrillic
  pattern as twice the length of a 4-grapheme Latin one and hands it every comparison at equal
  similarity. `Order::Default`, `Order::Greedy` and the tiebreakers are now consistent, and the
  relevant doc comments say which unit is meant. **This changes ranking order for multi-byte
  patterns**; ASCII result sets are unaffected. (Note that `Overlap::NonOverlapping` re-sorts the kept
  matches by `start`, so the ranking decides *which* matches survive, not their output order.)
- **The unstartable-haystack skip no longer misreads non-ASCII bytes.** It probes a 128-bit bitmap of
  candidate bytes by shifting it by the haystack byte, but a byte at or above 128 shifts past the
  width of a `u128`: debug builds panic, and release builds mask the shift to 7 bits and read bit
  `b & 127` instead. The candidate set only ever covers ASCII, so any byte ≥ 128 is simply not a
  candidate — a UTF-8 lead or continuation byte could therefore be mistaken for one and the skip
  would stop early. That costs a wasted search window per false positive rather than a wrong answer,
  which is why it went unnoticed in release; running the suite without `--release` is what surfaced
  it. The test meant to catch it had the same shift in its own reference, so the two agreed and it
  could not fail.
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

- **A state that cannot report anything no longer sets up a call to find that out.** `report`
  already began with `if output.is_empty() { return; }`, but both call sites built the argument list
  and made the call regardless. Removing the call from the saturated walk entirely — which changes
  results, purely to price it — showed it cost **16.8%** of the walk, and none of that was the
  scoring body: with a long-pattern set a node only carries `output` when some pattern is exactly as
  long as its depth, so nearly every step of nearly every chain reports nothing. Both call sites now
  test emptiness themselves, so the hot path is a plain branch. Median of 11 paired rounds: 500
  patterns **0.91**, 200 sorted **0.96**, 4 patterns 0.99, exact unchanged.
- **Wide pattern sets get an O(1) exact-transition lookup, worth 1.3x on top of everything else
  here.** Finding an edge meant scanning the node's flat edge list. A u128 bitmap already rejected
  misses in constant time, but a *hit* still walked the list with an unpredictable exit, and on a
  500-pattern automaton 43% of states sit on a degree-9-to-18 node: 27 nodes take 43% of all state
  visits and carry roughly 78% of the scan's work. Those nodes now carry a 128-entry
  `ascii_byte -> target` table (13 KiB for 27 nodes), and the search is 1.3x faster than it was
  before this change.

  Two things about it are worth stating because both went the other way first. It is deliberately
  **additive rather than a reordering** of `edges`: sorting the list by `first_char` would give the
  same answer from a popcount rank with no allocation at all, but the edge order *is* the
  `transitions` map's, chosen precisely so tie-breaking among equal-similarity matches is unchanged.
  And the bitmap test runs **before** the table, not after — a node with no table should never have
  its `dense` field read, and on a narrow automaton that lookup misses 25 times in 26, so consulting
  the table first measured 20% on a wide corpus while costing 2.8% everywhere else.

  Getting the table in also meant fixing `Node`'s field order, which turned out to matter more than
  its size. `Node` spans two cache lines, and leaving the layout to the compiler shuffled the five
  fields read on every state across both of them: 3.6% on a 4-pattern corpus over 15 paired rounds,
  0/15 in favour, with the struct's size *unchanged* at 128 bytes. It is now `#[repr(C)]` with those
  five — edge list, edge bitmap, output list and the two pruning floats — in the first 64 bytes, so
  they share one line. `edges` became a `Box<[Edge]>` to pay for the new field; a `Box<[T]>` is 16
  bytes against a `Vec<T>`'s 24, which is exactly the budget. If a field joins the hot group, check
  the arithmetic still fits.

  `structs::dense_tests::dense_table_agrees_with_the_scan_on_every_node` checks the table against an
  independently computed scan answer for every node and every ASCII character, because only one of
  the two lookup paths is ever taken on a given corpus and neither implementation can otherwise be
  held against the other. Corrupting a table entry by one fails it at node 0.

  Cumulative for the four search changes in this section, against 0.5.1: 500 patterns **0.58**
  (1.7x), 500 pre-filtered **0.59**, 200 sorted **0.80**, 4 patterns **0.89**, sparse **0.90**,
  exact and Unicode unchanged.
- **The fuzzy search is 1.1-1.4x faster at a one-edit budget, by walking budget-exhausted states
  instead of queueing them.** A state that has spent its whole edit budget can only report and
  follow exact transitions, yet each one was still built into a 24-byte `State`, appended to the
  queue, and read back out — for the bulk of the states, since at `edits(1)` the substitution,
  deletion, insertion and swap children of the unsaturated chain are *all* saturated and each then
  runs a short exact chain of its own. Those chains are now followed in place, which takes the queue
  from roughly sixty entries per start position down to the handful of unsaturated states. Median of
  9 paired rounds against 0.5.1:
  - 500 patterns: **0.73** (1.37x)
  - 500 patterns, pre-filtered: **0.72** (1.39x)
  - 200 patterns, sorted + non-overlapping: **0.80** (1.26x)
  - 4 patterns: **0.89**, sparse corpus: **0.90**
  - exact search: **0.97**, Unicode path: unchanged

  **Results are identical.** The only order-sensitive part of this search is the beam, which keeps
  the lowest-penalty states in the frontier and breaks ties by position, so the walk is enabled only
  when neither beam is configured (both are opt-in and default to `None`) and the traversal order is
  then exactly what it was. It is also restricted to a budget of one, which is the only budget where
  these states are the bulk of the work — and the one budget at which the dedup table is already
  folded away, so there is no interaction with it. Multi-grapheme mappings are excluded, since a
  mapping lands a state somewhere a plain exact chain does not model.

  Each of the four call sites was checked by deliberately breaking it and confirming the suite
  notices: the substitution and insertion sites fail 8 and 5 tests respectively, and a transposition
  site — the one transition that consumes *two* text graphemes and so resumes at `j + 2` rather than
  `j + 1` — is not reliably reached by the general sweep, whose patterns and text are built
  independently at random and may never produce one. It now has a dedicated test that builds
  transposed text on purpose and additionally asserts a swap was actually reported, since otherwise
  it could pass by finding nothing.
- **The fuzzy search is ~2-3% faster everywhere, by taking the scoring code out of the state loop.**
  Reporting a match is a 74-line block — per-pattern limit check, similarity, hash-map update — that
  was written inline in the middle of the BFS's per-state body. It is also, for a long-pattern set,
  *never executed*: a node carries `output` only if some pattern is exactly as long as its depth, so
  with patterns of length 11 every node above depth 10 reports nothing and the block is dead weight
  sitting in the hottest loop in the crate. It now lives in a `ReportCtx` whose `report` inlines just
  the emptiness test and leaves the scoring in an out-of-line `report_slow`. Median of 13 paired
  rounds: 500 patterns **0.97**, 200 sorted **0.97**, 4 patterns **0.98**, exact search **0.98**.

  Getting there took two failures worth recording, because both were *refactors that should have
  been free* and both were slower:

  - Hoisting the block into a closure capturing `&mut best` cost **9.3%**. The capture stopped the
    caller's local being promotable and the map's internals spilled.
  - Moving it into an `#[inline]` method on a context struct was worse still, **12.0%**: the body is
    far too large for LLVM's inliner, so `report` stopped being inlined *at all* and every state paid
    a real call. Only splitting the cold part out made the wrapper small enough to inline.

  And clippy's own suggestion for the resulting eight-argument method cost another **4.4%** (13
  paired rounds, 0/13 in favour). Bundling the per-state arguments into a `ReportState` struct is
  exactly what `too_many_arguments` asks for, and it is slower: the struct is built and immediately
  destructured at the call site, where the flat form stays in registers. The arguments stay flat and
  the lint is silenced with that reason rather than obeyed.
- **The pre-filter's scan was ~6x slower than it should have been, which made the whole feature a net
  loss; it is now a large win.** The q-gram table indexed on the *raw low bits* of the packed block
  key. Symbol ids are small and densely numbered from 1, so a 3-gram key
  (`id0 | id1 << 8 | id2 << 16`) against a 256-slot table was indexed by `id0` alone, and every block
  starting with the same symbol shared one probe chain. Real text is exactly the case that breaks it:
  on a corpus with a handful of distinct leading graphemes, around a hundred blocks collapsed into a
  few chains, and since almost every lookup is a *miss* — and a miss must walk its chain to an empty
  slot — the scan spent its time on dependent loads. Isolated at 24 patterns, the scan cost **~60 ns
  per grapheme against ~40 ns for the entire plain fuzzy search it exists to accelerate**. Mixing the
  key with a golden-ratio multiply first drops it to **~9.6 ns**, a 6x improvement.

  This is why the pre-filter has been reading as a loss. Paired against a plain search on identical
  data, both lanes timed back to back in one process, median of 21 reps — it was a *net loss almost
  everywhere*, including the 4-pattern case its own documentation cites as the win:

  | shape | before | after |
  |---|---|---|
  | 8 patterns / sparse 96K | 1.63 | **0.18** |
  | 12 patterns / sparse 96K | 1.54 | **0.17** |
  | 4 patterns / prose 96K | 1.50 | **0.34** |
  | 30 patterns / long patterns | 1.18 | **0.066** |
  | 500 patterns / sparse 96K | 1.12 | **1.00** |
  | 30 patterns / short patterns | 1.39 | 1.03 |

  (ratio of pre-filtered to plain search; below 1.0 is the pre-filter winning. Results were identical
  to a plain search throughout — this was never a correctness problem, only a cost one.) Cumulative
  against the tree before the dedup work, median of 9 paired A/B rounds: 4 patterns on a sparse
  250 KiB corpus **0.38**, 500 patterns **0.57**.

  Answers are unaffected — a badly distributed slot still finds every key. What was untested was the
  distribution itself, and it is now
  `prefilter::tests::qgram_slots_depend_on_the_whole_block`, which fails on the old hash with "only 1
  of 100 blocks got distinct slots".

- **The fuzzy search is ~1.7x faster on a many-pattern corpus, and no slower anywhere.** The
  per-window state-dedup table is the hottest structure in the search — one hash probe per expanded
  state, ~36% of the fuzzy path on 500 patterns — and at a one-edit budget it is not needed, so it is
  now folded away at compile time the way it already was for an exact search. Median of 9 paired A/B
  rounds, instrument self-consistency median 1.00 / IQR 0.006:
  - 500 patterns, one edit: **0.56** (1.8x)
  - 200 patterns, sorted + non-overlapping: **0.66** (1.5x)
  - 4 patterns, one edit: **0.91** (1.1x)
  - exact search, and the Unicode path: unchanged

  The same invariant, applied to `State` rather than to the dedup key, removes a field: the matched
  span's *start* is always the current window's start, so it is the same for every state in a window
  and now lives in the search loop instead of being written and read on every BFS transition. That
  takes `State` from 28 to 24 bytes, and hoists the span's start byte offset out of the per-state
  report path entirely. Worth about a further 7% on the many-pattern cases.

  Two things make this safe, and they are different things. Collapsing duplicate states is a *pure
  optimisation* — two states agreeing on node, span and per-type edit counts have identical futures,
  so expanding both gives the same answers; the table changes work, never results. And the one-edit
  state space is *bounded*: at most `O(m)` ways to place a single edit along a pattern of `m` graphemes
  and then follow an exact chain, so a window's state count depends on the pattern and not on the
  haystack, and the total stays linear in the input. At a budget of two it is `O(m^2)` and at six
  `O(m^7)`, which is where the table earns its keep — measured, it collapses 0% / 0.8% / 3.4% / 3.6% /
  3.7% / 3.2% of expansions at budgets 1–6, and a ~5-cycle probe against a ~60-cycle expansion needs
  more than ~8% to pay. A single-grapheme mapping is excluded from the gate, because there a free
  mapping really does collapse a costlier substitution and picks the better alignment.
- **The pre-filter is no longer slower than a plain search on a large pattern set.** It ran one Bitap
  scan *per pattern*, so its cost was `O(text x patterns)`: with 500 patterns on a 24 KiB corpus it
  took **95 ms against 26 ms for the plain search it was supposed to accelerate** -- the documented
  ~13x was only ever true for a handful of patterns, where the per-pattern scans are cheap. The scan
  is now a single Wu-Manber pass: every pattern's blocks go into one lookup table, so the cost is
  `O(text)`. Measured on identical input, median of 11 paired rounds, instrument self-consistency
  under 1%:
  - 4 patterns, sparse corpus: **7.9 ms -> 3.4 ms**, i.e. 5.0x faster than a plain search
  - 500 patterns, sparse corpus: **95.0 ms -> 26.2 ms**, i.e. back to parity with a plain search
- The block keys are exact rather than hashed. Pattern symbol ids start at 1, so a block of up to 8
  of them packs into a `u64` with no collisions, making a lookup a load, a mask and a compare.
- The pre-filter now **declines** when it cannot pay for itself, instead of approximating: if the
  block length would be 1, or if the candidate regions end up covering as much text as a plain search
  would, it transparently runs the plain search. Results are identical either way -- a filter that
  cannot pay for itself is removed, not approximated. The 500-pattern case above is exactly this:
  `edits(1)` bounds the block at `m / 3`, and a few hundred patterns saturate the 3-gram space of a
  26-letter alphabet, so there is nothing left to reject.
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
[0.5.1]: https://github.com/kakserpom/fuzzy-aho-corasick-rs/releases/tag/v0.5.1
[0.5.0]: https://github.com/kakserpom/fuzzy-aho-corasick-rs/releases/tag/v0.5.0
