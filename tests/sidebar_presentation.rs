mod common;

use ahu::cli::{Command, parse};
use ahu::util::{sidebar_text, task_title_from_prompt};

#[test]
fn common_markdown_becomes_plain_text_without_mangling_identifiers() {
    assert_eq!(
        sidebar_text(
            "## **Fix parser**\n\n- [x] Keep `task_id` and [labels](https://example.invalid/a(b)).\n> _Review_ ~~old~~ text",
            160
        ),
        "Fix parser Keep task_id and labels. Review old text"
    );
    for plain in [
        "task_id foo_bar_baz",
        "C++ a * b * c",
        "--color=always",
        "日本語 🦀",
        "x < y && y > z",
    ] {
        assert_eq!(sidebar_text(plain, 160), plain);
    }
    assert_eq!(
        sidebar_text("```rust\nlet task_id = 1;\n```", 160),
        "let task_id = 1;"
    );
}

#[test]
fn labels_are_bounded_and_display_safe() {
    for limit in [0, 1, 2, 60, 160] {
        let result = sidebar_text(&"界🦀e\u{301}\u{202e}\u{1b}\n".repeat(100), limit);
        assert!(result.chars().count() <= limit);
        assert!(!result.contains(['\u{202e}', '\u{1b}', '\n']));
    }
    assert_eq!(
        task_title_from_prompt("\n## **Fix parser**\nInstructions"),
        "Fix parser"
    );
    assert_eq!(task_title_from_prompt(" \n "), "untitled task");
}

#[test]
fn metadata_options_preserve_values_and_reject_duplicates() {
    let (args, color) = ahu::cli::extract_color(
        [
            "launch",
            "@sable",
            "--prompt",
            "assignment",
            "--title",
            "--color=always",
            "--summary",
            "--color=never",
        ]
        .map(str::to_string)
        .to_vec(),
    )
    .unwrap();
    assert!(color.is_none());
    let Command::Launch { display, .. } = parse(args).unwrap() else {
        panic!("launch")
    };
    assert_eq!(display.title.as_deref(), Some("--color=always"));
    assert_eq!(display.summary.as_deref(), Some("--color=never"));
    for option in ["--title", "--summary"] {
        assert!(parse(["launch", "@sable", "--prompt", "task", option]).is_err());
        assert!(
            parse([
                "launch", "@sable", "--prompt", "task", option, "first", option, "second"
            ])
            .is_err()
        );
    }
}

#[test]
fn cli_transports_metadata_to_plan_without_changing_assignment() {
    let repo = common::TestRepo::new();
    repo.init_config();
    repo.add_agent("sable", "1.0.0", "claude-sonnet-5");
    repo.commit("fixture");
    let bin = common::fake_harness(repo.state_path(), &repo.state_path().join("argv"));
    let prompt = "Operational boilerplate\nActual assignment";
    for (extra, title, summary) in [
        (
            vec![],
            "Operational boilerplate",
            "Operational boilerplate Actual assignment",
        ),
        (
            vec!["--title", "## **Fix parser**"],
            "Fix parser",
            "Fix parser",
        ),
        (
            vec![
                "--title",
                "Fix parser",
                "--summary",
                "Review [labels](https://example.invalid) and `task_id`",
            ],
            "Fix parser",
            "Review labels and task_id",
        ),
    ] {
        let output = common::ahu()
            .current_dir(repo.path())
            .args([
                "launch",
                "@sable",
                "--prompt",
                prompt,
                "--dry-run",
                "--output",
                "json",
            ])
            .args(extra)
            .env("AHU_CMUX_BIN", repo.state_path().join("missing-cmux"))
            .env(
                "PATH",
                format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()),
            )
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(json["title"], title);
        assert_eq!(json["summary"], summary);
        assert_eq!(
            json["prompt_digest"],
            ahu::util::digest_bytes(prompt.as_bytes())
        );
        assert_eq!(json["executed"], false);
    }
}
