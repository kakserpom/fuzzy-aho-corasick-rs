# Streaming Replace

`replace_stream` is the streaming counterpart of [`replace`](../searching/replacement.md): it reads
from a [`Read`], writes the transformed stream to a [`Write`] in **constant memory**, substituting
matches as they are found and copying everything else through verbatim. It returns the number of bytes
written.

```rust
use fuzzy_aho_corasick::{FuzzyAhoCorasickBuilder, FuzzyLimits};

let engine = FuzzyAhoCorasickBuilder::new()
    .fuzzy(FuzzyLimits::new().edits(1))
    .case_insensitive(true)
    .build(["needle"]);

let mut out = Vec::new();
// "neeedle" has one extra 'e' (an insertion); it is replaced, the rest copied through.
engine.replace_stream("a neeedle b".as_bytes(), &mut out, |_m| Some("X"), 0.8).unwrap();
assert_eq!(String::from_utf8(out).unwrap(), "a X b");
```

The `FuzzyReplacer` turnkey form uses its configured `(pattern → replacement)` table:

```rust
# use fuzzy_aho_corasick::{FuzzyAhoCorasickBuilder, FuzzyLimits};
let replacer = FuzzyAhoCorasickBuilder::new()
    .case_insensitive(true)
    .fuzzy(FuzzyLimits::new().edits(1))
    .build_replacer([("hello", "hi"), ("world", "earth")]);

let mut out = Vec::new();
replacer.replace_stream("hell0 w0rld!".as_bytes(), &mut out, 0.8).unwrap();
assert_eq!(String::from_utf8(out).unwrap(), "hi earth!");
```

## Semantics and limitations

- **Per-window selection.** Matches are chosen per window (as in the streaming search), so at a
  window boundary overlaps are resolved left-to-right — the earlier-starting match wins — rather than
  by the global ranking a whole-input [`replace`](../searching/replacement.md) uses. For inputs where
  matches are separated by non-matching text, the two agree exactly.
- **Replacement can't borrow the match.** The replacement type is independent of the match, so it may
  borrow external data (e.g. a replacement table) but not the transient matched text. Return an owned
  `String` if you need to derive the replacement from `m.text`.
- **Buffer the writer.** Wrap the writer in a [`BufWriter`] for throughput.

## Parallel replace

`replace_stream_parallel(reader, writer, threads, callback, threshold)` fans the CPU-bound search
across a thread pool while reassembling the output **in stream order** on the calling thread. Because
output is inherently ordered, only the search is parallelized — the callback and writer stay on the
calling thread (no `Send`/`Sync` bounds), and the result is **byte-identical** to `replace_stream`.

```rust
# use fuzzy_aho_corasick::{FuzzyAhoCorasickBuilder, FuzzyLimits};
# let engine = FuzzyAhoCorasickBuilder::new().fuzzy(FuzzyLimits::new().edits(1)).build(["needle"]);
let mut out = Vec::new();
let threads = std::thread::available_parallelism().map_or(1, |n| n.get());
engine.replace_stream_parallel("a needle b".as_bytes(), &mut out, threads, |_m| Some("X"), 0.8).unwrap();
assert_eq!(String::from_utf8(out).unwrap(), "a X b");
```

On a 10-core machine this scales roughly 1.9× / 3.7× / 6.3× at 2 / 4 / 8 threads on a 32 MiB input —
near-linear until the serial output reassembly and memory bandwidth take over. At one thread it
matches the single-threaded form exactly, so there's no penalty for using it when the input turns out
small. See [`examples/replace_bench.rs`](https://github.com/kakserpom/fuzzy-aho-corasick-rs/blob/master/examples/replace_bench.rs).

[`Read`]: https://doc.rust-lang.org/std/io/trait.Read.html
[`Write`]: https://doc.rust-lang.org/std/io/trait.Write.html
[`BufWriter`]: https://doc.rust-lang.org/std/io/struct.BufWriter.html
