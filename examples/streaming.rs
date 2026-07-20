//! Streaming fuzzy search over an input larger than 4 GiB, using the library's streaming API.
//!
//! A single [`FuzzyAhoCorasick::search`] call caps the haystack at ~4 GiB (grapheme positions are
//! `u32`). The [`search_stream_parallel`](fuzzy_aho_corasick::FuzzyAhoCorasick::search_stream_parallel)
//! API streams input of any size in bounded, overlapping windows across a thread pool, reporting
//! matches with absolute `u64` offsets. (See also `search_stream` for the single-threaded callback
//! form and `stream_matches` for an iterator.)
//!
//! This example feeds it a synthetic multi-GiB stream and confirms matches are reported past
//! `u32::MAX` with correct offsets, wrapping the reader to drive an `indicatif` progress bar.
//!
//! Usage: `cargo run --release --example streaming -- [SIZE_GiB] [THREADS]`
//! ```text
//! cargo run --release --example streaming             # ~64 MiB demo, all cores
//! cargo run --release --example streaming -- 4.2      # stream past 4 GiB
//! cargo run --release --example streaming -- 0.125 1  # 128 MiB, single-threaded
//! ```

use fuzzy_aho_corasick::{FuzzyAhoCorasickBuilder, FuzzyLimits};
use indicatif::{ProgressBar, ProgressStyle};
use std::io::{self, Read};
use std::time::Instant;

/// Wraps a reader and advances a progress bar by the number of bytes read. The producer reads
/// slightly ahead of the workers (bounded by the internal channel), so this tracks progress to
/// within a few windows — plenty accurate for a bar.
struct ProgressReader<R> {
    inner: R,
    pb: ProgressBar,
}

impl<R: Read> Read for ProgressReader<R> {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(out)?;
        self.pb.inc(n as u64);
        Ok(n)
    }
}

/// A `Read`er that fabricates an arbitrarily large stream by repeating a ~1 MiB block. Each block
/// contains exactly one needle (`vestibulum`) surrounded by whitespace, so matches land at
/// predictable, ever-increasing offsets — including past `u32::MAX` once we cross 4 GiB.
struct SyntheticStream {
    block: Vec<u8>,
    pos: usize,
    remaining: u64,
}

impl SyntheticStream {
    fn new(total_bytes: u64) -> Self {
        let filler = "the quick brown fox jumps over the lazy dog and runs away quickly ";
        let mut block = String::new();
        while block.len() < 512 * 1024 {
            block.push_str(filler);
        }
        block.push_str("vestibulum "); // the needle, mid-block, whitespace-delimited
        while block.len() < 1024 * 1024 {
            block.push_str(filler);
        }
        Self {
            block: block.into_bytes(),
            pos: 0,
            remaining: total_bytes,
        }
    }
}

impl Read for SyntheticStream {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if self.remaining == 0 {
            return Ok(0);
        }
        let n = out
            .len()
            .min(self.block.len() - self.pos)
            .min(self.remaining as usize);
        out[..n].copy_from_slice(&self.block[self.pos..self.pos + n]);
        self.pos = (self.pos + n) % self.block.len();
        self.remaining -= n as u64;
        Ok(n)
    }
}

fn main() {
    // Args: [1] target size in GiB (default ~64 MiB); [2] thread count (default: all cores).
    let gib: f64 = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(1.);
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let total_bytes = (gib * 1024. * 1024. * 1024.) as u64;
    let threads: usize = std::env::args()
        .nth(2)
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(|| std::thread::available_parallelism().map_or(1, std::num::NonZero::get));

    let engine = FuzzyAhoCorasickBuilder::new()
        .case_insensitive(true)
        .fuzzy(FuzzyLimits::new().edits(1))
        .build(["vestibulum", "accumsan"]);

    println!(
        "streaming {gib:.4} GiB ({total_bytes} bytes) on {threads} thread(s), \
         overlap={} graphemes, u32::MAX = {}",
        engine.max_match_graphemes(),
        u64::from(u32::MAX)
    );

    let pb = ProgressBar::new(total_bytes);
    pb.set_style(
        ProgressStyle::with_template(
            "{bar:40.cyan/blue} {bytes:>10}/{total_bytes} ({bytes_per_sec}, ETA {eta})",
        )
        .unwrap()
        .progress_chars("=> "),
    );
    let reader = ProgressReader {
        inner: SyntheticStream::new(total_bytes),
        pb: pb.clone(), // clone shares the same underlying bar (Arc-backed)
    };

    let mut matches: u64 = 0;
    let mut max_end: u64 = 0;
    let mut first_past_u32: Option<u64> = None;
    let start = Instant::now();
    let read = engine
        .search_stream_parallel(reader, 0.85, threads, |m| {
            matches += 1;
            max_end = max_end.max(m.end);
            if m.start > u64::from(u32::MAX) && first_past_u32.is_none() {
                first_past_u32 = Some(m.start);
            }
        })
        .expect("stream");
    pb.finish_and_clear();
    let secs = start.elapsed().as_secs_f64();

    println!(
        "done: read {read} bytes in {secs:.1}s (~{:.1} MB/s), {matches} matches, highest end offset {max_end}",
        read as f64 / 1e6 / secs
    );
    if let Some(off) = first_past_u32 {
        println!("first match beyond u32::MAX at byte offset {off} ✓ (>4 GiB streaming confirmed)");
    } else if read > u64::from(u32::MAX) {
        println!("streamed past 4 GiB but no match landed there");
    }

    // Sanity: one needle per ~1 MiB block.
    let expected = total_bytes / (1024 * 1024);
    assert!(
        matches + 2 >= expected && matches <= expected + 2,
        "expected ~{expected} needles, found {matches}"
    );
    if read > u64::from(u32::MAX) {
        assert!(
            max_end > u64::from(u32::MAX),
            "offsets must exceed u32::MAX past 4 GiB"
        );
    }
}
