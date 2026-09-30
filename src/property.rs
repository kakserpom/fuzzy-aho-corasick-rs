/* -------------------------------------------------------------------------
 *  Property test: the real search vs a brute-force reference
 *
 *  The engine's BFS prunes aggressively -- per-node penalty ceilings, push-time global ceilings,
 *  dead-end filters, a per-window state-dedup table. Every one of those is a proof obligation, and a
 *  wrong bound does not fail loudly: it just stops reporting matches. These tests compare the engine
 *  against `reference::reference_search`, which enumerates every alignment with no pruning at all,
 *  over many randomly generated configurations.
 *
 *  Between them the sweeps below cover both search paths: a total edit budget (the monomorphised
 *  fast path) and per-type caps (the `MAX_EDITS_FAST == 255` slow path with `within_limits_*`
 *  checks), which are separate code and were previously untested against anything.
 * ---------------------------------------------------------------------- */
use crate::reference::{RefConfig, RefLimits, RefPattern, RefPenalties, reference_search};
use crate::{
    FuzzyAhoCorasick, FuzzyAhoCorasickBuilder, FuzzyLimits, FuzzyPenalties, Pattern, SearchOptions,
    Similarity,
};
use std::collections::HashMap;
use unicode_segmentation::UnicodeSegmentation;

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

/// Shared fixtures: a similarity table, the engine's penalties, and the reference's mirror of them.
struct Fixture {
    sim_map: HashMap<(char, char), f32>,
    similarity: &'static Similarity,
    penalties: FuzzyPenalties,
    ref_pen: RefPenalties,
}

impl Fixture {
    fn new() -> Self {
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
        Self {
            sim_map,
            similarity,
            penalties,
            ref_pen,
        }
    }

    /// Reference configuration for a total edit budget -- the fast path.
    fn total(&self, max_edits: u8, threshold: f32, case_insensitive: bool) -> RefConfig<'_> {
        RefConfig {
            pen: self.ref_pen,
            min_symbol_similarity: 0.0,
            limits: RefLimits {
                max_edits: Some(max_edits),
                ..RefLimits::default()
            },
            mappings: &[],
            threshold,
            case_insensitive,
        }
    }
}

/// Canonical form of the engine's matches: `(start, end, pattern_index, similarity_bits)`, sorted.
///
/// Only the span and the score are compared. The engine also reports per-type edit counts, and
/// those depend on which of several equally-cheap alignments its BFS happens to reach first, so
/// they are not part of the contract under test.
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

    let fx = Fixture::new();
    let mut rng = Rng(0x5EED_1234_ABCD_0001);

    for case in 0..CASES {
        let max_edits = rng.below(3) as u8;
        let threshold = [0.0f32, 0.5, 0.7, 0.85, 1.0][rng.below(5)];
        let case_insensitive = rng.below(2) == 0;
        // A floor on substitution similarity. The table above only has 0.5 and 0.75 pairs, so 0.6
        // and 0.8 admit strictly fewer substitutions than 0.0 does and exercise the filter.
        let min_symbol_similarity = [0.0f32, 0.4, 0.6, 0.8][rng.below(4)];
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
            .similarity(fx.similarity)
            .penalties(fx.penalties.clone())
            .case_insensitive(case_insensitive)
            .min_symbol_similarity(min_symbol_similarity)
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
                limits: None,
            })
            .collect();
        let mut config = fx.total(max_edits, threshold, case_insensitive);
        config.min_symbol_similarity = min_symbol_similarity;
        let expected = reference_search(&text, &refs, &fx.sim_map, config);
        let got = engine_matches(&engine, &text, threshold);

        assert_eq!(
            got, expected,
            "case {case}: max_edits={max_edits} threshold={threshold} \
             case_insensitive={case_insensitive} min_symbol_similarity={min_symbol_similarity} \
             patterns={patterns:?} text={text:?}"
        );
    }
}

/// Per-type caps instead of a total budget.
///
/// Setting a per-type cap leaves `edits` unset, which drops the search onto the `MAX_EDITS_FAST == 255`
/// slow path: every transition is gated by a separate `within_limits_*` check instead of one shared
/// counter. Caps are also randomly *left unset*, because `FuzzyLimits::finalize` turns an unset
/// per-type cap into `0` -- that type is then disallowed -- and the reference has to agree. All four
/// capped at zero degenerates to an exact search, so this also walks the exact path with a pattern
/// set and limits that would not otherwise produce it.
#[test]
fn search_matches_reference_with_per_type_caps() {
    const ALPHABET: &[char] = &['a', 'b', 'c', 'd', 'x', 'y'];
    const CASES: u32 = 600;

    let fx = Fixture::new();
    let mut rng = Rng(0xB0A7_5EED_1234_5678);

    for case in 0..CASES {
        let threshold = [0.0f32, 0.4, 0.65, 0.9][rng.below(4)];
        let n_patterns = 1 + rng.below(3);
        let word_len = 1 + rng.below(4);
        // Each cap is independently absent or 0..=2.
        let mut cap = || match rng.below(3) {
            0 => None,
            n => Some(n as u8 - 1),
        };
        let (ins, del, sub, swp) = (cap(), cap(), cap(), cap());

        let patterns: Vec<String> = (0..n_patterns)
            .map(|_| {
                (0..word_len)
                    .map(|_| ALPHABET[rng.below(ALPHABET.len())])
                    .collect()
            })
            .collect();
        let text: String = (0..(3 + rng.below(5)))
            .map(|_| ALPHABET[rng.below(ALPHABET.len())])
            .collect();

        // Build the limits the same way the test declares them, so the engine and the reference
        // are driven by one set of numbers.
        let mut limits = FuzzyLimits::new();
        if let Some(n) = ins {
            limits = limits.insertions(n);
        }
        if let Some(n) = del {
            limits = limits.deletions(n);
        }
        if let Some(n) = sub {
            limits = limits.substitutions(n);
        }
        if let Some(n) = swp {
            limits = limits.swaps(n);
        }

        let engine = FuzzyAhoCorasickBuilder::new()
            .similarity(fx.similarity)
            .penalties(fx.penalties.clone())
            .fuzzy(limits)
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
                limits: None,
            })
            .collect();
        let expected = reference_search(
            &text,
            &refs,
            &fx.sim_map,
            RefConfig {
                pen: fx.ref_pen,
                min_symbol_similarity: 0.0,
                limits: RefLimits {
                    max_edits: None,
                    max_insertions: ins,
                    max_deletions: del,
                    max_substitutions: sub,
                    max_swaps: swp,
                },
                mappings: &[],
                threshold,
                case_insensitive: false,
            },
        );
        let got = engine_matches(&engine, &text, threshold);

        assert_eq!(
            got, expected,
            "case {case}: caps(ins={ins:?} del={del:?} sub={sub:?} swap={swp:?}) \
             threshold={threshold} patterns={patterns:?} text={text:?}"
        );
    }
}

