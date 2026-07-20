/* -------------------------------------------------------------------------
 *  Tests
 * ---------------------------------------------------------------------- */
use crate::{FuzzyAhoCorasick, FuzzyAhoCorasickBuilder, FuzzyLimits, FuzzyPenalties, Pattern};

fn make_engine() -> FuzzyAhoCorasick {
    FuzzyAhoCorasickBuilder::new()
        .fuzzy(FuzzyLimits::new().edits(2))
        .build(["saddam", "hussein"])
}

#[test]
fn test_non_overlapping_regression_0() {
    let fac = FuzzyAhoCorasickBuilder::new()
        .fuzzy(FuzzyLimits::new().edits(2))
        .case_insensitive(true)
        .build(["NA", "MENA"]);
    let result = fac.search_non_overlapping("NA MENA", 0.6);
    println!("Result: {result:?}");
    assert!(
        result
            .iter()
            .any(|m| m.pattern.as_str() == "MENA" && m.text == "MENA")
    );
}

#[test]
fn test_non_overlapping_regression_2() {
    let fac = FuzzyAhoCorasickBuilder::new()
        .fuzzy(FuzzyLimits::new().edits(1))
        .case_insensitive(true)
        .build(["KO", "KO", "LWIN"]);
    let result = fac.search_non_overlapping("KWO KO LWIN", 0.6);
    println!("Result: {result:#?}");
    assert!(
        result
            .iter()
            .any(|m| m.pattern.as_str() == "KO" && m.text == "KWO")
    );
}
#[test]
fn test_non_overlapping_regression_3() {
    let fac = FuzzyAhoCorasickBuilder::new()
        .fuzzy(FuzzyLimits::new().edits(1))
        .case_insensitive(true)
        .build(["AL", "WASEL", "AND", "BABEL", "GENERAL", "TRADING", "LLC"]);
    let result = fac.search_non_overlapping_unique("AL WASL ANT BBEL GNERAL TRATING LC", 0.6);
    println!("Result: {result:#?}");
    assert!(
        result
            .iter()
            .any(|m| m.pattern.as_str() == "WASEL" && m.text == "WASL")
    );
    assert!(
        result
            .iter()
            .any(|m| m.pattern.as_str() == "BABEL" && m.text == "BBEL")
    );
}

#[test]
fn test_case_insensitive_ascii() {
    let engine = FuzzyAhoCorasickBuilder::new()
        .case_insensitive(true)
        .build(["world"]);
    let res = engine.search("HeLlO WoRlD", 0.9);
    assert!(res.iter().any(|m| m.text.eq_ignore_ascii_case("world")));
}

#[test]
fn test_unicode_cyrillic() {
    let engine = FuzzyAhoCorasickBuilder::new()
        .case_insensitive(true)
        .build(["юрий"]);
    let res = engine.search("ЮРИЙ ГАГАРИН", 0.9);
    println!("{res:?}");
    assert!(res.iter().any(|m| m.text.to_lowercase() == "юрий"));

    let res = engine.segment_text("ЮРИЙГАГАРИН", 0.9);
    println!("{res:?}");

    assert_eq!(res, "ЮРИЙ ГАГАРИН");
}

#[test]
fn test_exact_match() {
    let fac = make_engine();
    let result = fac.search("saddamhussein", 0.5);
    assert!(
        result
            .iter()
            .any(|m| m.pattern.as_str() == "saddam" && m.text == "saddam")
    );
    assert!(
        result
            .iter()
            .any(|m| m.pattern.as_str() == "hussein" && m.text == "hussein")
    );
}

#[test]
fn test_extra_letter() {
    let fac = make_engine();
    let result = fac.search("saddammhussein", 0.3);
    assert!(
        result
            .iter()
            .any(|m| m.pattern.as_str() == "saddam" && m.text == "saddam")
    );
}

#[test]
fn test_missing_letter() {
    let fac = make_engine();
    let result = fac.search("saddmhussin", 0.3);
    println!("{result:?}");
    assert!(
        result
            .iter()
            .any(|m| m.pattern.as_str() == "saddam" && m.text == "saddm")
    );
}

#[test]
fn test_substitution() {
    let fac = make_engine();
    let result = fac.search("saddamhuzein", 0.2);
    assert!(
        result
            .iter()
            .any(|m| m.pattern.as_str() == "hussein" && m.text == "huzein")
    );
}

