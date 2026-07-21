//! Benchmark `replace_stream_parallel` against the single-threaded `replace_stream` and the
//! whole-input `replace`, and verify all three produce byte-identical output.
//!
//! The search is CPU-bound (a BFS from every position), so the parallel form should scale close to
//! linearly with cores until the serial output reassembly / memory bandwidth becomes the limit.
//!
//! Run: `cargo run --release --example replace_bench [input_MiB]`  (default 32)

use fuzzy_aho_corasick::{FuzzyAhoCorasickBuilder, FuzzyLimits};
use std::hint::black_box;
use std::io;
use std::time::Instant;

fn main() {
    let mib: usize = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(32);

    // Sparse-ish input: filler that cannot fuzzy-match "needle", punctuated by needles (some with a
    // one-edit typo). Matches are well separated so streaming and whole-input selection agree.
    let filler = "the quick brown fox jumps over the lazy dog ";
    let mut input = String::with_capacity(mib * 1024 * 1024);
    let mut toggle = false;
    while input.len() < mib * 1024 * 1024 {
        for _ in 0..12 {
            input.push_str(filler);
        }
        input.push_str(if toggle { "neeedle " } else { "needle " }); // alternate a typo
        toggle = !toggle;
    }
    let bytes = input.len();

    let engine = FuzzyAhoCorasickBuilder::new()
        .fuzzy(FuzzyLimits::new().edits(1))
        .case_insensitive(true)
        .build(["needle"]);

    // Cheap, allocation-free replacement (inline `|_m| Some("N")`) so we measure search +
    // reassembly, not formatting.

    // Correctness: every path must produce identical bytes.
    let whole = engine.replace(&input, |_m| Some("N"), 0.85);
    let mut st_out = Vec::with_capacity(whole.len());
    engine
        .replace_stream(input.as_bytes(), &mut st_out, |_m| Some("N"), 0.85)
        .unwrap();
    assert_eq!(st_out, whole.as_bytes(), "replace_stream != replace");
    let mut par_out = Vec::with_capacity(whole.len());
    engine
        .replace_stream_parallel(input.as_bytes(), &mut par_out, 8, |_m| Some("N"), 0.85)
        .unwrap();
    assert_eq!(
        par_out,
        whole.as_bytes(),
        "replace_stream_parallel != replace"
    );
    let match_count = whole.matches('N').count();

    let mbps = |secs: f64| bytes as f64 / 1e6 / secs;
    println!(
        "input: {} MiB, {match_count} matches, output {} MiB (writing to io::sink)",
        bytes / (1024 * 1024),
        whole.len() / (1024 * 1024),
    );

    // Whole-input replace (loads all into memory; reference only).
    let t = Instant::now();
    black_box(engine.replace(black_box(&input), |_m| Some("N"), 0.85));
    let whole_secs = t.elapsed().as_secs_f64();
    println!(
        "  replace (whole-input)      : {:8.1} MB/s",
        mbps(whole_secs)
    );

    // Single-threaded streaming.
    let t = Instant::now();
    engine
        .replace_stream(
            black_box(input.as_bytes()),
            io::sink(),
            |_m| Some("N"),
            0.85,
        )
        .unwrap();
    let st_secs = t.elapsed().as_secs_f64();
    println!(
        "  replace_stream (1 thread)  : {:8.1} MB/s  (1.0x)",
        mbps(st_secs)
    );

    // Parallel streaming across thread counts.
    let ncpu = std::thread::available_parallelism().map_or(1, |n| n.get());
    let mut counts: Vec<usize> = [1, 2, 4, 8, 16]
        .into_iter()
        .filter(|&t| t <= ncpu)
        .collect();
    if !counts.contains(&ncpu) {
        counts.push(ncpu);
    }
    for threads in counts {
        let t = Instant::now();
        engine
            .replace_stream_parallel(
                black_box(input.as_bytes()),
                io::sink(),
                threads,
                |_m| Some("N"),
                0.85,
            )
            .unwrap();
        let secs = t.elapsed().as_secs_f64();
        println!(
            "  replace_stream_parallel({threads:2}) : {:8.1} MB/s  ({:.1}x vs 1 thread)",
            mbps(secs),
            st_secs / secs,
        );
    }
}