/// A random sequence of `len` graphemes drawn from `alphabet`. Takes the generator by argument so
/// it can be called from inside another iterator that also draws from it.
fn seq(alphabet: &[char], rng: &mut Rng, len: usize) -> String {
    (0..len)
        .map(|_| alphabet[rng.below(alphabet.len())])
        .collect()
}

/// Mirror engine [`FuzzyLimits`] into the reference's own type, so the two cannot drift on what an
/// unset cap means.
fn to_ref(lim: &FuzzyLimits) -> RefLimits {
    RefLimits {
        max_edits: lim.edits,
        max_insertions: lim.insertions,
        max_deletions: lim.deletions,
        max_substitutions: lim.substitutions,
        max_swaps: lim.swaps,
    }
}

/// Per-pattern limits, mixed with engine-wide ones and with patterns that carry none.
///
/// A pattern with its own limits sets `has_pattern_limits`, which forces the search off the
/// monomorphised fast path *and* makes the walk consult per-node limits: at each node the limits of
/// the pattern ending there are looked up and used to gate every outgoing transition. That is the
/// one place where a limit can be applied to a walk that another pattern at the same node would have
/// allowed, so it is worth checking against a reference that applies limits only per pattern.
///
/// Patterns may also be duplicates carrying *different* limits, which is the sharpest version of
/// that concern: two entries, one automaton node.
#[test]
fn search_matches_reference_with_per_pattern_limits() {
    const ALPHABET: &[char] = &['a', 'b', 'c', 'd', 'x', 'y'];
    const CASES: u32 = 500;

    let fx = Fixture::new();
    let mut rng = Rng(0xDEAD_BEEF_0BAD_F00D);

    for case in 0..CASES {
        let threshold = [0.0f32, 0.4, 0.7][rng.below(3)];
        let n_patterns = 2 + rng.below(3);
        let word_len = 1 + rng.below(4);

        // A random limit set: either a total budget, or per-type caps with some left unset.
        let random_limits = |rng: &mut Rng| {
            let mut lim = FuzzyLimits::new();
            let mut ref_lim = RefLimits::default();
            if rng.below(2) == 0 {
                let n = rng.below(3) as u8;
                lim = lim.edits(n);
                ref_lim.max_edits = Some(n);
            } else {
                let cap = |rng: &mut Rng| match rng.below(3) {
                    0 => None,
                    n => Some(n as u8 - 1),
                };
                let (i, d, s, w) = (cap(rng), cap(rng), cap(rng), cap(rng));
                if let Some(n) = i {
                    lim = lim.insertions(n);
                }
                if let Some(n) = d {
                    lim = lim.deletions(n);
                }
                if let Some(n) = s {
                    lim = lim.substitutions(n);
                }
                if let Some(n) = w {
                    lim = lim.swaps(n);
                }
                ref_lim.max_insertions = i;
                ref_lim.max_deletions = d;
                ref_lim.max_substitutions = s;
                ref_lim.max_swaps = w;
            }
            (lim, ref_lim)
        };

        // Each pattern independently gets its own limits, or falls back to the engine-wide set.
        // The global set is itself sometimes absent, in which case the builder derives one from the
        // per-pattern maxima.
        let global = match rng.below(3) {
            0 => Some(random_limits(&mut rng).0),
            _ => None,
        };

        let mut patterns: Vec<String> = Vec::new();
        let mut engine_patterns: Vec<Pattern> = Vec::new();
        let mut own: Vec<Option<RefLimits>> = Vec::new();
        for _ in 0..n_patterns {
            let p: String = (0..word_len)
                .map(|_| ALPHABET[rng.below(ALPHABET.len())])
                .collect();
            // Reuse an earlier pattern text now and then, so duplicates land on one node with
            // different limits.
            let p = if !patterns.is_empty() && rng.below(3) == 0 {
                patterns[rng.below(patterns.len())].clone()
            } else {
                p
            };
            let pat = Pattern::from(p.as_str());
            let (lim, ref_lim) = if rng.below(2) == 0 {
                let pair = random_limits(&mut rng);
                (Some(pair.0), Some(pair.1))
            } else {
                (None, None)
            };
            engine_patterns.push(match lim {
                Some(l) => pat.fuzzy(l),
                None => pat,
            });
            patterns.push(p);
            own.push(ref_lim);
        }

        let text: String = (0..(3 + rng.below(5)))
            .map(|_| ALPHABET[rng.below(ALPHABET.len())])
            .collect();

        let mut builder = FuzzyAhoCorasickBuilder::new()
            .similarity(fx.similarity)
            .penalties(fx.penalties.clone());
        if let Some(l) = global.clone() {
            builder = builder.fuzzy(l);
        }
        let engine = builder.build(engine_patterns);

        // A pattern carrying no limits of its own falls back to the engine-wide limits, and with
        // none of those it is exact -- which is what `RefLimits::default` says here.
        let effective_global = global
            .as_ref()
            .map_or_else(RefLimits::default, |l| to_ref(&l.clone().finalize()));

        let refs: Vec<RefPattern<'_>> = patterns
            .iter()
            .zip(&own)
            .map(|(p, l)| RefPattern {
                text: p.as_str(),
                weight: 1.0,
                limits: *l,
            })
            .collect();
        let expected = reference_search(
            &text,
            &refs,
            &fx.sim_map,
            RefConfig {
                pen: fx.ref_pen,
                min_symbol_similarity: 0.0,
                limits: effective_global,
                mappings: &[],
                threshold,
                case_insensitive: false,
            },
        );
        let got = engine_matches(&engine, &text, threshold);

        assert_eq!(
            got, expected,
            "case {case}: effective_global={effective_global:?} own_limits={own:?} \
             threshold={threshold} patterns={patterns:?} text={text:?}"
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
    let fx = Fixture::new();
    let engine = FuzzyAhoCorasickBuilder::new()
        .similarity(fx.similarity)
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
    let fx = Fixture::new();

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
            .similarity(fx.similarity)
            .penalties(fx.penalties.clone())
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
                limits: None,
            })
            .collect();
        let expected = reference_search(
            &text,
            &refs,
            &fx.sim_map,
            fx.total(max_edits, threshold, false),
        );
        let got = engine_matches(&engine, &text, threshold);

        assert_eq!(
            got, expected,
            "case {case}: max_edits={max_edits} threshold={threshold} \
             patterns={patterns:?} weights={weights:?} text={text:?}"
        );
    }
}

