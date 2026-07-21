# fuzzy-aho-corasick

A high-performance, Unicode-aware, safe-Rust implementation of the
[Aho–Corasick](https://en.wikipedia.org/wiki/Aho%E2%80%93Corasick_algorithm) automaton extended with
**fuzzy matching** — insertions, deletions, substitutions, and transpositions — over grapheme
clusters, with optional case-insensitive folding.

It answers the question *"where in this text do any of my patterns appear, allowing for a few
typos?"* — the kind of problem that shows up in entity/name matching (e.g. AML screening), OCR
cleanup, search-as-you-type, and log/stream scanning.

## What it does

- **Exact and fuzzy multi-pattern matching** in one pass over a shared trie, with Levenshtein-style
  edits plus transposition.
- **Unicode correctness**: it matches over grapheme clusters, not bytes or code points, and folds
  case in a Unicode-aware way.
- **Tunable scoring**: per-edit-type penalties, a character similarity table, per-pattern weights,
  and a similarity threshold decide what counts as a match and how matches rank.
- **Rich output control**: raw matches, several ranking strategies, non-overlapping selection,
  segmentation, splitting, and find-and-replace.
- **Scales to streams**: search or replace over any `Read`/`Write` in constant memory, single- or
  multi-threaded.
- **Optional fast lane**: an opt-in bit-parallel pre-filter skips regions that provably cannot
  match, for a large speedup on big, sparse inputs — with identical results.

## How to read this book

- **Getting Started** gets you matching in a couple of minutes.
- **Core Concepts** explains the edit model and how scores are computed — read this once and the
  rest of the API makes sense.
- **Building an Engine**, **Searching**, and **Similarity** cover the configuration surface.
- **Streaming** and **Performance** are for larger or latency-sensitive workloads.
- **Reference** describes how the engine works internally and credits the underlying research.

The crate's [API documentation on docs.rs](https://docs.rs/fuzzy-aho-corasick) is the authoritative
reference for every type and method; this book is the narrative guide.
