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
    fn total(&self, max_edits: u8, threshold: f32, case_insensitive: bool) -> RefConfig {
        RefConfig {
            pen: self.ref_pen,
            min_symbol_similarity: 0.0,
            limits: RefLimits {
                max_edits: Some(max_edits),
                ..RefLimits::default()
            },
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