/// A pattern's limits must not gate walks that are not its own.
///
/// `get_node_limits` returns the limits of *the* pattern ending at a node, and the search uses that
/// to gate every transition leaving the node. A walk sitting at a node may equally be on its way to
/// some longer pattern whose trie path runs through that node, and that longer pattern may have a
/// much larger budget. The gate therefore has to be the *union* of the budgets of every pattern
/// whose path includes the node, not any one of them.
///
/// Short pattern "ab" forbids substitutions; longer pattern "abc" allows one. "abc" against "abx"
/// needs a substitution to leave the "ab" node, so a gate that reads "ab"'s limits loses it.
#[test]
fn a_patterns_limits_do_not_gate_longer_patterns() {
    let short = Pattern::from("ab").fuzzy(FuzzyLimits::new().substitutions(0));
    let long = Pattern::from("abc").fuzzy(FuzzyLimits::new().edits(1));
    let engine = FuzzyAhoCorasickBuilder::new().build([short, long]);

    let got = engine_matches(&engine, "abx", 0.0);
    let ends: Vec<usize> = got
        .iter()
        .filter(|(s, _, pi, _)| *s == 0 && *pi == 1)
        .map(|(_, e, _, _)| *e)
        .collect();
    assert!(
        ends.contains(&3),
        "\"abc\" must match \"abx\" at [0,3) with one substitution; \
         the walk is gated by \"ab\"'s zero substitution budget. Engine reported ends {ends:?}"
    );
}

/// Beam pruning may only lose matches, never invent them.
///
/// A beam is documented as lossy — `beam_width`'s own doc says it "may miss some fuzzy matches" — so
/// it cannot be compared for equality against the exhaustive reference. But there is a sharp
/// invariant that *does* have to hold, and which the reference makes checkable: every match a
/// beamed search reports is backed by some real alignment, so it must appear in the unpruned result
/// too, and never with a *better* score than the unpruned search found for that same span. A beam
/// that produced anything else — a span the unpruned search never found, or a score above the
/// optimum — would mean the frontier selection is corrupting state rather than discarding it.
///
/// The same holds for the automatic beam, which is checked separately with a tiny budget so it
/// actually engages.
#[test]
fn beam_pruning_only_loses_matches() {
    const ALPHABET: &[char] = &['a', 'b', 'c', 'd', 'x', 'y'];
    const CASES: u32 = 300;

    let fx = Fixture::new();
    let mut rng = Rng(0xB3A4_0000_1234_5678);
    // Count cases where the beam actually cost us something. Without this the test could pass
    // vacuously -- e.g. if `beam_width` were silently ignored, every case would be unpruned and the
    // invariant would hold trivially.
    let mut pruned_somewhere = 0usize;

    for case in 0..CASES {
        let max_edits = rng.below(3) as u8;
        let threshold = [0.0f32, 0.4, 0.6][rng.below(3)];
        let n_patterns = 1 + rng.below(3);
        let word_len = 2 + rng.below(2);

        let patterns: Vec<String> = (0..n_patterns)
            .map(|_| {
                (0..word_len)
                    .map(|_| ALPHABET[rng.below(ALPHABET.len())])
                    .collect()
            })
            .collect();
        let text: String = (0..(4 + rng.below(6)))
            .map(|_| ALPHABET[rng.below(ALPHABET.len())])
            .collect();

        let refs: Vec<RefPattern<'_>> = patterns
            .iter()
            .map(|p| RefPattern {
                text: p.as_str(),
                weight: 1.0,
                limits: None,
            })
            .collect();
        let config = fx.total(max_edits, threshold, false);
        let expected = reference_search(&text, &refs, &fx.sim_map, config);
        let best: HashMap<(usize, usize, usize), f32> = expected
            .iter()
            .map(|&(s, e, pi, bits)| ((s, e, pi), f32::from_bits(bits)))
            .collect();

        // A width of 1 or 2 prunes hard; the automatic beam gets a budget of a handful of states so
        // it engages almost immediately.
        let engine = FuzzyAhoCorasickBuilder::new()
            .similarity(fx.similarity)
            .penalties(fx.penalties.clone())
            .fuzzy(FuzzyLimits::new().edits(max_edits))
            .beam_width(1 + rng.below(2))
            .auto_beam(4, 2)
            .build(
                patterns
                    .iter()
                    .map(|p| Pattern::from(p.as_str()))
                    .collect::<Vec<_>>(),
            );

        let got = engine_matches(&engine, &text, threshold);
        if got.len() < expected.len() {
            pruned_somewhere += 1;
        }
        for (start, end, pi, bits) in got {
            let similarity = f32::from_bits(bits);
            let optimum = best.get(&(start, end, pi)).unwrap_or_else(|| {
                panic!(
                    "case {case}: beam reported ({start},{end},{pi}) at {similarity}, \\
                         which the unpruned search does not find at all. \\
                         patterns={patterns:?} text={text:?}"
                )
            });
            assert!(
                similarity <= *optimum,
                "case {case}: beam scored ({start},{end},{pi}) at {similarity}, above the \\
                 unpruned optimum {optimum}. patterns={patterns:?} text={text:?}"
            );
        }
    }
    assert!(
        pruned_somewhere > CASES as usize / 10,
        "the beam pruned nothing in {pruned_somewhere} of {CASES} cases, so this test proved nothing"
    );
}