#[test]
fn test_swap() {
    let fac = FuzzyAhoCorasickBuilder::new()
        .fuzzy(FuzzyLimits::new().edits(2))
        .case_insensitive(true)
        .build(["ALI", "KONY"]);
    let result = fac.search_non_overlapping("ALIKOYN", 0.6);
    assert!(
        result
            .iter()
            .any(|m| m.pattern.as_str() == "KONY" && m.text == "KOYN")
    );
}

#[test]
fn test_big() {
    let fac = FuzzyAhoCorasickBuilder::new()
        .fuzzy(FuzzyLimits::new().edits(1))
        .case_insensitive(true)
        .build(["tincidunt", "porta"]);
    let text = "Lorem ipsum dolor sit amet, consectetur adipiscing elit. Vestibulum eros ipsum, tincidutn eu metus ut, commodo accumsan mi. Vestibulum porta, orci nec ullamcorper posuere, eros tortor pharetra est, at porttitor mi leo a velit. Aenean sollicitudin mauris elit, ultricies congue dui vulputate in. In hac habitasse platea dictumst. Nam iaculis sagittis justo a condimentum. Curabitur sed rhoncus dolor. Lorem ipsum dolor sit amet, consectetur adipiscing elit. Vivamus egestas congue lorem, in convallis magna viverra quis. Maecenas fringilla mollis arcu quis maximus. Maecenas tincidunt semper vestibulum. Donec aliquet leo at molestie elementum. Nulla venenatis iaculis gravida. Phasellus at pulvinar odio. Etiam bibendum tempor purus at dignissim. Nam a turpis ante. Etiam imperdiet justo sit amet quam tristique porttitor. Cras ultrices tellus et dolor lobortis tempor. Suspendisse eu mi nec nisi sollicitudin pharetra. Proin imperdiet elementum ullamcorper. Nam imperdiet quis mi at vulputate. Vivamus pulvinar, quam et tempus sollicitudin, justo dolor venenatis lacus, sit amet dignissim ex quam ut est. Suspendisse feugiat libero a augue malesuada sagittis. Curabitur vel magna neque. Praesent eu nulla faucibus, egestas eros sit amet, elementum quam. Fusce porttitor et lacus vitae maximus. Ut viverra eu sem sed lobortis. Fusce feugiat vestibulum posuere. Integer erat mauris, tempor eu magna vitae, varius rutrum elit. Proin mattis, nunc at porta commodo, erat urna viverra ante, vitae feugiat velit dolor ac quam. Nulla semper elit in neque mollis molestie. Aenean a augue scelerisque, tincidunt odio ut, finibus erat. Integer feugiat eros ac dolor tempus, sed varius lectus ullamcorper. Orci varius natoque penatibus et magnis dis parturient montes, nascetur ridiculus mus.";
    let result = fac.search_non_overlapping(text, 0.8);
    assert!(result.iter().any(|x| x.text == "tincidutn"), "{result:?}");
    assert!(result.iter().any(|x| x.text == "tincidunt"), "{result:?}");
    assert!(result.iter().any(|x| x.text == "porta"), "{result:?}");
}

#[test]
fn test_overlap_vs_nonoverlap() {
    let engine = FuzzyAhoCorasickBuilder::new().build([("saddam", 1.0, 2), ("ddamhu", 1.0, 2)]);

    let matches = engine.search("saddamddamhu", 0.5);
    println!();
    println!("{:?}", matches[0]);
    println!();
    println!("{:?}", matches[1]);
    assert!(
        matches
            .iter()
            .any(|m| m.pattern.as_str() == "saddam" && m.text == "saddam")
    );
    assert!(
        matches
            .iter()
            .any(|m| m.pattern.as_str() == "ddamhu" && m.text == "ddamhu"),
        "{matches:?}"
    );

    let matches_nonoverlap = engine.search_non_overlapping("saddamhussein", 0.7);
    assert_eq!(matches_nonoverlap.len(), 1, "{matches_nonoverlap:?}");

    let matches_nonoverlap_two = engine.search_non_overlapping("sadam ddamhu", 0.4);
    assert_eq!(
        matches_nonoverlap_two.len(),
        2,
        "{matches_nonoverlap_two:?}"
    );
    assert!(
        matches_nonoverlap_two
            .iter()
            .any(|m| m.pattern.as_str() == "saddam" && m.text == "sadam"),
        "{matches_nonoverlap_two:?}"
    );
    assert!(
        matches_nonoverlap_two
            .iter()
            .any(|m| m.pattern.as_str() == "ddamhu" && m.text == "ddamhu"),
        "{matches_nonoverlap_two:?}"
    );
}

