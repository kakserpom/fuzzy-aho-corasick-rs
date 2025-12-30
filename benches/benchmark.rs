use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use fuzzy_aho_corasick::{FuzzyAhoCorasickBuilder, FuzzyLimits};
use std::hint::black_box;

fn benchmark_search(c: &mut Criterion) {
    let automaton = FuzzyAhoCorasickBuilder::new()
        .fuzzy(FuzzyLimits::new().edits(2))
        .build(["saddam", "ddamhu"]);
    let input = "this is a saddamhu example with multiple saddam matches and ddamhu too";

    c.bench_function("search_basic", |b| {
        b.iter(|| {
            let _ = automaton.search_non_overlapping(black_box(input), 0.8);
        });
    });
}

fn benchmark_long_text(c: &mut Criterion) {
    let automaton = FuzzyAhoCorasickBuilder::new()
        .fuzzy(FuzzyLimits::new().edits(1))
        .case_insensitive(true)
        .build(["tincidunt", "porta", "lorem", "ipsum"]);

    let text = "Lorem ipsum dolor sit amet, consectetur adipiscing elit. Vestibulum eros ipsum, tincidutn eu metus ut, commodo accumsan mi. Vestibulum porta, orci nec ullamcorper posuere, eros tortor pharetra est, at porttitor mi leo a velit. Aenean sollicitudin mauris elit, ultricies congue dui vulputate in. In hac habitasse platea dictumst. Nam iaculis sagittis justo a condimentum. Curabitur sed rhoncus dolor. Lorem ipsum dolor sit amet, consectetur adipiscing elit. Vivamus egestas congue lorem, in convallis magna viverra quis.";

    c.bench_function("search_long_text", |b| {
        b.iter(|| {
            let _ = automaton.search_non_overlapping(black_box(text), 0.8);
        });
    });
}

fn benchmark_many_patterns(c: &mut Criterion) {
    let patterns: Vec<&str> = vec![
        "JOINT",
        "STOCK",
        "COMPANY",
        "LIMITED",
        "LIABILITY",
        "PUBLIC",
        "PRIVATE",
        "CORPORATION",
        "INTERNATIONAL",
        "ENTERPRISE",
        "TRADING",
        "HOLDINGS",
        "INVESTMENT",
        "CAPITAL",
        "PARTNERS",
        "ASSOCIATES",
        "SOLUTIONS",
        "INDUSTRIES",
        "TECHNOLOGIES",
        "SERVICES",
    ];

    let automaton = FuzzyAhoCorasickBuilder::new()
        .fuzzy(FuzzyLimits::new().edits(1))
        .case_insensitive(true)
        .build(patterns);

    let text = "PUBLIC JOINT STOCK COMPANY GAZPROM INTERNATIONAL HOLDINGS LIMITED LIABILITY";

    c.bench_function("search_many_patterns", |b| {
        b.iter(|| {
            let _ = automaton.search_non_overlapping(black_box(text), 0.7);
        });
    });
}

fn benchmark_fuzzy_levels(c: &mut Criterion) {
    let mut group = c.benchmark_group("fuzzy_levels");

    let text = "this is a saddamhu example with multiple saddam matches";

    for edits in [1, 2, 3] {
        let automaton = FuzzyAhoCorasickBuilder::new()
            .fuzzy(FuzzyLimits::new().edits(edits))
            .build(["saddam", "hussein"]);

        group.bench_with_input(BenchmarkId::new("edits", edits), &edits, |b, _| {
            b.iter(|| {
                let _ = automaton.search_non_overlapping(black_box(text), 0.6);
            });
        });
    }

    group.finish();
}

fn benchmark_build(c: &mut Criterion) {
    let patterns: Vec<&str> = vec![
        "JOINT",
        "STOCK",
        "COMPANY",
        "LIMITED",
        "LIABILITY",
        "PUBLIC",
        "PRIVATE",
        "CORPORATION",
        "INTERNATIONAL",
        "ENTERPRISE",
        "TRADING",
        "HOLDINGS",
        "INVESTMENT",
        "CAPITAL",
        "PARTNERS",
    ];

    c.bench_function("build_automaton", |b| {
        b.iter(|| {
            let _ = FuzzyAhoCorasickBuilder::new()
                .fuzzy(FuzzyLimits::new().edits(2))
                .case_insensitive(true)
                .build(black_box(patterns.clone()));
        });
    });
}

fn benchmark_replace(c: &mut Criterion) {
    let replacer = FuzzyAhoCorasickBuilder::new()
        .fuzzy(FuzzyLimits::new().edits(1))
        .case_insensitive(true)
        .build_replacer([
            ("PUBLIC JOINT STOCK COMPANY", "PJSC"),
            ("LIMITED LIABILITY COMPANY", "LLC"),
            ("JOINT STOCK COMPANY", "JSC"),
        ]);

    let text = "PUBLIC JOINT STOCK COMPANY GAZPROM AND LIMITED LIABILITY COMPANY ROSNEFT";

    c.bench_function("replace", |b| {
        b.iter(|| {
            let _ = replacer.replace(black_box(text), 0.8);
        });
    });
}

fn benchmark_beam_search(c: &mut Criterion) {
    let mut group = c.benchmark_group("beam_search");

    // Longer text with fuzzy matches to stress test state explosion
    let text = "Lorem ipsum dolor sit amet, consectetur adipiscing elit. saddamhussein tincidutn porta vestibulum eros ipsum accumsan mi. portta commodo vestibuulum orci nec.";

    // Very high edit count to stress test - this causes exponential state explosion
    let automaton_no_beam = FuzzyAhoCorasickBuilder::new()
        .fuzzy(FuzzyLimits::new().edits(4))
        .case_insensitive(true)
        .build([
            "saddam",
            "hussein",
            "tincidunt",
            "porta",
            "vestibulum",
            "accumsan",
        ]);

    let automaton_beam_500 = FuzzyAhoCorasickBuilder::new()
        .fuzzy(FuzzyLimits::new().edits(4))
        .case_insensitive(true)
        .beam_width(500)
        .build([
            "saddam",
            "hussein",
            "tincidunt",
            "porta",
            "vestibulum",
            "accumsan",
        ]);

    let automaton_beam_100 = FuzzyAhoCorasickBuilder::new()
        .fuzzy(FuzzyLimits::new().edits(4))
        .case_insensitive(true)
        .beam_width(100)
        .build([
            "saddam",
            "hussein",
            "tincidunt",
            "porta",
            "vestibulum",
            "accumsan",
        ]);

    group.bench_function("no_beam_edits4", |b| {
        b.iter(|| {
            let _ = automaton_no_beam.search_non_overlapping(black_box(text), 0.5);
        });
    });

    group.bench_function("beam_500_edits4", |b| {
        b.iter(|| {
            let _ = automaton_beam_500.search_non_overlapping(black_box(text), 0.5);
        });
    });

    group.bench_function("beam_100_edits4", |b| {
        b.iter(|| {
            let _ = automaton_beam_100.search_non_overlapping(black_box(text), 0.5);
        });
    });

    group.finish();
}

criterion_group!(
    benches,
    benchmark_search,
    benchmark_long_text,
    benchmark_many_patterns,
    benchmark_fuzzy_levels,
    benchmark_build,
    benchmark_replace,
    benchmark_beam_search
);
criterion_main!(benches);