/// Multi-character mappings, the last search feature the reference could not previously express.
///
/// A rule stands one grapheme sequence in for another, bidirectionally, and applies as a single
/// substitution: it consumes the whole pattern side and the whole haystack side and is charged to
/// `substitutions` alone. The builder only records a rule where its pattern side is a real path in
/// the trie, so a rule that no pattern contains can never fire — which is what keeps a mapping from
/// costing anything on the many nodes it does not apply to.
///
/// Sequences here are built from the same alphabet as the patterns, so every "grapheme" is one
/// ASCII byte. Scores vary so the mapped penalty is not always the full substitution.
#[test]
fn search_matches_reference_with_mappings() {
    const ALPHABET: &[char] = &['a', 'b', 'c', 'd', 'x', 'y'];
    const CASES: u32 = 500;

    let fx = Fixture::new();
    let mut rng = Rng(0xAAAB_BBB0_1234_5678);
    // Cases where the mappings actually changed the outcome. Without this the test could pass
    // vacuously, if no rule ever lined up with a pattern and a haystack.
    let mut changed_something = 0usize;

    for case in 0..CASES {
        let max_edits = rng.below(3) as u8;
        let threshold = [0.0f32, 0.4, 0.65, 0.85][rng.below(4)];
        let n_patterns = 1 + rng.below(3);
        let word_len = 3 + rng.below(3);

        let patterns: Vec<String> = (0..n_patterns)
            .map(|_| seq(ALPHABET, &mut rng, word_len))
            .collect();

        // Take each rule's pattern side from an actual pattern, so the builder records it and it
        // has a chance to fire. Drawing both sides at random instead made almost every rule
        // unapplicable, and the sweep passed without really testing anything. Half the rules are
        // reversed, to exercise both directions of the expansion.
        let n_rules = rng.below(3);
        let mut rules: Vec<(String, String, f32)> = Vec::new();
        for _ in 0..n_rules {
            let host: Vec<char> = patterns[rng.below(patterns.len())].chars().collect();
            let alen = 1 + rng.below(host.len());
            let astart = rng.below(host.len() - alen + 1);
            let a: String = host[astart..astart + alen].iter().collect();
            let blen = 1 + rng.below(3);
            let b = seq(ALPHABET, &mut rng, blen);
            let score = [0.0f32, 0.5, 1.0][rng.below(3)];
            if rng.below(2) == 0 {
                rules.push((a, b, score));
            } else {
                rules.push((b, a, score));
            }
        }

        // Half the time, plant a haystack side in the text so the rule has something to match.
        let mut text: String = (0..(4 + rng.below(6)))
            .map(|_| ALPHABET[rng.below(ALPHABET.len())])
            .collect();
        if let Some((_, b, _)) = rules.first() {
            if rng.below(2) == 0 {
                let at = rng.below(text.chars().count() + 1);
                let mut chars: Vec<char> = text.chars().collect();
                for (k, c) in b.chars().enumerate() {
                    chars.insert(at + k, c);
                }
                text = chars.into_iter().collect();
            }
        }

        let mut builder = FuzzyAhoCorasickBuilder::new()
            .similarity(fx.similarity)
            .penalties(fx.penalties.clone())
            .fuzzy(FuzzyLimits::new().edits(max_edits));
        for (a, b, score) in rules.clone() {
            builder = builder.mapping_scored(a, b, score);
        }
        let engine = builder.build(
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
                limits: None,
            })
            .collect();
        let expected = reference_search(
            &text,
            &refs,
            &fx.sim_map,
            RefConfig {
                pen: fx.ref_pen,
                min_symbol_similarity: 0.0,
                limits: RefLimits {
                    max_edits: Some(max_edits),
                    ..RefLimits::default()
                },
                mappings: &rules,
                threshold,
                case_insensitive: false,
            },
        );
        let got = engine_matches(&engine, &text, threshold);

        assert_eq!(
            got, expected,
            "case {case}: max_edits={max_edits} threshold={threshold} rules={rules:?} \
             patterns={patterns:?} text={text:?}"
        );

        // The same search with no mappings at all: where the two differ, a rule did real work.
        let without = FuzzyAhoCorasickBuilder::new()
            .similarity(fx.similarity)
            .penalties(fx.penalties.clone())
            .fuzzy(FuzzyLimits::new().edits(max_edits))
            .build(
                patterns
                    .iter()
                    .map(|p| Pattern::from(p.as_str()))
                    .collect::<Vec<_>>(),
            );
        if engine_matches(&without, &text, threshold) != got {
            changed_something += 1;
        }
    }
    assert!(
        changed_something > CASES as usize / 20,
        "mappings changed the result in only {changed_something} of {CASES} cases, \
         so this test proved little about them"
    );
}

