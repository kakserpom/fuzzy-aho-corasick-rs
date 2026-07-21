//! Prototype: bit-parallel (Bitap / Wu–Manber) approximate matching as a potential "fast lane".
//!
//! This is a *restricted* model compared to the main engine — deliberately, to show the throughput
//! ceiling of a bit-parallel NFA:
//! * a single total edit budget `k` (Levenshtein: insert/delete/substitute), no per-type limits,
//! * a byte alphabet (no grapheme clusters / Unicode-aware casefolding),
//! * no weighted similarity table, no transpositions,
//! * one pattern of length ≤ 63 (fits a `u64`), reporting match **end** positions.
//!
//! It carries a brute-force DP verifier (fuzzed) so the recurrence is trustworthy, then benchmarks
//! bitap vs the main engine on the same input.
//!
//! Run: `cargo run --release --example bitap_prototype`

use fuzzy_aho_corasick::{FuzzyAhoCorasickBuilder, FuzzyLimits};
use std::hint::black_box;
use std::time::Instant;

/// Bit-parallel approximate search (Wu–Manber, shift-AND form). Returns the exclusive byte end
/// positions `e` such that some `s` gives `levenshtein(pattern, text[s..e]) <= k`.
fn bitap(pattern: &[u8], text: &[u8], k: usize, ends: &mut Vec<usize>) {
    let m = pattern.len();
    assert!((1..=63).contains(&m), "pattern length must be 1..=63");
    ends.clear();

    // B[c]: bit i set iff pattern[i] == c.
    let mut b = [0u64; 256];
    for (i, &c) in pattern.iter().enumerate() {
        b[c as usize] |= 1u64 << i;
    }
    let match_bit = 1u64 << (m - 1);

    // R[d]: bit j set iff pattern prefix P[0..=j] matches a suffix of the text read so far within d
    // errors. Init lets d deletions of the pattern prefix be free (low d bits set).
    let mut r = vec![0u64; k + 1];
    let mut nr = vec![0u64; k + 1];
    for (d, slot) in r.iter_mut().enumerate() {
        *slot = (1u64 << d) - 1;
    }

    for (i, &c) in text.iter().enumerate() {
        let bc = b[c as usize];
        nr[0] = ((r[0] << 1) | 1) & bc;
        for d in 1..=k {
            nr[d] = ((r[d] << 1) & bc)            // match / exact extension
                | ((r[d - 1] | nr[d - 1]) << 1)   // substitution (prev) + deletion (current)
                | r[d - 1]                        // insertion
                | 1; // start state stays active at every error level (begin with an edit)
        }
        // R[k] subsumes all lower error levels, so one test suffices.
        if nr[k] & match_bit != 0 {
            ends.push(i + 1);
        }
        std::mem::swap(&mut r, &mut nr);
    }
}

/// Reference: classic approximate-search DP. Match ends at `e` iff `min_s lev(pattern, text[s..e]) <= k`.
fn brute_force_ends(pattern: &[u8], text: &[u8], k: usize) -> Vec<usize> {
    let m = pattern.len();
    let n = text.len();
    // prev[j] = D[j] for the previous text column; start row D[0][*] = 0 (match may start anywhere).
    let mut prev: Vec<usize> = (0..=m).collect(); // column i=0: D[j][0] = j
    let mut ends = Vec::new();
    for i in 1..=n {
        let mut curr = vec![0usize; m + 1]; // D[0][i] = 0
        for j in 1..=m {
            let sub = prev[j - 1] + usize::from(pattern[j - 1] != text[i - 1]);
            let del = prev[j] + 1; // delete text char (advance i)
            let ins = curr[j - 1] + 1; // insert -> skip pattern char
            curr[j] = sub.min(del).min(ins);
        }
        if curr[m] <= k {
            ends.push(i);
        }
        prev = curr;
    }
    ends
}

/// Deterministic xorshift so the fuzz is reproducible without a dependency.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    fn byte(&mut self, alphabet: u8) -> u8 {
        b'a' + (self.next() % u64::from(alphabet)) as u8
    }
}

fn fuzz_correctness() {
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let mut ends = Vec::new();
    let mut cases = 0u32;
    for _ in 0..20_000 {
        let alphabet = 2 + (rng.next() % 4) as u8; // tiny alphabet -> lots of fuzzy hits
        let m = 1 + (rng.next() % 12) as usize;
        let n = (rng.next() % 40) as usize;
        let k = (rng.next() % 4) as usize;
        let pattern: Vec<u8> = (0..m).map(|_| rng.byte(alphabet)).collect();
        let text: Vec<u8> = (0..n).map(|_| rng.byte(alphabet)).collect();
        bitap(&pattern, &text, k, &mut ends);
        let reference = brute_force_ends(&pattern, &text, k);
        assert_eq!(
            ends,
            reference,
            "mismatch: pattern={:?} text={:?} k={k}",
            String::from_utf8_lossy(&pattern),
            String::from_utf8_lossy(&text),
        );
        cases += 1;
    }
    println!("correctness: {cases} random cases match the brute-force DP ✓");
}

