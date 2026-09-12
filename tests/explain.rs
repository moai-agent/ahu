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
    assert!(!text.contains(ahu::harness::RELIABILITY_WARNING));
    assert!(text.contains("normal harness capabilities"));
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
    for text in [explain::overview(), explain::markdown()] {
        for harness in ahu::catalog::HARNESSES {
            assert!(text.contains(harness.id), "{} missing", harness.id);
        }
        // Each harness shows the version its adapter was verified against.
        for version in ["2.1.269", "0.154.0", "1.2.2"] {
            assert!(text.contains(version), "{version} missing from {text}");
        }
    }
}

#[test]
fn the_overview_says_ahu_is_not_a_harness_and_names_what_it_needs() {
    for text in [explain::overview(), explain::markdown()] {
        assert!(text.contains("ahu is not an agent harness"), "{text}");
        assert!(
            text.contains("cross-harness configuration management"),
            "{text}"
        );
        // The hard dependencies have to be stated, not implied.
        assert!(text.contains("cmux"), "cmux is required");
        assert!(
            text.contains("A supported harness"),
            "the harness dependency must be named: {text}"
        );
        assert!(
            text.contains("installed and signed in by you"),
            "who installs it must be explicit: {text}"
        );
        assert!(
            text.contains("holds no API key"),
            "ahu must state that it holds no credentials: {text}"
        );
        assert!(
            text.contains("never installs, configures, or authenticates one"),
            "{text}"
        );
        assert!(text.contains("holds no credentials"), "{text}");
        assert!(
            text.contains("install, configure, or authenticate a harness"),
            "the will-not list must cover this"
        );
    }
}

#[test]
fn the_markdown_render_is_a_real_markdown_document() {
    let text = explain::markdown();
    assert!(text.starts_with("# ahu "), "{}", &text[..40]);
    assert_eq!(
        mermaid_blocks(&text).len(),
        4,
        "diagrams survive the render"
    );
    // Headings, not underlined terminal titles.
    assert!(text.contains("\n## Launch pipeline\n"), "{text}");
    assert!(
        !text.contains("\n-----"),
        "no terminal underlines leaked in"
    );
    // Warnings become blockquotes rather than `!!` gutters.
    assert!(
        text.contains(&format!("> **{}**", ahu::hooks::NON_PROJECT_HOOK_WARNING)),
        "{text}"
    );
    assert!(
        !text.contains("  !! "),
        "terminal warning gutters leaked in"
    );
    // The catalog becomes a table.
    assert!(
        text.contains("| harness | ahu adapter | verified against |"),
        "{text}"
    );
    assert!(text.contains("| --- | --- | --- |"), "{text}");
}

#[test]
fn the_terminal_render_carries_no_markdown_syntax() {
    let text = explain::overview();
    assert!(
        !text.contains("**"),
        "bold markers should be stripped for a terminal"
    );
    assert!(
        !text.contains("| --- |"),
        "no markdown tables in terminal output"
    );
    assert!(text.contains("  !! "), "warnings use a terminal gutter");
}

#[test]
fn both_renders_are_built_from_the_same_document() {
    // Every section title appears in both, so the two can never drift.
    let terminal = explain::overview();
    let markdown = explain::markdown();
    for section in explain::document() {
        assert!(
            terminal.contains(section.title),
            "terminal missing {}",
            section.title
        );
        assert!(
            markdown.contains(section.title),
            "markdown missing {}",
            section.title
        );
    }
}