/// Self-consistency of reported spans, over grapheme clusters rather than bytes.
///
/// The reference is `char`-based, so it cannot adjudicate the Unicode path — where a "position" is a
/// grapheme and a "span" is a pair of byte offsets that have to survive clusters of unequal length.
/// What it *can* do is tell us which invariants any correct result set has to satisfy, and those are
/// checkable without it:
///
/// * every reported offset is a `char` boundary, and `text` is exactly the haystack slice it claims;
/// * `edits` is the sum of the per-type counts (a swap and a mapping each count as one edit, and a
///   mapping is charged to `substitutions`);
/// * **zero edits implies the span is exactly the pattern** — in graphemes, not bytes. This is the
///   invariant that caught a suffix pattern being reported at a longer walk's span, and it is the
///   sharpest thing here, because an off-by-one anywhere in the grapheme-to-byte mapping breaks it.
///
/// The clusters are chosen to span the awkward cases: a precomposed accented letter, the same letter
/// decomposed, a ZWJ emoji sequence, a regional-indicator flag, a Hangul syllable in both forms, and
/// a ligature — several of which are more than one byte and more than one `char`.
#[test]
fn reported_spans_are_self_consistent_over_grapheme_clusters() {
    const CLUSTERS: &[&str] = &[
        "a",
        "b",
        "c",
        "\u{e9}",                                      // é precomposed: 2 bytes, 1 char
        "e\u{301}",                                    // é decomposed: 3 bytes, 2 chars, 1 grapheme
        "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}", // family, ZWJ-joined
        "\u{1F1EC}\u{1F1E7}",                          // flag, two regional indicators
        "\u{AC00}",                                    // 가 precomposed Hangul
        "\u{1112}\u{1161}",                            // 한 as Jamo: one grapheme, two chars
        "\u{FB01}",                                    // ﬁ ligature
        "\u{DF}",                                      // ß, which case-folds to two chars
    ];
    const CASES: u32 = 400;

    let fx = Fixture::new();
    let mut rng = Rng(0x9C1C_1E57_1234_5678);
    // Matches actually inspected, and how many were multi-byte or multi-char spans -- without these
    // the sweep could check nothing at all on a generation that happens to find no matches.
    let mut checked = 0usize;
    let mut multi_char_clusters = 0usize;

    for case in 0..CASES {
        let max_edits = rng.below(2) as u8;
        let threshold = [0.0f32, 0.5, 0.8][rng.below(3)];
        let case_insensitive = rng.below(2) == 0;
        let n_patterns = 1 + rng.below(3);

        let patterns: Vec<String> = (0..n_patterns)
            .map(|_| {
                let n = 1 + rng.below(3);
                (0..n)
                    .map(|_| CLUSTERS[rng.below(CLUSTERS.len())])
                    .collect::<String>()
            })
            .collect();
        let text: String = (0..(2 + rng.below(6)))
            .map(|_| CLUSTERS[rng.below(CLUSTERS.len())])
            .collect();

        let engine = FuzzyAhoCorasickBuilder::new()
            .similarity(fx.similarity)
            .penalties(fx.penalties.clone())
            .case_insensitive(case_insensitive)
            .fuzzy(FuzzyLimits::new().edits(max_edits))
            .build(
                patterns
                    .iter()
                    .map(|p| Pattern::from(p.as_str()))
                    .collect::<Vec<_>>(),
            );

        let found = engine.search(&text, &SearchOptions::new().threshold(threshold));
        let found = found
            .as_ref()
            .unwrap_or_else(|e| panic!("search failed in case {case}: {e}"));

        for m in found {
            checked += 1;
            // A span whose graphemes are not all single `char`s, i.e. the case where getting the
            // grapheme-to-byte mapping wrong would actually show up.
            if m.text.chars().count() != m.text.graphemes(true).count() {
                multi_char_clusters += 1;
            }
            let where_ = format!(
                "case {case}: pattern {:?} span [{},{}) text {:?} edits={} \
                 (i{} d{} s{} w{}) case_insensitive={case_insensitive} text={text:?}",
                m.pattern.as_str(),
                m.start,
                m.end,
                m.text,
                m.edits,
                m.insertions,
                m.deletions,
                m.substitutions,
                m.swaps,
            );

            assert!(m.start <= m.end, "inverted span at {where_}");
            assert!(
                text.is_char_boundary(m.start),
                "start {} is not a char boundary at {where_}",
                m.start
            );
            assert!(
                text.is_char_boundary(m.end),
                "end {} is not a char boundary at {where_}",
                m.end
            );
            assert_eq!(
                m.text,
                &text[m.start..m.end],
                "reported text disagrees with the haystack slice at {where_}"
            );
            assert_eq!(
                m.edits,
                m.insertions + m.deletions + m.substitutions + m.swaps,
                "edits is not the sum of the per-type counts at {where_}"
            );

            if m.edits == 0 {
                // The invariant that a grapheme/byte confusion breaks.
                assert_eq!(
                    m.text.graphemes(true).count(),
                    m.pattern.grapheme_len,
                    "a zero-edit match must span exactly its pattern in graphemes at {where_}"
                );
                assert!(
                    (m.similarity - m.pattern.weight).abs() < 1e-6,
                    "a zero-edit match must score exactly the pattern weight at {where_}"
                );
            }
            assert!(
                m.similarity <= m.pattern.weight + 1e-6,
                "similarity {} exceeds the weight at {where_}",
                m.similarity
            );
        }
    }
    assert!(
        checked > 200,
        "only {checked} matches were inspected across {CASES} cases"
    );
    assert!(
        multi_char_clusters > 20,
        "only {multi_char_clusters} inspected matches spanned a multi-char grapheme cluster, \
         which is the whole point of this sweep"
    );
}

/// The pre-filter must return exactly what a plain search returns.
///
/// `Prefiltered::search` documents that it "returns exactly what
/// `FuzzyAhoCorasick::search` would for the same `opts`", which is a strong and useful claim: it is
/// what makes the pre-filter safe to reach for by default. This checks it the same way as everything
/// else here, over random configurations, and includes the knobs the pre-filter's own construction
/// does *not* obviously account for:
///
/// * `auto_beam` counts expanded states *across all windows* and switches the beam on partway
///   through. A pre-filtered search examines fewer windows, so the running total differs and the
///   switch can land in a different place — which would make the two disagree on a lossy search.
/// * `beam_width` is per window, so it should not matter, but that is worth confirming rather than
///   assuming.
/// * `min_symbol_similarity` only ever *rejects* substitutions, so the filter can over-approximate
///   and stay sound; again, worth confirming.
///
/// Any disagreement is a divergence between two APIs that are documented to agree.
#[test]
fn prefilter_agrees_with_plain_search() {
    const ALPHABET: &[char] = &['a', 'b', 'c', 'd', 'x', 'y'];
    const CASES: u32 = 400;

    let fx = Fixture::new();
    let mut rng = Rng(0x9EF1_17E4_0000_0001);
    // Configurations where a filter was actually built, and cases where it changed the result --
    // without both, this could pass by always falling back.
    let mut active = 0usize;

    for case in 0..CASES {
        let max_edits = rng.below(3) as u8;
        let threshold = [0.0f32, 0.5, 0.7, 0.85, 0.95][rng.below(5)];
        let case_insensitive = rng.below(2) == 0;
        let n_patterns = 1 + rng.below(4);
        let word_len = 2 + rng.below(4);
        let min_symbol_similarity = [0.0f32, 0.4, 0.6][rng.below(3)];
        // The beam knobs, cycled through rather than randomised so all three shapes get covered.
        let beam = case % 3;

        let patterns: Vec<String> = (0..n_patterns)
            .map(|_| seq(ALPHABET, &mut rng, word_len))
            .collect();
        let text: String = (0..(4 + rng.below(10)))
            .map(|_| ALPHABET[rng.below(ALPHABET.len())])
            .collect();

        let mut builder = FuzzyAhoCorasickBuilder::new()
            .similarity(fx.similarity)
            .penalties(fx.penalties.clone())
            .case_insensitive(case_insensitive)
            .min_symbol_similarity(min_symbol_similarity)
            .fuzzy(FuzzyLimits::new().edits(max_edits));
        builder = match beam {
            0 => builder,
            1 => builder.beam_width(2),
            _ => builder.auto_beam(2, 2),
        };
        let engine = builder.build(
            patterns
                .iter()
                .map(|p| Pattern::from(p.as_str()))
                .collect::<Vec<_>>(),
        );

        let pf = engine.with_prefilter();
        if pf.is_active() {
            active += 1;
        }

        for &sorted in &[false, true] {
            // The un-beamed result is the yardstick: every reported match must be a real match, so
            // any lossy configuration is free to omit some but never to invent one.
            let mut base_opts = SearchOptions::new().threshold(threshold);
            if sorted {
                base_opts = base_opts.sorted().non_overlapping();
            }
            let unbeammed: HashMap<(usize, usize, usize), f32> = engine
                .search(&text, &base_opts)
                .unwrap()
                .inner
                .iter()
                .map(|m| ((m.start, m.end, m.pattern_index), m.similarity))
                .collect();

            let mut opts = SearchOptions::new().threshold(threshold);
            if sorted {
                opts = opts.sorted().non_overlapping();
            }
            let plain = engine.search(&text, &opts).unwrap();
            let filtered = pf.search(&text, &opts).unwrap();

            let plain_set: HashMap<(usize, usize, usize), f32> = plain
                .inner
                .iter()
                .map(|m| ((m.start, m.end, m.pattern_index), m.similarity))
                .collect();
            let filtered_set: HashMap<(usize, usize, usize), f32> = filtered
                .inner
                .iter()
                .map(|m| ((m.start, m.end, m.pattern_index), m.similarity))
                .collect();

            // Every match either way must be real. This is the invariant that must hold under a
            // lossy beam, and it is what makes the pre-filter's own re-search trustworthy.
            for (key, sim) in plain_set.iter().chain(filtered_set.iter()) {
                assert_eq!(
                    unbeammed.get(key).copied(),
                    Some(*sim),
                    "case {case}: a beamed search reported {key:?} at {sim}, which the un-beamed \
                     search does not find (beam={beam} sorted={sorted} \
                     patterns={patterns:?} text={text:?})"
                );
            }

            if beam != 0 {
                // A beam makes results lossy, and *which* states survive depends on the order
                // windows are visited -- which a pre-filtered search changes, because it visits only
                // candidate regions. So the two are not expected to agree exactly here. Ordering is
                // unspecified under `Order::Unsorted` anyway.
                continue;
            }

            assert_eq!(
                plain_set,
                filtered_set,
                "case {case}: pre-filter disagrees with plain search on an un-beamed search \
                 (sorted={sorted} max_edits={max_edits} threshold={threshold} \
                 case_insensitive={case_insensitive} \
                 min_symbol_similarity={min_symbol_similarity} active={} \
                 patterns={patterns:?} text={text:?})\n plain:    {plain:?}\n filtered: {filtered:?}",
                pf.is_active(),
            );
        }
    }
    assert!(
        active > CASES as usize / 2,
        "a filter was only built for {active} of {CASES} cases, so this proved little"
    );
}