#[test]
fn test_adjustable_penalties() {
    let engine_strict = FuzzyAhoCorasickBuilder::new().build([("hussein", 1.0, 2)]);
    let strict = engine_strict.search("huzein", 0.3);
    assert!(
        strict
            .iter()
            .any(|m| m.pattern.as_str() == "hussein" && m.text == "huzein")
    );

    let engine = FuzzyAhoCorasickBuilder::new()
        .penalties(
            FuzzyPenalties::default()
                .substitution(0.8)
                .insertion(0.95)
                .deletion(0.95),
        )
        .build([("hussein", 1.0, 3)]);
    let loose = engine.search("huzein", 0.2);
    assert!(
        loose
            .iter()
            .any(|m| m.pattern.as_str() == "hussein" && m.text == "huzein")
    );
}

#[test]
fn test_regression_1() {
    let engine = FuzzyAhoCorasickBuilder::new()
        .case_insensitive(true)
        .build(["CO"]);

    let result = engine.search("CA", 0.8);
    println!("{result:?}");
    assert_eq!(result.iter().count(), 0);
}

#[test]
fn test_regression_2() {
    let engine = FuzzyAhoCorasickBuilder::new()
        .build([Pattern::from("TOLA").fuzzy(FuzzyLimits::new().edits(2))]);

    let result = engine.search_non_overlapping("TOL", 0.5);
    println!("\nResult: {result:?}");
    assert!(result.iter().any(|x| x.text == "TOL"));
}

#[test]
fn test_segment_text() {
    let engine = FuzzyAhoCorasickBuilder::new()
        .fuzzy(FuzzyLimits::new().edits(3))
        .build(["saddam", "hussein"]);
    assert_eq!(engine.segment_text("sadamhusein", 0.8), "sadam husein");
    assert_eq!(
        engine.segment_text("sadamhuseinaltikriti", 0.8),
        "sadam husein altikriti"
    );
}

#[test]
fn test_segment_readme() {
    let engine = FuzzyAhoCorasickBuilder::new()
        .fuzzy(FuzzyLimits::new().edits(1))
        .build(["input", "more"]);
    let matches = engine.search_non_overlapping("someinptandm0re", 0.75);
    let segmented_text = matches.segment_text();
    assert_eq!(segmented_text, "some inpt and m0re");
}

#[test]
fn test_segment_name() {
    let engine = FuzzyAhoCorasickBuilder::new()
        .fuzzy(FuzzyLimits::new().edits(3))
        .build(["SHANE", "DOMINIC", "CRAWFORD"]);
    assert_eq!(
        engine.segment_text("SHANEDOM INICCRAWFORD", 0.8),
        "SHANE DOM INIC CRAWFORD"
    );
}

#[test]
fn test_segment_text2() {
    let engine = FuzzyAhoCorasickBuilder::new()
        .case_insensitive(true)
        .build(["HASAN", "JAMAL", "HUSSEIN", "ZEINIYE"]);
    assert_eq!(
        engine.segment_text("ZEINIYEHussEINHASaNJAMAL", 0.8),
        "ZEINIYE HussEIN HASaN JAMAL"
    );
}

#[test]
fn test_fail() {
    let engine = FuzzyAhoCorasickBuilder::new().build(["saddam", "hussein"]);
    assert_eq!(engine.segment_text("sadam husein", 0.8), "sadam husein");
}

#[test]
fn test_fuzzy_replace() {
    let source = "PUBLIC JOINT STOCK COMPANY GAZPROM";
    let result = FuzzyAhoCorasickBuilder::new()
        .case_insensitive(true)
        .build_replacer([
            ("PUBLIC JOINT STOCK COMPANY", "PJSC"),
            ("PUBLIC JOINT STOCK", "PJSC"),
            ("LIMITED LIABILITY COMPANY", "LLC"),
            ("LIMITED LIABILITY", "LLC"),
        ])
        .replace(source, 0.8);
    assert_eq!(result, "PJSC GAZPROM");
}

