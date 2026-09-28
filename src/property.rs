/* -------------------------------------------------------------------------
 *  Property test: the real search vs a brute-force reference
 *
 *  The engine's BFS prunes aggressively -- per-node penalty ceilings, push-time global ceilings,
 *  dead-end filters, a per-window state-dedup table. Every one of those is a proof obligation, and a
 *  wrong bound does not fail loudly: it just stops reporting matches. These tests compare the engine
 *  against `reference::reference_search`, which enumerates every alignment with no pruning at all,
 *  over many randomly generated configurations.
 * ---------------------------------------------------------------------- */
use crate::reference::{RefConfig, RefPattern, RefPenalties, reference_search};
use crate::{
    FuzzyAhoCorasick, FuzzyAhoCorasickBuilder, FuzzyLimits, FuzzyPenalties, Pattern, SearchOptions,
    Similarity,
};
use std::collections::HashMap;

/// The similarity pairs, as one source of truth for both the engine's table and the reference's, so
/// the two cannot disagree about what "similar" means. Values are deliberately not all equal, so
/// substitution costs differ per pair and the cheapest-alignment bookkeeping is exercised.
const SIMILARITY_PAIRS: &[((char, char), f32)] = &[
    (('a', 'e'), 0.5),
    (('e', 'a'), 0.5),
    (('a', 'b'), 0.5),
    (('b', 'a'), 0.5),
    (('c', 'd'), 0.5),
    (('d', 'c'), 0.5),
    (('x', 'y'), 0.75),
    (('y', 'x'), 0.75),
];

/// Canonical form of the engine's matches: `(start, end, pattern_index, similarity_bits)`, sorted.
///
/// Only the span and the score are compared. The engine also reports per-type edit counts, and
/// those depend on which of several equally-cheap alignments its BFS reaches first, so they are not
/// part of the contract under test.
fn engine_matches(
    engine: &FuzzyAhoCorasick,
    text: &str,
    threshold: f32,
) -> Vec<(usize, usize, usize, u32)> {
    let mut out: Vec<(usize, usize, usize, u32)> = engine
        .search(text, &SearchOptions::new().threshold(threshold))
        .expect("haystack is small")
        .iter()
        .map(|m| (m.start, m.end, m.pattern_index, m.similarity.to_bits()))
        .collect();
    out.sort_unstable();
    out
}

/// Deterministic PRNG, so a failure is reproducible from the printed case number.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0 >> 33
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

/// The main sweep: random patterns, text, edit budget, threshold and case-sensitivity, each case
/// compared against the unpruned reference.
#[test]
fn search_matches_brute_force_reference() {
    // A small alphabet keeps matches dense, giving the search plenty of chances to prune something
    // it should have kept.
    const ALPHABET: &[char] = &['a', 'b', 'c', 'd', 'e', 'x', 'y'];
    const CASES: u32 = 500;

    let sim_map: HashMap<(char, char), f32> = SIMILARITY_PAIRS.iter().copied().collect();
    let similarity: &'static Similarity =
        Box::leak(Box::new(Similarity::from_map(sim_map.clone())));

    let penalties = FuzzyPenalties::default();
    let ref_pen = RefPenalties {
        substitution: penalties.substitution,
        insertion: penalties.insertion,
        deletion: penalties.deletion,
        swap: penalties.swap,
    };
    let config = |max_edits: u8, threshold: f32, case_insensitive: bool| RefConfig {
        pen: ref_pen,
        min_symbol_similarity: 0.0,
        max_edits,
        threshold,
        case_insensitive,
    };

    let mut rng = Rng(0x5EED_1234_ABCD_0001);
    for case in 0..CASES {
        let max_edits = rng.below(3) as u8;
        let threshold = [0.0f32, 0.5, 0.7, 0.85, 1.0][rng.below(5)];
        let case_insensitive = rng.below(2) == 0;
        let n_patterns = 1 + rng.below(4);
        let n_words = 1 + rng.below(6);
        let word_len = 1 + rng.below(4);

        let patterns: Vec<String> = (0..n_patterns)
            .map(|_| {
                (0..word_len)
                    .map(|_| ALPHABET[rng.below(ALPHABET.len())])
                    .collect()
            })
            .collect();
        let mut text = String::new();
        for _ in 0..n_words {
            for _ in 0..word_len {
                text.push(ALPHABET[rng.below(ALPHABET.len())]);
            }
            text.push(' ');
        }

        let engine = FuzzyAhoCorasickBuilder::new()
            .similarity(similarity)
            .penalties(penalties.clone())
            .case_insensitive(case_insensitive)
            .fuzzy(FuzzyLimits::new().edits(max_edits))
            .build(
                patterns
                    .iter()
                    .map(|p| Pattern::from(p.as_str()))
                    .collect::<Vec<_>>(),
            );

        let refs: Vec<RefPattern<'_>> = patterns
            .iter()
            .map(|p| RefPattern {
                text: p.as_str(),
                weight: 1.0,
            })
            .collect();
        let expected = reference_search(
            &text,
            &refs,
            &sim_map,
            config(max_edits, threshold, case_insensitive),
        );
        let got = engine_matches(&engine, &text, threshold);

        assert_eq!(
            got, expected,
            "case {case}: max_edits={max_edits} threshold={threshold} \
             case_insensitive={case_insensitive} patterns={patterns:?} text={text:?}"
        );
    }
}