/// Ranking and overlap resolution are presentational; the found-match set may not depend on them.
///
/// The `Order`/`Overlap` matrix is twelve combinations with intricate tie-breaking, and one real bug
/// lived in it (ranking counted pattern length in bytes rather than graphemes). What that bug had in
/// common with any other is that it changed *which* match was selected, so the invariant worth
/// sweeping is the strong one:
///
/// * the set of matches found is **identical across all four orders** — ranking may only reorder,
///   never add or drop. (`Overlap::Keep` is what makes this observable: the other two resolve
///   overlaps, which selects.)
/// * `NonOverlapping` and `NonOverlappingUnique` are subsets of that set — resolving may only drop.
/// * `NonOverlapping` spans are pairwise disjoint, and `NonOverlappingUnique` additionally keeps at
///   most one match per pattern identity, where identity is `custom_unique_id` when set and the
///   pattern index otherwise.
/// * every reported match meets the threshold and has a valid span on the haystack.
/// * searching twice gives identical results, in every combination.
///
/// Patterns carry a shared `custom_unique_id` so `NonOverlappingUnique` has something real to
/// collapse, and some are multi-byte so length is counted in the unit the engine uses.
#[test]
fn order_and_overlap_change_only_presentation() {
    const ALPHABET: &[char] = &['a', 'b', 'c', 'd', 'x', 'y'];
    const CASES: u32 = 300;

    let fx = Fixture::new();
    let mut rng = Rng(0x0A0E_0A0E_1234_5678);
    let mut with_shared_identity = 0usize;

    for case in 0..CASES {
        let max_edits = rng.below(3) as u8;
        let threshold = [0.0f32, 0.3, 0.6, 0.85][rng.below(4)];
        let n_patterns = 2 + rng.below(3);
        let word_len = 1 + rng.below(4);

        let patterns: Vec<String> = (0..n_patterns)
            .map(|_| seq(ALPHABET, &mut rng, word_len))
            .collect();
        // Give every third pattern a shared identity, so `NonOverlappingUnique` has distinct
        // patterns competing for one slot.
        let identities: Vec<Option<usize>> = (0..n_patterns)
            .map(|i| if i % 3 == 2 { Some(7) } else { None })
            .collect();
        if identities.iter().any(Option::is_some) {
            with_shared_identity += 1;
        }

        let text: String = (0..(3 + rng.below(8)))
            .map(|_| ALPHABET[rng.below(ALPHABET.len())])
            .collect();

        let built: Vec<Pattern> = patterns
            .iter()
            .zip(&identities)
            .map(|(p, id)| {
                let pat = Pattern::from(p.as_str());
                match id {
                    Some(i) => pat.custom_unique_id(*i),
                    None => pat,
                }
            })
            .collect();
        let engine = FuzzyAhoCorasickBuilder::new()
            .similarity(fx.similarity)
            .penalties(fx.penalties.clone())
            .fuzzy(FuzzyLimits::new().edits(max_edits))
            .build(built);

        // `Overlap::Keep` under each order: this is where the raw found set is observable.
        let orders: [(&str, SearchOptions); 4] = [
            ("unsorted", SearchOptions::new().threshold(threshold)),
            (
                "default",
                SearchOptions::new().threshold(threshold).sorted(),
            ),
            ("greedy", SearchOptions::new().threshold(threshold).greedy()),
            (
                "coverage_weighted",
                SearchOptions::new()
                    .threshold(threshold)
                    .coverage_weighted(),
            ),
        ];

        // Similarity goes in as its bit pattern so the key is `Ord`; the same float has one bit
        // pattern, and a NaN would never sort equal to a real score anyway.
        let key =
            |m: &crate::FuzzyMatch<'_>| (m.start, m.end, m.pattern_index, m.similarity.to_bits());
        let mut baseline: Option<Vec<_>> = None;
        for (name, opts) in orders {
            let hits = engine.search(&text, &opts).unwrap();
            // Determinism: the same call twice must give the same answer.
            let again = engine.search(&text, &opts).unwrap();
            assert_eq!(
                hits.inner.iter().map(key).collect::<Vec<_>>(),
                again.inner.iter().map(key).collect::<Vec<_>>(),
                "case {case}: {name} is not deterministic"
            );
            for m in hits.iter() {
                assert!(
                    m.similarity >= threshold - 1e-6,
                    "case {case}: {name} reported similarity {} below the threshold {threshold}",
                    m.similarity
                );
                assert!(
                    m.start <= m.end && m.end <= text.len(),
                    "case {case}: {name} reported an out-of-range span [{},{}) for {text:?}",
                    m.start,
                    m.end
                );
            }
            // Compare as a *set*: `Unsorted` returns matches in an unspecified order and `Default`
            // returns them ranked, so the sequences legitimately differ even when the found set is
            // identical. That difference is the whole point of `Order`.
            let mut found: Vec<_> = hits.inner.iter().map(key).collect();
            found.sort_unstable();
            match &baseline {
                None => baseline = Some(found),
                Some(first) => assert_eq!(
                    *first, found,
                    "case {case}: order {name} changed *which* matches were found, not just their \
                     order ({patterns:?} text={text:?})"
                ),
            }
        }
        let baseline = baseline.expect("at least one order is always run");

        // Overlap resolution may only drop, and must respect its two contracts.
        for (name, opts) in [
            (
                "non_overlapping",
                SearchOptions::new()
                    .threshold(threshold)
                    .sorted()
                    .non_overlapping(),
            ),
            (
                "non_overlapping_unique",
                SearchOptions::new()
                    .threshold(threshold)
                    .sorted()
                    .non_overlapping_unique(),
            ),
        ] {
            let hits = engine.search(&text, &opts).unwrap();
            let got: Vec<_> = hits.inner.iter().map(key).collect();
            for m in &got {
                assert!(
                    baseline.contains(m),
                    "case {case}: {name} reported {m:?}, which no order found under Keep"
                );
            }
            // Pairwise disjoint.
            let mut spans: Vec<(usize, usize)> = hits.iter().map(|m| (m.start, m.end)).collect();
            spans.sort_unstable();
            for pair in spans.windows(2) {
                assert!(
                    pair[0].1 <= pair[1].0,
                    "case {case}: {name} left overlapping spans {pair:?} in {text:?} \
                     ({patterns:?})"
                );
            }
            if name == "non_overlapping_unique" {
                let mut seen = std::collections::HashSet::new();
                for m in hits.iter() {
                    let id = m.pattern.custom_unique_id.unwrap_or(m.pattern_index);
                    assert!(
                        seen.insert(id),
                        "case {case}: {name} kept two matches for identity {id}"
                    );
                }
            }
        }
    }
    assert!(
        with_shared_identity > CASES as usize / 2,
        "only {with_shared_identity} of {CASES} cases had a shared custom_unique_id, so \
         non_overlapping_unique was barely exercised"
    );
}