#[test]
fn test_fuzzy_replace_fn() {
    assert_eq!(
        FuzzyAhoCorasickBuilder::new()
            .case_insensitive(true)
            .build(["hair", "bear", "wuzzy"])
            .replace(
                "Fuzzy Wuzzy was a hair. Fuzzy Wuzzy had no bear.",
                |m| {
                    match m.text {
                        "bear" => Some("hair"),
                        "hair" => Some("bear"),
                        _ => None,
                    }
                },
                0.8,
            ),
        "Fuzzy Wuzzy was a bear. Fuzzy Wuzzy had no hair."
    );
}

#[test]
fn test_longer_match_preference() {
    let engine = FuzzyAhoCorasickBuilder::new().build(["JOINT STOCK COMPANY", "STOCK"]);
    let result = engine.search_non_overlapping("JOINT STOCK COMPANY GAZPROM", 0.8);
    assert!(
        result
            .iter()
            .any(|m| m.pattern.as_str() == "JOINT STOCK COMPANY")
    );
    assert!(!result.iter().any(|m| m.pattern.as_str() == "STOCK"));
}

#[test]
fn test_regression_0() {
    let engine = FuzzyAhoCorasickBuilder::new()
        .fuzzy(FuzzyLimits::new().edits(2).substitutions(1))
        .case_insensitive(true)
        .build(["zavod"]);

    let result = engine.search_non_overlapping("NARODNY", 0.8);
    assert!(result.is_empty());
}

#[test]
fn test_readme() {
    let replacer = FuzzyAhoCorasickBuilder::new()
        .fuzzy(FuzzyLimits::new().substitutions(1))
        .case_insensitive(true)
        .build_replacer([("foo", "bar"), ("baz", "qux")]);

    let out = replacer.replace("fo0 and BAZ!", 0.7);
    assert_eq!(out, "bar and qux!");
}

#[test]
fn test_country() {
    let replacer = FuzzyAhoCorasickBuilder::new()
        .fuzzy(FuzzyLimits::new().edits(5))
        .case_insensitive(true)
        .build_replacer([("CZECHOSLOVAKIA", "SERBIA")]);

    let out = replacer.replace("CHEKHOSLOVAKIA", 0.7);
    assert_eq!(out, "SERBIA");
}

#[test]
fn test_strip_prefix() {
    assert_eq!(
        FuzzyAhoCorasickBuilder::new()
            .fuzzy(FuzzyLimits::new().edits(1))
            .case_insensitive(true)
            .build(["LOREM", "IPSUM"])
            .strip_prefix("LrEM ISuM Lorm ZZZ", 0.8),
        "ZZZ"
    );
}

#[test]
fn test_strip_postfix() {
    assert_eq!(
        FuzzyAhoCorasickBuilder::new()
            .fuzzy(FuzzyLimits::new().edits(1))
            .case_insensitive(true)
            .build(["LOREM", "IPSUM"])
            .strip_postfix("ZZZ LrEM ISuM Lorm", 0.8),
        "ZZZ"
    );
}
#[test]
fn test_split() {
    assert_eq!(
        FuzzyAhoCorasickBuilder::new()
            .fuzzy(FuzzyLimits::new().edits(1))
            .case_insensitive(true)
            .build(["LOREM", "IPSUM"])
            .split("ZZZLrEMISuMAAA", 0.8)
            .collect::<Vec<_>>(),
        ["ZZZ", "AAA"]
    );
}

#[test]
fn test_beam_search() {
    // Test that beam search still finds matches (may find fewer with very small beam)
    let engine_no_beam = FuzzyAhoCorasickBuilder::new()
        .fuzzy(FuzzyLimits::new().edits(2))
        .case_insensitive(true)
        .build(["saddam", "hussein"]);

    let engine_with_beam = FuzzyAhoCorasickBuilder::new()
        .fuzzy(FuzzyLimits::new().edits(2))
        .case_insensitive(true)
        .beam_width(100)
        .build(["saddam", "hussein"]);

    let text = "saddamhusein";

    let results_no_beam = engine_no_beam.search_non_overlapping(text, 0.7);
    let results_with_beam = engine_with_beam.search_non_overlapping(text, 0.7);

    // Both should find matches
    assert!(!results_no_beam.is_empty(), "No beam should find matches");
    assert!(
        !results_with_beam.is_empty(),
        "Beam search should also find matches"
    );

    // Check that beam search found the key patterns
    assert!(
        results_with_beam
            .iter()
            .any(|m| m.pattern.as_str() == "saddam")
    );
}

