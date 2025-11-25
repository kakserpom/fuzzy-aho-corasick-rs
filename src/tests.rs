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
    println!("Result: {:?}", result);
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
    println!("Result: {:#?}", result);
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
    println!("Result: {:#?}", result);
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
    println!("{:?}", res);
    assert!(res.iter().any(|m| m.text.to_lowercase() == "юрий"));

    let res = engine.segment_text("ЮРИЙГАГАРИН", 0.9);
    println!("{:?}", res);

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
    let result = fac.search("saddmhussein", 0.3);
    println!("{:?}", result);
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
    println!("{:?}", result);
    assert_eq!(result.iter().count(), 0);
}

#[test]
fn test_regression_2() {
    let engine = FuzzyAhoCorasickBuilder::new()
        .build([Pattern::from("TOLA").fuzzy(FuzzyLimits::new().edits(2))]);

    let result = engine.search_non_overlapping("TOL", 0.5);
    println!("\nResult: {:?}", result);
    assert_eq!(result.iter().any(|x| x.text == "TOL"), true);
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