/// The pre-filter must stay exact as the pattern set grows, across the whole range of its behaviour.
///
/// The filter's selectivity depends on how many patterns there are, and the new `q`-gram filter makes
/// that dependence explicit rather than incidental. A small set leaves the block space sparse, so
/// candidates are rare and the filter earns its keep; a few hundred patterns saturate it, candidates
/// cover the text, and the search deliberately falls back to a plain one. Both regimes, and the
/// boundary between them, have to produce the plain search's answer exactly.
///
/// Pattern lengths vary deliberately: the block length is `m / (k + 1)`, so a set mixing lengths
/// 1..=14 exercises the block-length computation, the `MIN_Q` refusal that fires when a
/// single-character pattern drags the block down to 1, and the "covered too much, just search" path.
#[test]
fn prefilter_stays_exact_as_the_pattern_set_grows() {
    const ALPHABET: &[char] = &['a', 'b', 'c', 'd', 'x', 'y', 'z'];
    const CASES: u32 = 250;

    let fx = Fixture::new();
    let mut rng = Rng(0x9A9A_0000_1234_5678);
    let mut active = 0usize;

    for case in 0..CASES {
        let threshold = [0.5f32, 0.7, 0.8, 0.9, 1.0][rng.below(5)];
        let max_edits = [0u8, 1, 2][rng.below(3)];
        // Spread across the regimes: a handful of patterns, a few dozen, and a few hundred.
        let n_patterns = [2usize, 8, 40, 200][rng.below(4)];

        let mut patterns: Vec<String> = Vec::new();
        for _ in 0..n_patterns {
            // Lengths 1..=14 so a single short pattern can force the `MIN_Q` refusal.
            let len = 1 + rng.below(14);
            patterns.push(seq(ALPHABET, &mut rng, len));
        }
        let text: String = (0..(6 + rng.below(40)))
            .map(|_| ALPHABET[rng.below(ALPHABET.len())])
            .collect();

        let engine = FuzzyAhoCorasickBuilder::new()
            .similarity(fx.similarity)
            .penalties(fx.penalties.clone())
            .fuzzy(FuzzyLimits::new().edits(max_edits))
            .build(
                patterns
                    .iter()
                    .map(|p| Pattern::from(p.as_str()))
                    .collect::<Vec<_>>(),
            );

        let pf = engine.with_prefilter();
        if pf.is_active() {
            active += 1;
        }
        let opts = SearchOptions::new().threshold(threshold);
        let plain: std::collections::HashMap<(usize, usize, usize), f32> = engine
            .search(&text, &opts)
            .unwrap()
            .inner
            .iter()
            .map(|m| ((m.start, m.end, m.pattern_index), m.similarity))
            .collect();
        let filtered: std::collections::HashMap<(usize, usize, usize), f32> = pf
            .search(&text, &opts)
            .unwrap()
            .inner
            .iter()
            .map(|m| ((m.start, m.end, m.pattern_index), m.similarity))
            .collect();
        assert_eq!(
            plain,
            filtered,
            "case {case}: pre-filter differs at {n_patterns} patterns, edits={max_edits}, \
             threshold={threshold} (active={}) patterns={patterns:?} text={text:?}",
            pf.is_active(),
        );
    }
    assert!(
        active > CASES as usize / 2,
        "a filter was only built for {active} of {CASES} cases, so this proved little"
    );
}