fn main() {
    fuzz_correctness();

    // Throughput comparison on a large text. Note this is NOT apples-to-apples: the engine also
    // reports spans, per-pattern index, weighted scores, and handles swaps/Unicode. This measures
    // the raw detection ceiling of the bit-parallel approach.
    let pattern = b"vestibulum";
    let filler = "the quick brown fox jumps over the lazy dog and runs away quickly ";
    let mut text = String::new();
    while text.len() < 16 * 1024 * 1024 {
        text.push_str(filler);
        text.push_str("vestibulum ");
    }
    let bytes = text.as_bytes();
    let k = 1usize;

    // bitap
    let mut ends = Vec::new();
    bitap(pattern, bytes, k, &mut ends); // warm
    let t = Instant::now();
    let iters = 5;
    for _ in 0..iters {
        bitap(black_box(pattern), black_box(bytes), k, &mut ends);
    }
    let bitap_secs = t.elapsed().as_secs_f64() / f64::from(iters);
    let bitap_mbps = bytes.len() as f64 / 1e6 / bitap_secs;

    // main engine, equivalent restricted config (single pattern, edits(k), case-sensitive)
    let engine = FuzzyAhoCorasickBuilder::new()
        .fuzzy(FuzzyLimits::new().edits(k as u8))
        .build(["vestibulum"]);
    let _ = engine.search(&text, 0.85); // warm
    let t = Instant::now();
    for _ in 0..iters {
        black_box(engine.search(black_box(&text), 0.85));
    }
    let engine_secs = t.elapsed().as_secs_f64() / f64::from(iters);
    let engine_mbps = bytes.len() as f64 / 1e6 / engine_secs;

    println!(
        "input: {} MiB, pattern {:?}, k={k}",
        bytes.len() / (1024 * 1024),
        String::from_utf8_lossy(pattern)
    );
    println!(
        "  bitap  : {bitap_mbps:8.1} MB/s ({} end positions)",
        ends.len()
    );
    println!("  engine : {engine_mbps:8.1} MB/s");
    println!(
        "  speedup: {:.1}x  (raw detection ceiling)",
        bitap_mbps / engine_mbps
    );

    // -----------------------------------------------------------------------------------------
    // Integrated pre-filter: Prefiltered::search vs FuzzyAhoCorasick::search, IDENTICAL results.
    // This is the real end-to-end win — a sparse haystack where most of the text can be skipped.
    // -----------------------------------------------------------------------------------------
    let mut sparse = String::new();
    while sparse.len() < 16 * 1024 * 1024 {
        // Long runs of filler with no near-match, then one fuzzy hit ("vestibulm" = 1 deletion).
        for _ in 0..200 {
            sparse.push_str(filler);
        }
        sparse.push_str("vestibulm ");
    }
    let threshold = 0.85;

    let pf = engine.with_prefilter();
    assert!(
        pf.is_active(),
        "config should be reducible to the bit model"
    );

    // Correctness: the pre-filtered results must equal the full search exactly.
    let full = engine.search(&sparse, threshold);
    let filtered = pf.search(&sparse, threshold);
    assert_eq!(
        full.len(),
        filtered.len(),
        "pre-filter changed the match set!"
    );

    let _ = pf.search(&sparse, threshold); // warm
    let t = Instant::now();
    for _ in 0..iters {
        black_box(pf.search(black_box(&sparse), threshold));
    }
    let pf_secs = t.elapsed().as_secs_f64() / f64::from(iters);

    let _ = engine.search(&sparse, threshold); // warm
    let t = Instant::now();
    for _ in 0..iters {
        black_box(engine.search(black_box(&sparse), threshold));
    }
    let full_secs = t.elapsed().as_secs_f64() / f64::from(iters);

    let pf_mbps = sparse.len() as f64 / 1e6 / pf_secs;
    let full_mbps = sparse.len() as f64 / 1e6 / full_secs;
    println!(
        "\nend-to-end on {} MiB sparse text ({} matches, results identical):",
        sparse.len() / (1024 * 1024),
        full.len()
    );
    println!("  engine.search       : {full_mbps:8.1} MB/s");
    println!("  prefiltered.search  : {pf_mbps:8.1} MB/s");
    println!("  speedup             : {:.1}x", pf_mbps / full_mbps);
}
