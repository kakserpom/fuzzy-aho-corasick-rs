# The Fuzzy Matching Model

The engine looks for each pattern starting at **every position** in the haystack, exploring the edit
operations you allow and keeping the best-scoring way to match each span. Understanding the four edit
operations and the unit of matching (grapheme clusters) explains most of the engine's behavior.

## Edit operations

A candidate match is the pattern transformed into a haystack substring by a sequence of edits:

| Operation | Meaning | Example (pattern → text) |
| --- | --- | --- |
| **Substitution** | one symbol replaced by another | `needle` → `noodle` |
| **Insertion** | an extra symbol appears in the text | `needle` → `neeedle` |
| **Deletion** | a pattern symbol is missing from the text | `needle` → `nedle` |
| **Transposition (swap)** | two adjacent symbols are swapped | `world` → `wolrd` |

Substitution, insertion, and deletion are the classic
[Levenshtein](https://en.wikipedia.org/wiki/Levenshtein_distance) edits; transposition is the extra
[Damerau](https://en.wikipedia.org/wiki/Damerau%E2%80%93Levenshtein_distance) operation, which
matters because swapped letters are one of the most common human typos.

Each operation adds a **penalty** to the candidate; substitutions add a penalty scaled by how
*similar* the two symbols are (see [Similarity](../similarity/custom.md)), and a swap is a single
operation rather than two substitutions. How much each costs is configurable — see
[Penalties](../building/penalties.md).

## Symbols are grapheme clusters

The engine operates over **grapheme clusters**, not bytes or `char`s. A grapheme cluster is what a
reader perceives as a single character: `a`, `é`, `😀`, or a base letter plus combining marks
(`e` + `◌́`). This is the right unit for human-facing text:

- `"café"` is four symbols whether the `é` is one code point or `e` + a combining accent.
- An emoji with a skin-tone modifier is one symbol, so a single edit can't tear it in half.

Pattern length `N`, which drives scoring, is measured in grapheme clusters, and edits act on whole
grapheme clusters.

## Case folding

With [`case_insensitive(true)`](../building/builder.md) the engine folds case in a Unicode-aware way
(`str::to_lowercase` per grapheme), so `Straße`, `STRASSE`-style and Greek/Cyrillic case differences
match as you'd expect. Folding is applied identically to the patterns at build time and to the
haystack at search time.

## Where matching starts and stops

Because the search restarts at every grapheme position, a pattern can be found anywhere — there is no
notion of word boundaries built in. If you only want whole-token matches, filter the results by the
surrounding characters, or use the [segmentation API](../searching/segmentation.md) to reason about
the gaps between matches.

The search is **exact by default**: with no [`fuzzy(..)`](../building/builder.md) limits, only
zero-edit matches are produced, and the engine behaves like a classic Unicode Aho–Corasick automaton.
