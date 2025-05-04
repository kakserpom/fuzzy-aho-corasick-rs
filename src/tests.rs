/* -------------------------------------------------------------------------
 *  Tests
 * ---------------------------------------------------------------------- */
use crate::{FuzzyAhoCorasick, FuzzyAhoCorasickBuilder, FuzzyLimits, FuzzyPenalties};

fn make_engine() -> FuzzyAhoCorasick {
    FuzzyAhoCorasickBuilder::new()
        .fuzzy(FuzzyLimits::new().edits(2))
        .build(["saddam", "hussein"])
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
}

#[test]
fn test_exact_match() {
    let fac = make_engine();
    let result = fac.search("saddamhussein", 0.5);
    assert!(
        result
            .iter()
            .any(|m| m.pattern == "saddam" && m.text == "saddam")
    );
    assert!(
        result
            .iter()
            .any(|m| m.pattern == "hussein" && m.text == "hussein")
    );
}

#[test]
fn test_extra_letter() {
    let fac = make_engine();
    let result = fac.search("saddammhussein", 0.3);
    assert!(
        result
            .iter()
            .any(|m| m.pattern == "saddam" && m.text == "saddam")
    );
}

#[test]
fn test_missing_letter() {
    let fac = make_engine();
    let result = fac.search("saddmhussein", 0.3);
    assert!(
        result
            .iter()
            .any(|m| m.pattern == "saddam" && m.text == "saddm")
    );
}

#[test]
fn test_substitution() {
    let fac = make_engine();
    let result = fac.search("saddamhuzein", 0.2);
    assert!(
        result
            .iter()
            .any(|m| m.pattern == "hussein" && m.text == "huzein")
    );
}

#[test]
fn test_swap() {
    let fac = FuzzyAhoCorasickBuilder::new()
        .fuzzy(FuzzyLimits::new().edits(3))
        .case_insensitive(true)
        .build(["ALI", "KONY"]);
    let result = fac.search("ALIKOYN", 0.8);
    assert!(
        result
            .iter()
            .any(|m| m.pattern == "KONY" && m.text == "KOYN")
    );
}

#[test]
fn test_big() {
    let fac = FuzzyAhoCorasickBuilder::new()
        .fuzzy(FuzzyLimits::new().edits(1))
        .case_insensitive(true)
        .non_overlapping(true)
        .build(["tincidunt", "porta"]);
    let text = "Lorem ipsum dolor sit amet, consectetur adipiscing elit. Vestibulum eros ipsum, tincidutn eu metus ut, commodo accumsan mi. Vestibulum porta, orci nec ullamcorper posuere, eros tortor pharetra est, at porttitor mi leo a velit. Aenean sollicitudin mauris elit, ultricies congue dui vulputate in. In hac habitasse platea dictumst. Nam iaculis sagittis justo a condimentum. Curabitur sed rhoncus dolor. Lorem ipsum dolor sit amet, consectetur adipiscing elit. Vivamus egestas congue lorem, in convallis magna viverra quis. Maecenas fringilla mollis arcu quis maximus. Maecenas tincidunt semper vestibulum. Donec aliquet leo at molestie elementum. Nulla venenatis iaculis gravida. Phasellus at pulvinar odio. Etiam bibendum tempor purus at dignissim. Nam a turpis ante. Etiam imperdiet justo sit amet quam tristique porttitor. Cras ultrices tellus et dolor lobortis tempor. Suspendisse eu mi nec nisi sollicitudin pharetra. Proin imperdiet elementum ullamcorper. Nam imperdiet quis mi at vulputate. Vivamus pulvinar, quam et tempus sollicitudin, justo dolor venenatis lacus, sit amet dignissim ex quam ut est. Suspendisse feugiat libero a augue malesuada sagittis. Curabitur vel magna neque. Praesent eu nulla faucibus, egestas eros sit amet, elementum quam. Fusce porttitor et lacus vitae maximus. Ut viverra eu sem sed lobortis. Fusce feugiat vestibulum posuere. Integer erat mauris, tempor eu magna vitae, varius rutrum elit. Proin mattis, nunc at porta commodo, erat urna viverra ante, vitae feugiat velit dolor ac quam. Nulla semper elit in neque mollis molestie. Aenean a augue scelerisque, tincidunt odio ut, finibus erat. Integer feugiat eros ac dolor tempus, sed varius lectus ullamcorper. Orci varius natoque penatibus et magnis dis parturient montes, nascetur ridiculus mus.";
    let result = fac.search(text, 0.8);
    assert!(result.iter().any(|x| x.text == "tincidutn"));
    assert!(result.iter().any(|x| x.text == "tincidunt"));
    assert!(result.iter().any(|x| x.text == "porta"));
}

