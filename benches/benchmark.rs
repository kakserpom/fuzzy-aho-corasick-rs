use criterion::{Criterion, black_box, criterion_group, criterion_main};
use fuzzy_aho_corasick::{FuzzyAhoCorasick, FuzzyAhoCorasickBuilder, FuzzyLimits};

fn benchmark_search(c: &mut Criterion) {
    let automaton = FuzzyAhoCorasickBuilder::new()
        .fuzzy(FuzzyLimits::new().edits(2))
        .non_overlapping(true)
        .build(["saddam", "ddamhu"]);
    let input = "this is a saddamhu example with multiple saddam matches and ddamhu too";

    c.bench_function("search", |b| {
        b.iter(|| {
            let _ = automaton.search(black_box(input), 0.8);
        });
    });
}

criterion_group!(benches, benchmark_search);
criterion_main!(benches);