#[test]
fn test_truncated_walijan() {
    // Pattern = "WALIJAN" (7 chars), Haystack = "alijan" (6 chars)
    // Need 1 deletion at the start to match
    let engine = FuzzyAhoCorasickBuilder::new()
        .case_insensitive(true)
        .build([Pattern::from("WALIJAN").fuzzy(FuzzyLimits::new().edits(3))]);

    let result = engine.search("alijan", 0.7);
    println!("\nResult for alijan: {result:?}");

    // This should find WALIJAN with text="alijan"
    assert!(
        result.iter().any(|m| m.pattern.as_str() == "WALIJAN"),
        "Should find WALIJAN in 'alijan' with deletions. Results: {result:?}"
    );
}

#[test]
fn test_truncated_short() {
    // Pattern = "TOLA" (4 chars), Haystack = "OLA" (3 chars)
    // Need 1 deletion at the start to match
    let engine = FuzzyAhoCorasickBuilder::new()
        .case_insensitive(true)
        .build([Pattern::from("TOLA").fuzzy(FuzzyLimits::new().edits(2))]);

    let result = engine.search("OLA", 0.5);
    println!("\nResult for OLA: {result:?}");

    assert!(
        result.iter().any(|m| m.text == "OLA"),
        "Should find TOLA in 'OLA' with deletion. Results: {result:?}"
    );
}

#[test]
fn test_truncated_with_global_limits() {
    // Use GLOBAL limits instead of pattern-specific limits
    let engine = FuzzyAhoCorasickBuilder::new()
        .case_insensitive(true)
        .fuzzy(FuzzyLimits::new().edits(2)) // Global limits
        .build(["TOLA"]);

    let result = engine.search("OLA", 0.5);
    println!("\nResult for OLA with global limits: {result:?}");

    assert!(
        result.iter().any(|m| m.text == "OLA"),
        "Should find TOLA in 'OLA' with global limits. Results: {result:?}"
    );
}

#[test]
fn test_truncated_walijan_with_global_limits() {
    // Use GLOBAL limits for WALIJAN
    let engine = FuzzyAhoCorasickBuilder::new()
        .case_insensitive(true)
        .fuzzy(FuzzyLimits::new().edits(3)) // Global limits
        .build(["WALIJAN"]);

    let result = engine.search("alijan", 0.7);
    println!("\nResult for alijan with global limits: {result:?}");

    assert!(
        result.iter().any(|m| m.pattern.as_str() == "WALIJAN"),
        "Should find WALIJAN in 'alijan' with global limits. Results: {result:?}"
    );
}

#[test]
fn test_phonetic_td_substitution() {
    // Test T↔D phonetic substitution: "Tjamel" should match "DJAMEL"
    let engine = FuzzyAhoCorasickBuilder::new()
        .case_insensitive(true)
        .build([Pattern::from("DJAMEL").fuzzy(FuzzyLimits::new().edits(3))]);

    let result = engine.search("Tjamel", 0.5);
    println!("\nResult for 'Tjamel' vs 'DJAMEL' (0.5): {result:?}");

    let result2 = engine.search("Tjamel", 0.7);
    println!("Result for 'Tjamel' vs 'DJAMEL' (0.7): {result2:?}");

    // Calculate expected similarity:
    // Pattern: DJAMEL (6 chars)
    // Query: Tjamel
    // T↔D substitution: consonant-consonant similarity = 0.4, penalty = 1.43 * (1 - 0.4) = 0.858
    // Similarity = (6 - 0.858) / 6 = 0.857

    assert!(
        result.iter().any(|m| m.pattern.as_str() == "DJAMEL"),
        "Should find DJAMEL in 'Tjamel' with T↔D substitution. Results: {result:?}"
    );
}

