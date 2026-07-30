use fuzzy_aho_corasick::{FuzzyAhoCorasickBuilder, FuzzyLimits, SearchOptions};
use std::hint::black_box;
use std::time::Instant;

fn main() {
    let patterns = vec!["hello", "world", "rust", "safety"];
    let fac = FuzzyAhoCorasickBuilder::new()
        .fuzzy(FuzzyLimits::new().edits(2))
        .case_insensitive(true)
        .build(patterns);

    let text = "hell world rst safety helo wold savefty hell rust";
    // Warmup
    for _ in 0..1000 {
        let _ = fac.search(
            black_box(text),
            &SearchOptions::new()
                .threshold(0.8)
                .sorted()
                .non_overlapping(),
        );
    }

    let rounds = 20;
    let iters = 100_000;
    let mut best = u128::MAX;
    for _ in 0..rounds {
        let start = Instant::now();
        for _ in 0..iters {
            let _ = fac.search(
                black_box(text),
                &SearchOptions::new()
                    .threshold(0.8)
                    .sorted()
                    .non_overlapping(),
            );
        }
        let elapsed = start.elapsed().as_nanos();
        if elapsed < best {
            best = elapsed;
        }
    }
    let ns_per_call = best / iters;
    let us_per_call = ns_per_call as f64 / 1000.0;
    println!("micro_bench: best of {rounds} rounds, {iters} iters/round");
    println!("  total: {best} ns");
    println!("  per call: {ns_per_call} ns ({us_per_call:.1} µs)");
}