#[test]
fn test_overlap_vs_nonoverlap() {
    let patterns = ["saddam", "ddamhu"];
    let engine = FuzzyAhoCorasickBuilder::new()
        .non_overlapping(false)
        .build([("saddam", 1.0, 2), ("ddamhu", 1.0, 2)]);

    let matches = engine.search("saddamddamhu", 0.5);
    assert!(
        matches
            .iter()
            .any(|m| m.pattern == "saddam" && m.text == "saddam")
    );
    assert!(
        matches
            .iter()
            .any(|m| m.pattern == "ddamhu" && m.text == "ddamhu"),
        "{matches:?}"
    );

    let engine_nonoverlap = FuzzyAhoCorasickBuilder::new()
        .fuzzy(FuzzyLimits::default().edits(2))
        .non_overlapping(true)
        .build(patterns);

    let matches_nonoverlap = engine_nonoverlap.search("saddamhussein", 0.5);
    assert_eq!(matches_nonoverlap.len(), 1, "{matches_nonoverlap:?}");

    let matches_nonoverlap_two = engine_nonoverlap.search("sadamddamhu", 0.5);
    assert_eq!(
        matches_nonoverlap_two.len(),
        2,
        "{matches_nonoverlap_two:?}"
    );
    assert!(
        matches_nonoverlap_two
            .iter()
            .any(|m| m.pattern == "saddam" && m.text == "sadam"),
        "{matches_nonoverlap_two:?}"
    );
    assert!(
        matches_nonoverlap_two
            .iter()
            .any(|m| m.pattern == "ddamhu" && m.text == "ddamhu"),
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
            .any(|m| m.pattern == "hussein" && m.text == "huzein")
    );

    let loose = FuzzyAhoCorasickBuilder::new()
        .penalties(
            FuzzyPenalties::default()
                .substitution(0.8)
                .insertion(0.95)
                .deletion(0.95),
        )
        .build([("hussein", 1.0, 3)])
        .search("huzein", 0.2);
    assert!(
        loose
            .iter()
            .any(|m| m.pattern == "hussein" && m.text == "huzein")
    );
}

#[test]
fn test_segment_text() {
    let engine = FuzzyAhoCorasickBuilder::new()
        .non_overlapping(true)
        .fuzzy(FuzzyLimits::new().edits(2))
        .build(["saddam", "hussein"]);
    assert_eq!(engine.segment_text("saddam hussein", 0.8), "saddam hussein");
    assert_eq!(engine.segment_text("sadamhusein", 0.8), "sadam husein");
    assert_eq!(
        engine.segment_text("sadamhuseinaltikriti", 0.8),
        "sadam husein altikriti"
    );
}

#[test]
fn test_segment_text2() {
    let engine = FuzzyAhoCorasickBuilder::new()
        .non_overlapping(true)
        .case_insensitive(true)
        .build(["HASAN", "JAMAL", "HUSSEIN", "ZEINIYE"]);
    assert_eq!(
        engine.segment_text("ZEINIYEHussEINHASaNJAMAL", 0.8),
        "ZEINIYE HussEIN HASaN JAMAL"
    );
}

#[test]
fn test_fail() {
    let engine = FuzzyAhoCorasickBuilder::new()
        .non_overlapping(true)
        .build(["saddam", "hussein"]);
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
fn test_longer_match_preference() {
    let engine = FuzzyAhoCorasickBuilder::new()
        .non_overlapping(true)
        .build(["JOINT STOCK COMPANY", "STOCK"]);
    let result = engine.search("JOINT STOCK COMPANY GAZPROM", 0.8);
    assert!(result.iter().any(|m| m.pattern == "JOINT STOCK COMPANY"));
    assert!(!result.iter().any(|m| m.pattern == "STOCK"));
}
