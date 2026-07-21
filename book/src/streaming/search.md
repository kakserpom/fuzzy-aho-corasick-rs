# Streaming Search

A single [`search`](../searching/search.md) call loads the whole haystack into memory and is limited
to inputs under ~4 GiB (grapheme positions are `u32` internally). The streaming API instead consumes
a [`Read`] source incrementally in bounded, overlapping windows, so it runs in **constant memory**
regardless of input size — files, sockets, pipes, decompressors — and reports matches at absolute
`u64` byte offsets.

Windows overlap by the longest possible match (computed automatically from the patterns and edit
limits), so a match spanning a window boundary is never split, and each window "owns" the matches
whose start falls in its non-overlap prefix — every match is emitted exactly once, with no
deduplication on your side.

## Three entry points

All yield [`StreamMatch`], which *owns* its matched `text` (so it is `Send` and outlives the transient
window):

```rust
use fuzzy_aho_corasick::{FuzzyAhoCorasickBuilder, FuzzyLimits};
use std::fs::File;

let engine = FuzzyAhoCorasickBuilder::new()
    .fuzzy(FuzzyLimits::new().edits(1))
    .case_insensitive(true)
    .build(["needle"]);

// 1) Callback (single-threaded)
engine.search_stream(File::open("huge.txt")?, 0.8, |m| {
    println!("{}..{}: pattern #{} ({:.2})", m.start, m.end, m.pattern_index, m.similarity);
})?;

// 2) Iterator — lazy; windows are read and searched on demand
for m in engine.stream_matches(File::open("huge.txt")?, 0.8) {
    let m = m?; // io::Result<StreamMatch>
    println!("{}..{}", m.start, m.end);
}

// 3) Parallel — fans windows across a thread pool (dependency-free std::thread)
let threads = std::thread::available_parallelism().map_or(1, |n| n.get());
engine.search_stream_parallel(File::open("huge.txt")?, 0.8, threads, |m| { /* ... */ })?;
# Ok::<(), std::io::Error>(())
```

| Method | Shape | Threading |
| --- | --- | --- |
| `search_stream` | callback | single-threaded |
| `stream_matches` | `Iterator<Item = io::Result<StreamMatch>>` | single-threaded, lazy |
| `search_stream_parallel` | callback | producer + worker pool |

## Going fast

The search is **CPU-bound** — a BFS from every position, roughly independent of window size — so the
parallel form is how you get throughput: windows are independent and share the immutable engine,
scaling close to linearly with cores. In `search_stream_parallel`, `on_match` is invoked on the
calling thread as results arrive (in arbitrary order), so it needs no synchronization.

`max_match_graphemes()` exposes the auto-computed overlap if you'd rather window the input yourself.
See [`examples/streaming.rs`](https://github.com/kakserpom/fuzzy-aho-corasick-rs/blob/master/examples/streaming.rs)
for a full multi-GiB demo with a progress bar.

## Errors

The callback forms return `io::Result<u64>` (the total bytes read), propagating any reader error. The
iterator yields one `Err` if the reader fails, after which iteration ends.

[`Read`]: https://doc.rust-lang.org/std/io/trait.Read.html
[`StreamMatch`]: https://docs.rs/fuzzy-aho-corasick/latest/fuzzy_aho_corasick/struct.StreamMatch.html