/// Regression: the dedup key once omitted `matched_end`, on a false invariant.
///
/// For pattern `"yx"` on `"ybyyya"` at `edits(2)`, two alignments reach the same automaton node
/// with the same `j`, the same matched start, the same edit counts and the same penalty, yet report
/// different spans: `E I S` ends at 6, `E S I` at 5. Keying on the other four fields conflated
/// them, and one match vanished. Found by the sweep above, at case 121.
#[test]
fn equal_penalty_alignments_with_different_span_ends_both_survive() {
    let sim_map: HashMap<(char, char), f32> = SIMILARITY_PAIRS.iter().copied().collect();
    let similarity: &'static Similarity =
        Box::leak(Box::new(Similarity::from_map(sim_map.clone())));
    let engine = FuzzyAhoCorasickBuilder::new()
        .similarity(similarity)
        .fuzzy(FuzzyLimits::new().edits(2))
        .build([Pattern::from("yx")]);

    let text = "ybyyya";
    let got = engine_matches(&engine, text, 0.0);
    let ends: Vec<usize> = got
        .iter()
        .filter(|(start, _, _, _)| *start == 3)
        .map(|(_, end, _, _)| *end)
        .collect();
    // Start 3 also has legitimate ends 3 and 4 (a pure-deletion and a one-substitution match); what
    // matters is that the two equal-penalty alignments at 5 and 6 both survive.
    for end in [5, 6] {
        assert!(
            ends.contains(&end),
            "match at [3,{end}) is missing; the engine reported ends {ends:?}"
        );
    }
}

/// The same sweep with non-unit pattern weights, which scale the score and so move the threshold
/// relative to each match.
#[test]
fn search_matches_reference_with_weights() {
    const ALPHABET: &[char] = &['a', 'b', 'c', 'x', 'y'];
    let sim_map: HashMap<(char, char), f32> = SIMILARITY_PAIRS.iter().copied().collect();
    let similarity: &'static Similarity =
        Box::leak(Box::new(Similarity::from_map(sim_map.clone())));
    let penalties = FuzzyPenalties::default();
    let ref_pen = RefPenalties {
        substitution: penalties.substitution,
        insertion: penalties.insertion,
        deletion: penalties.deletion,
        swap: penalties.swap,
    };

    let mut rng = Rng(0xC0FF_EE00_1234_5678);
    for case in 0..300u32 {
        let max_edits = rng.below(3) as u8;
        let threshold = [0.0f32, 0.4, 0.6, 0.9][rng.below(4)];
        let n_patterns = 1 + rng.below(3);

        let mut patterns: Vec<String> = Vec::new();
        let mut weights: Vec<f32> = Vec::new();
        for _ in 0..n_patterns {
            let len = 1 + rng.below(4);
            patterns.push(
                (0..len)
                    .map(|_| ALPHABET[rng.below(ALPHABET.len())])
                    .collect::<String>(),
            );
            weights.push([0.8f32, 1.0, 1.25, 2.0][rng.below(4)]);
        }
        let text: String = (0..(3 + rng.below(5)))
            .map(|_| ALPHABET[rng.below(ALPHABET.len())])
            .collect();

        let engine = FuzzyAhoCorasickBuilder::new()
            .similarity(similarity)
            .penalties(penalties.clone())
            .fuzzy(FuzzyLimits::new().edits(max_edits))
            .build(
                patterns
                    .iter()
                    .zip(&weights)
                    .map(|(p, &w)| Pattern::from(p.as_str()).weight(w))
                    .collect::<Vec<_>>(),
            );

        let refs: Vec<RefPattern<'_>> = patterns
            .iter()
            .zip(&weights)
            .map(|(p, &w)| RefPattern {
                text: p.as_str(),
                weight: w,
            })
            .collect();
        let config = RefConfig {
            pen: ref_pen,
            min_symbol_similarity: 0.0,
            max_edits,
            threshold,
            case_insensitive: false,
        };
        let expected = reference_search(&text, &refs, &sim_map, config);
        let got = engine_matches(&engine, &text, threshold);

        assert_eq!(
            got, expected,
            "case {case}: max_edits={max_edits} threshold={threshold} \
             patterns={patterns:?} weights={weights:?} text={text:?}"
        );
    }
}