#[test]
fn test_missing_middle_char() {
    // "Mmir" should match "MOMIR" (missing 'O')
    let engine = FuzzyAhoCorasickBuilder::new()
        .case_insensitive(true)
        .build([Pattern::from("MOMIR").fuzzy(FuzzyLimits::new().edits(3))]);

    let result = engine.search("Mmir", 0.5);
    println!("\nResult for 'Mmir' vs 'MOMIR' (0.5): {result:?}");

    let result2 = engine.search("Mmir", 0.7);
    println!("Result for 'Mmir' vs 'MOMIR' (0.7): {result2:?}");

    // For 5-char pattern with 1 deletion at position 2:
    // similarity = (5 - 0.91) / 5 = 0.818
    assert!(
        result.iter().any(|m| m.pattern.as_str() == "MOMIR"),
        "Should find MOMIR in 'Mmir'. Results: {result:?}"
    );
}

#[test]
fn test_siic_simic() {
    // "SIIC" (4 chars) should match "SIMIC" (5 chars) - missing 'M'
    let engine = FuzzyAhoCorasickBuilder::new()
        .case_insensitive(true)
        .build([Pattern::from("SIMIC").fuzzy(FuzzyLimits::new().edits(3))]);

    let result = engine.search("SIIC", 0.7);
    println!("\nResult for 'SIIC' vs 'SIMIC': {result:?}");
}

#[test]
fn test_aminulah_aminullah() {
    // "Aminulah" should match "AMINULLAH" - missing 'L'
    let engine = FuzzyAhoCorasickBuilder::new()
        .case_insensitive(true)
        .build([Pattern::from("AMINULLAH").fuzzy(FuzzyLimits::new().edits(3))]);

    let result = engine.search("Aminulah", 0.7);
    println!("\nResult for 'Aminulah' vs 'AMINULLAH': {result:?}");
}

#[test]
fn test_jaar_jafar() {
    // "Jaar" should match "JAFAR" - missing 'F'
    let engine = FuzzyAhoCorasickBuilder::new()
        .case_insensitive(true)
        .build([Pattern::from("JAFAR").fuzzy(FuzzyLimits::new().edits(3))]);

    let result = engine.search("Jaar", 0.7);
    println!("\nResult for 'Jaar' vs 'JAFAR': {result:?}");
}

#[test]
fn test_aminullah_aminulah() {
    let engine = FuzzyAhoCorasickBuilder::new()
        .case_insensitive(true)
        .build([Pattern::from("AMINULLAH").fuzzy(FuzzyLimits::new().edits(3))]);

    let result = engine.search("Aminulah", 0.7);
    println!("Result for 'Aminulah' vs 'AMINULLAH': {result:?}");
    assert!(!result.inner.is_empty(), "AMINULLAH should match Aminulah");
}

/// Regression: searching a long token against fuzzy patterns that allow insertions and
/// deletions used to explode combinatorially (no state dedup), consuming tens of GB and
/// dozens of seconds for a single ~29-char word. With state deduplication it must complete
/// near-instantly. See amlsearch issue: "Russische ... Ruckversicherungsgesellschaft JSC" OOM.
#[test]
fn test_long_token_no_blowup_regression() {
    // Mirror the real-world entity automaton: a mix of short and long patterns, each fuzzy
    // with the same generous limits that triggered the blow-up.
    let limits = FuzzyLimits::new()
        .edits(3)
        .substitutions(1)
        .deletions(2)
        .insertions(2)
        .swaps(0);
    let patterns = [
        "SA",
        "LES",
        "CO",
        "JSC",
        "LTD",
        "BANK",
        "GROUP",
        "COMPANY",
        "CORPORATION",
        "JOINT STOCK COMPANY",
        "FEDERAL STATE BUDGETARY INSTITUTION OF SCIENCE",
    ]
    .into_iter()
    .map(|p| Pattern::from(p.to_owned()).fuzzy(limits.clone()));

    let engine = FuzzyAhoCorasickBuilder::new()
        .case_insensitive(true)
        .build(patterns);

    let haystack = "RUSSISCHE NATIONALE RUCKVERSICHERUNGSGESELLSCHAFT JSC";
    let start = std::time::Instant::now();
    let result = engine.search_greedy(haystack, 0.8);
    let elapsed = start.elapsed();

    println!("elapsed={elapsed:?} matches={}", result.inner.len());
    // Before the dedup fix this took ~20s+ and allocated tens of GB. A generous ceiling well
    // below that pathological behaviour while tolerating slow CI.
    assert!(
        elapsed < std::time::Duration::from_secs(2),
        "long-token fuzzy search took {elapsed:?} — state-dedup regression"
    );
    // Correctness sanity: the trailing exact "JSC" token must still be found.
    assert!(
        result.iter().any(|m| m.pattern.as_str() == "JSC"),
        "expected the JSC token to match"
    );
}

