use fuzzy_aho_corasick::{FuzzyAhoCorasickBuilder, FuzzyLimits};
use std::hint::black_box;

fn main() {
    let automaton = FuzzyAhoCorasickBuilder::new()
        .fuzzy(FuzzyLimits::new().edits(1))
        .build(["tincidunt", "porta", "lorem", "ipsum"]);

    let text = "lorem ipsum dolor sit amet consectetur adipiscing elit sed do eiusmod tempor incididunt ut labore et dolore magna aliqua porta lorem ipsum dolor sit amet consectetur adipiscing elit sed do eiusmod tempor incididunt ut labore et dolore magna aliqua porta lorem ipsum dolor sit amet consectetur adipiscing elit sed do eiusmod tempor incididunt ut labore et dolore magna aliqua porta";

    // Warm up
    for _ in 0..1000 {
        let results = automaton.search_non_overlapping(text, 0.6);
        black_box(&results);
    }

    // Run for profiling
    for _ in 0..100000 {
        let results = automaton.search_non_overlapping(text, 0.6);
        black_box(&results);
    }
}