/// Skipping the dedup at a one-edit budget must not blow the state space up.
///
/// The search folds the dedup away at `edits(1)` with no mappings, which is worth 40%-plus on a
/// many-pattern corpus. Collapsing duplicate states is a pure optimisation -- two states agreeing on
/// node, span and per-type counts have identical futures, so expanding both yields the same results
/// -- which is why the win is legitimate. What it is *not* is free of consequence: the reason the
/// table exists at all is that insertions and deletions reach the same position by exponentially many
/// orderings, and at a budget of 2 or more that count is unbounded in the haystack length.
///
/// So this checks the property that actually matters: with the table gone, the number of expanded
/// states per start window stays bounded, and the total therefore grows *linearly* in the haystack
/// rather than blowing up. If the gate were ever widened to a budget where the state space is not
/// bounded, the growth here would turn super-linear and this fails.
#[test]
fn one_edit_state_space_stays_bounded_without_the_dedup_table() {
    use crate::search::STATES_EXPANDED;

    const ALPHABET: &[char] = &['a', 'b', 'c', 'd'];
    let fx = Fixture::new();
    let mut per_char: Vec<f64> = Vec::new();

    for &len in &[500usize, 1000, 2000, 4000, 8000] {
        let mut rng = Rng(0x6000_0000_1234_5678);
        // One pattern, long enough that a window has room for a chain of exact transitions after
        // the single edit -- the shape where deletion placement is most ambiguous.
        let pattern: String = seq(ALPHABET, &mut rng, 12);
        let text: String = (0..len).map(|_| ALPHABET[rng.below(4)]).collect();

        let engine = FuzzyAhoCorasickBuilder::new()
            .similarity(fx.similarity)
            .fuzzy(FuzzyLimits::new().edits(1))
            .build([Pattern::from(pattern.as_str())]);

        STATES_EXPANDED.with(|c| c.set(0));
        let _ = engine
            .search(&text, &SearchOptions::new().threshold(0.5))
            .unwrap();
        let states = STATES_EXPANDED.with(std::cell::Cell::get) as f64;
        per_char.push(states / len as f64);
        assert!(
            states > 0.0,
            "no states were expanded for a {len}-char haystack, so the count is meaningless"
        );
        // Every start window does work, so a bounded per-window state count is the whole claim.
        assert!(
            states >= (len / 2) as f64,
            "expected at least one expansion per window at len {len}, saw {states}"
        );
    }

    // If the state space were unbounded the per-character count would climb with the haystack. It
    // should be flat: each window's work depends on the pattern length, not on the input length.
    let first = per_char[0];
    let last = *per_char.last().unwrap();
    assert!(
        last <= first * 1.5,
        "states per character grew with the haystack: {first:.2} at 500 chars, {last:.2} at 8000. \
         The one-edit state space is not bounded, so skipping the dedup is unsound. Series: \
         {per_char:?}"
    );
}

/// Transpositions at a one-edit budget, checked against the brute-force reference.
///
/// Transpositions are the reason this test exists separately. A swap spends the whole budget in one
/// step and consumes *two* text graphemes, so it is the one budget-spending transition whose
/// inlined walk resumes at a different position (`j + 2`) than the others. The general sweep builds
/// patterns and text independently at random, so a swap may simply never arise across all of its
/// cases — which would leave that walk unexercised while every test still passed.
///
/// So this constructs them deliberately: each pattern is transposed into the text, so a swap is
/// always available, and the assertion below additionally requires that swaps were actually
/// *reported*. Without that last check the test could pass by finding nothing, which is the failure
/// mode a coverage test is supposed to rule out.
#[test]
fn search_matches_reference_for_transpositions_at_one_edit() {
    const ALPHABET: &[char] = &['a', 'b', 'c', 'd', 'x', 'y'];
    let fx = Fixture::new();
    let mut rng = Rng(0x5715_0000_1234_5678);
    let mut swaps_seen = 0usize;

    for case in 0..600 {
        let len = 2 + rng.below(5);
        let patterns: Vec<String> = (0..=rng.below(3))
            .map(|_| {
                (0..len)
                    .map(|_| ALPHABET[rng.below(ALPHABET.len())])
                    .collect()
            })
            .collect();

        // Text is built by transposing a pattern's graphemes, optionally with a light mutation,
        // so a transposition is nearly always present.
        let mut text = String::new();
        for _ in 0..=rng.below(3) {
            let src = &patterns[rng.below(patterns.len())];
            let gs: Vec<char> = src.chars().collect();
            let i = rng.below(gs.len().saturating_sub(1).max(1));
            let j = i + 1;
            let mut swapped: String = if j < gs.len() {
                let mut v = gs.clone();
                v.swap(i, j);
                v.into_iter().collect()
            } else {
                gs.clone().into_iter().collect()
            };
            if rng.below(3) == 0 && !swapped.is_empty() {
                // A substitution on top, which must not hide the swap from the engine's ranking.
                let mut v: Vec<char> = swapped.chars().collect();
                let k = rng.below(v.len());
                v[k] = ALPHABET[rng.below(ALPHABET.len())];
                swapped = v.into_iter().collect();
            }
            text.push_str(&swapped);
            text.push(' ');
        }

        let threshold = [0.0f32, 0.5, 0.8][case % 3];
        let engine = FuzzyAhoCorasickBuilder::new()
            .similarity(fx.similarity)
            .penalties(fx.penalties.clone())
            .fuzzy(FuzzyLimits::new().edits(1))
            .build(
                patterns
                    .iter()
                    .map(|p| Pattern::from(p.as_str()))
                    .collect::<Vec<_>>(),
            );

        // Counted from the engine's own matches, since `engine_matches` reduces them to tuples.
        // A transposition is reported as a swap only when it is the *cheapest* alignment for that
        // span, so this undercounts; it just has to be non-zero for the test to prove anything.
        for m in engine
            .search(&text, &SearchOptions::new().threshold(threshold))
            .expect("haystack is small")
            .iter()
        {
            if m.swaps > 0 {
                swaps_seen += 1;
            }
        }

        let got = engine_matches(&engine, &text, threshold);

        let refs: Vec<RefPattern<'_>> = patterns
            .iter()
            .map(|p| RefPattern {
                text: p.as_str(),
                weight: 1.0,
                limits: None,
            })
            .collect();
        let expected = reference_search(&text, &refs, &fx.sim_map, fx.total(1, threshold, false));

        assert_eq!(
            got, expected,
            "case {case}: threshold={threshold} patterns={patterns:?} text={text:?}"
        );
    }

    assert!(
        swaps_seen > 0,
        "no transposition was reported in 600 cases, so this test proved nothing"
    );
}