#[test]
fn test_auto_beam_exact_below_budget_and_bounded_above() {
    // A budget far above what any small search reaches: auto_beam must never engage, so results are
    // byte-for-byte identical to the unlimited search.
    let patterns = [
        "saddam",
        "hussein",
        "tincidunt",
        "porta",
        "vestibulum",
        "accumsan",
    ];
    let text = "this is a saddamhu example with multiple saddam and tincidutn matches";
    let exact = FuzzyAhoCorasickBuilder::new()
        .fuzzy(FuzzyLimits::new().edits(2))
        .case_insensitive(true)
        .build(patterns);
    let huge_budget = FuzzyAhoCorasickBuilder::new()
        .fuzzy(FuzzyLimits::new().edits(2))
        .case_insensitive(true)
        .auto_beam(usize::MAX, 8)
        .build(patterns);
    assert_eq!(
        exact.search(text, 0.6).inner,
        huge_budget.search(text, 0.6).inner,
        "auto_beam must be exact when the budget is never reached"
    );

    // A tiny budget forces the beam on almost immediately; the obvious high-similarity matches must
    // still be found (the beam keeps the lowest-penalty candidates).
    let beamed = FuzzyAhoCorasickBuilder::new()
        .fuzzy(FuzzyLimits::new().edits(2))
        .case_insensitive(true)
        .auto_beam(1, 16)
        .build(patterns);
    let matched: Vec<&str> = beamed
        .search(text, 0.6)
        .iter()
        .map(|m| m.pattern.as_str())
        .collect();
    assert!(
        matched.contains(&"saddam"),
        "expected saddam, got {matched:?}"
    );
}

#[test]
fn test_multi_char_mapping_bidirectional() {
    // "æ" <-> "ae" applies in both directions and, at score 1.0, yields a perfect-quality match
    // (it still counts as one substitution).
    let ae = FuzzyAhoCorasickBuilder::new()
        .case_insensitive(true)
        .fuzzy(FuzzyLimits::new().edits(1))
        .mapping("æ", "ae")
        .build(["encyclopaedia"]);
    let m = ae.search("encyclopædia", 0.95);
    assert_eq!(
        m.len(),
        1,
        "æ in the haystack should match the 'ae' pattern"
    );
    assert_eq!(m[0].substitutions, 1);
    assert!(
        m[0].similarity > 0.999,
        "score-1.0 mapping should be penalty-free"
    );

    let ea = FuzzyAhoCorasickBuilder::new()
        .case_insensitive(true)
        .fuzzy(FuzzyLimits::new().edits(1))
        .mapping("æ", "ae")
        .build(["encyclopædia"]);
    assert_eq!(
        ea.search("encyclopaedia", 0.95).len(),
        1,
        "'ae' in the haystack should match the 'æ' pattern"
    );
}

#[test]
fn test_multi_char_mapping_many_to_one() {
    // "ks" <-> "x", covering both the 2->1 and 1->2 grapheme directions.
    let mk = |patterns: [&str; 1]| {
        FuzzyAhoCorasickBuilder::new()
            .case_insensitive(true)
            .fuzzy(FuzzyLimits::new().edits(1))
            .mapping("ks", "x")
            .build(patterns)
    };
    assert_eq!(mk(["alexandr"]).search("aleksandr", 0.95).len(), 1);
    assert_eq!(mk(["aleksandr"]).search("alexandr", 0.95).len(), 1);
}

#[test]
fn test_multi_char_mapping_counts_as_edit() {
    // Consistent with single-character substitutions: a mapping consumes the edit budget.
    let build = |edits| {
        FuzzyAhoCorasickBuilder::new()
            .case_insensitive(true)
            .fuzzy(FuzzyLimits::new().edits(edits))
            .mapping("ß", "ss")
            .build(["strasse"])
    };
    assert!(
        build(0u8).search("straße", 0.9).is_empty(),
        "with edits(0) the mapping must be rejected, like any substitution"
    );
    assert_eq!(build(1u8).search("straße", 0.9).len(), 1);
}

