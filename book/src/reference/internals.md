# How It Works

A high-level tour of the engine's internals, for the curious and for anyone tuning or contributing.

## The automaton

At build time the patterns are compiled into a trie (an Aho–Corasick automaton) over grapheme
clusters: each node is a pattern prefix, edges are grapheme transitions, and terminal nodes carry the
indices of the patterns that end there. Failure links and per-node metadata are precomputed so the
search never needs to mutate the automaton.

The engine is fully **immutable** after `build`, which is why it is cheap to share across threads
(`&FuzzyAhoCorasick`) and why every search allocates only transient per-call state.

## The fuzzy search

Fuzzy matching is a breadth-first exploration. Conceptually, a *state* is "we are at automaton node
`n`, having consumed up to haystack position `j`, with these accumulated edit counts and penalty". From
each state the search branches:

- **exact transition** — consume the matching grapheme, no penalty (an O(1) map lookup),
- **substitution** — consume a different grapheme, penalty scaled by [similarity](../similarity/custom.md),
- **insertion** — skip a haystack grapheme,
- **deletion** — advance in the pattern without consuming input,
- **transposition** — consume two adjacent graphemes swapped.

The search restarts from every grapheme position, which is what makes it a *multi-pattern, match-
anywhere* fuzzy search rather than a single alignment.

## Why it stays fast

Several mechanisms keep the exponential-looking exploration in check:

- **State deduplication.** Insertions and deletions can reach the same automaton position by
  exponentially many paths; a visited-set collapses states that agree on position, span, and per-type
  edit counts down to the lowest-penalty representative.
- **Pruning ceilings.** Each node stores coefficients for the best score still reachable through it,
  so a state whose penalty already exceeds what any reachable pattern could tolerate — at the current
  threshold — is dropped along with its entire subtree.
- **Push-time guards.** Cheap penalty checks reject an edit before a state is even enqueued.
- **Compact state.** Node indices and grapheme positions are `u32` and the four edit counts pack into
  a single word, keeping the per-state footprint small and cache-dense.
- **[Beam / auto-beam](../performance/bounding.md).** Optional caps bound the frontier for
  pathological inputs.

## Scoring

When the search reaches a terminal node within the [edit limits](../building/builder.md), it computes
`similarity = (N − penalties) / N × weight` and keeps the candidate if it clears the
[threshold](../concepts/scoring.md), retaining the best score per `(start, end, pattern)` span. The
various [search entry points](../searching/search.md) then sort and resolve overlaps.

## Streaming and the pre-filter

- **[Streaming](../streaming/search.md)** cuts the input into bounded, overlapping windows. The
  overlap equals the longest possible match (`max_match_graphemes()`), so no match is split, and each
  window owns the matches starting in its non-overlap prefix — giving exactly-once emission with no
  cross-window deduplication.
- **[The pre-filter](../performance/prefilter.md)** is a separate bit-parallel automaton (one machine
  word of NFA states advanced per input symbol) used only to locate candidate regions; the real
  engine described above still produces the results.

The [API documentation](https://docs.rs/fuzzy-aho-corasick) covers the concrete types; the source is
extensively commented if you want the exact recurrences.
