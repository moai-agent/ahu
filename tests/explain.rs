//! `ahu explain` — the built-in architecture overview.
//!
//! The Mermaid blocks are a deliverable: people redirect them into Markdown and
//! expect them to render. These tests guard their structure. Full parsing is
//! done out of band with the real Mermaid parser; see the README.

use ahu::explain;

fn mermaid_blocks(text: &str) -> Vec<&str> {
    let mut blocks = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find("```mermaid\n") {
        let after = &rest[start + "```mermaid\n".len()..];
        let end = after.find("```").expect("every mermaid fence is closed");
        blocks.push(&after[..end]);
        rest = &after[end + 3..];
    }
    blocks
}

#[test]
fn the_mermaid_output_is_four_well_formed_flowcharts() {
    let text = explain::mermaid_only();
    assert_eq!(
        text.matches("```").count() % 2,
        0,
        "fences must be balanced"
    );
    let blocks = mermaid_blocks(&text);
    assert_eq!(blocks.len(), 4);
    for block in &blocks {
        let first = block.lines().next().unwrap_or_default().trim();
        assert!(
            first.starts_with("flowchart "),
            "block should declare a diagram type, got {first:?}"
        );
        assert_eq!(
            block.matches('[').count() + block.matches('{').count(),
            block.matches(']').count() + block.matches('}').count(),
            "unbalanced node brackets in:\n{block}"
        );
        assert_eq!(block.matches('"').count() % 2, 0, "unbalanced quotes");
        assert!(
            !block.contains("-->|\"\"|"),
            "empty edge labels render badly"
        );
    }
}

#[test]
fn the_diagrams_avoid_raw_angle_brackets_inside_labels() {
    // Mermaid treats a bare `<` in a label as the start of markup, so ahu's
    // placeholders have to be escaped or they break rendering.
    for block in mermaid_blocks(&explain::mermaid_only()) {
        for line in block.lines() {
            let Some(label_start) = line.find('"') else {
                continue;
            };
            let label = &line[label_start..];
            for forbidden in ["<agent>", "<task-id>", "<id>", "<name>", "<prompt>"] {
                assert!(
                    !label.contains(forbidden),
                    "use HTML entities instead of {forbidden} in: {line}"
                );
            }
        }
    }
}

#[test]
fn the_overview_embeds_the_diagrams_and_the_standing_warnings() {
    let text = explain::overview();
    assert_eq!(
        mermaid_blocks(&text).len(),
        4,
        "all four diagrams are shown"
    );
    assert!(text.contains(ahu::harness::RELIABILITY_WARNING));
    assert!(text.contains(ahu::hooks::NON_PROJECT_HOOK_WARNING));
    assert!(text.contains(ahu::catalog::CATALOG_VERSION));
    assert!(text.contains(env!("CARGO_PKG_VERSION")));
}

#[test]
fn the_overview_states_the_load_bearing_invariants() {
    let text = explain::overview();
    for claim in [
        "substitute a different harness or model",
        "stage, commit, push, stash, reset, clean, or switch branches",
        "add, edit, or remove hooks",
        "one argument vector element",
        "never labelled",
        "pending behaviour change",
    ] {
        assert!(text.contains(claim), "overview should state: {claim}");
    }
}

#[test]
fn every_harness_in_the_catalog_is_listed_with_its_status() {
    let text = explain::overview();
    for harness in ahu::catalog::HARNESSES {
        assert!(text.contains(harness.id), "{} missing", harness.id);
    }
    assert!(text.contains("verified against 2.1.269"));
    assert!(text.contains("future work"));
}