#[test]
fn test_multi_char_mapping_scored_penalty() {
    let exact = FuzzyAhoCorasickBuilder::new()
        .case_insensitive(true)
        .fuzzy(FuzzyLimits::new().edits(1))
        .mapping("ks", "x")
        .build(["alexandr"]);
    let scored = FuzzyAhoCorasickBuilder::new()
        .case_insensitive(true)
        .fuzzy(FuzzyLimits::new().edits(1))
        .mapping_scored("ks", "x", 0.8)
        .build(["alexandr"]);
    let se = exact.search("aleksandr", 0.5)[0].similarity;
    let ss = scored.search("aleksandr", 0.5)[0].similarity;
    assert!(se > 0.999, "score 1.0 is penalty-free (got {se})");
    assert!(
        ss < se,
        "a scored (<1.0) mapping must lower the similarity (got {ss} vs {se})"
    );
}

#[test]
fn test_no_mapping_is_unaffected() {
    // Without a mapping, 'æ' is unrelated to "ae" and must not match.
    let e = FuzzyAhoCorasickBuilder::new()
        .case_insensitive(true)
        .fuzzy(FuzzyLimits::new().edits(1))
        .build(["encyclopaedia"]);
    assert!(e.search("encyclopædia", 0.9).is_empty());
}

#[test]
fn test_streaming_apis_match_whole_input() {
    // A >512 KiB input exercises several 256 KiB windows (incl. boundary-spanning needles). Needles
    // are separated by filler that cannot fuzzy-match "needle", so per-window and whole-input
    // non-overlapping selection agree and results can be compared directly.
    let engine = FuzzyAhoCorasickBuilder::new()
        .fuzzy(FuzzyLimits::new().edits(1))
        .case_insensitive(true)
        .build(["needle"]);
    let filler = "the quick brown fox ".repeat(50);
    let mut input = String::new();
    while input.len() < 600_000 {
        input.push_str(&filler);
        input.push_str("needle ");
    }

    // Ground truth: search the whole thing in one shot (input is < 4 GiB).
    let mut truth: Vec<(u64, u64, usize)> = engine
        .search_non_overlapping(&input, 0.8)
        .iter()
        .map(|m| (m.start as u64, m.end as u64, m.pattern_index))
        .collect();
    truth.sort_unstable();
    assert!(
        truth.len() > 300,
        "expected many needles across several windows"
    );

    let collect_sorted = |mut v: Vec<(u64, u64, usize)>| {
        v.sort_unstable();
        v
    };

    // 1) callback
    let mut cb = Vec::new();
    engine
        .search_stream(input.as_bytes(), 0.8, |m| {
            cb.push((m.start, m.end, m.pattern_index));
        })
        .unwrap();
    assert_eq!(
        collect_sorted(cb),
        truth,
        "search_stream must equal whole-input search"
    );

    // 2) iterator
    let it: Vec<_> = engine
        .stream_matches(input.as_bytes(), 0.8)
        .map(Result::unwrap)
        .map(|m| (m.start, m.end, m.pattern_index))
        .collect();
    assert_eq!(
        collect_sorted(it),
        truth,
        "stream_matches must equal whole-input search"
    );

    // 3) parallel
    let mut par = Vec::new();
    engine
        .search_stream_parallel(input.as_bytes(), 0.8, 4, |m| {
            par.push((m.start, m.end, m.pattern_index));
        })
        .unwrap();
    assert_eq!(
        collect_sorted(par),
        truth,
        "parallel stream must equal whole-input search"
    );

    // Offsets/text are consistent with the source.
    engine
        .search_stream(input.as_bytes(), 0.8, |m| {
            assert_eq!(&input[m.start as usize..m.end as usize], m.text);
        })
        .unwrap();
}

#[test]
fn test_streaming_empty_input() {
    let engine = FuzzyAhoCorasickBuilder::new().build(["x"]);
    let mut hits = 0;
    let n = engine.search_stream(&b""[..], 0.8, |_| hits += 1).unwrap();
    assert_eq!((hits, n), (0, 0));
}
