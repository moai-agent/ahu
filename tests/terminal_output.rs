//! What repository content is allowed to do to the terminal.
//!
//! ahu's output is a security surface: the launch preview is the only thing
//! standing between a repository's hooks and a session that runs them. A
//! repository controls file names, hook commands, agent descriptions, and
//! configuration values, so none of those may carry a sequence that moves the
//! cursor, clears the screen, or reorders what the reader sees.

mod common;

use ahu::util::{display_safe, display_safe_block};
use common::TestRepo;

/// Characters that reorder or hide text without being category `Cc`.
///
/// `char::is_control` covers `Cc` only, so bidi overrides and the zero-width
/// family used by Trojan Source pass straight through a `Cc`-only filter.
const INVISIBLE: &[char] = &[
    '\u{00ad}', // soft hyphen
    '\u{061c}', // arabic letter mark
    '\u{200b}', // zero width space
    '\u{200e}', // left-to-right mark
    '\u{200f}', // right-to-left mark
    '\u{202a}', // left-to-right embedding
    '\u{202e}', // right-to-left override
    '\u{2060}', // word joiner
    '\u{2066}', // left-to-right isolate
    '\u{2069}', // pop directional isolate
    '\u{2028}', // line separator
    '\u{2029}', // paragraph separator
    '\u{feff}', // zero width no-break space
];

#[test]
fn display_safe_neutralises_bidi_overrides_and_zero_width_characters() {
    for hostile in INVISIBLE {
        let rendered = display_safe(&format!("a{hostile}b"));
        assert!(
            !rendered.contains(*hostile),
            "U+{:04X} survived display_safe: {rendered:?}",
            *hostile as u32
        );
        assert!(
            rendered.starts_with('a') && rendered.ends_with('b'),
            "{rendered:?}"
        );
    }
}

#[test]
fn display_safe_block_neutralises_them_too_while_keeping_line_structure() {
    for hostile in INVISIBLE {
        let rendered = display_safe_block(&format!("a{hostile}b\nc\n"));
        assert!(
            !rendered.contains(*hostile),
            "U+{:04X} survived display_safe_block: {rendered:?}",
            *hostile as u32
        );
        assert_eq!(rendered.lines().count(), 2, "{rendered:?}");
    }
}

/// A `\xNN` escape cannot name a character above 0xFF unambiguously, so the
/// wide ones are rendered in the `\u{...}` form instead.
#[test]
fn escapes_name_the_character_they_replace() {
    assert_eq!(display_safe("\u{1b}[2J"), "\\x1b[2J");
    assert_eq!(display_safe("\u{202e}"), "\\u{202e}");
    assert_eq!(display_safe("\u{feff}"), "\\u{feff}");
}

/// Ordinary text, including non-ASCII text, must be left exactly alone.
#[test]
fn display_safe_leaves_legitimate_text_untouched() {
    for safe in ["plain", "héllo wörld", "日本語", "a-b_c.d/e", "emoji 🌱"] {
        assert_eq!(display_safe(safe), safe);
        assert_eq!(display_safe_block(safe), safe);
    }
}

/// The launch summary prints notes that carry repository-relative
/// configuration paths verbatim, so it has to escape them like every other
/// renderer does.
#[test]
fn launch_notes_are_escaped_before_they_reach_the_terminal() {
    let rendered = ahu::commands::render_launch_notes(&[
        "agent configuration changed while the task was being prepared: \
         .claude/x\u{1b}[2J\u{1b}[1;31mSAFE.json"
            .to_string(),
    ]);
    assert!(!rendered.contains('\u{1b}'), "{rendered:?}");
    assert!(rendered.contains("\\x1b[2J"), "{rendered:?}");
}

/// The error path is a renderer too. A repository controls the names of the
/// files ahu reads, and those names are embedded in error messages.
#[test]
fn a_hostile_file_name_in_an_error_cannot_repaint_the_terminal() {
    let repo = TestRepo::new();
    // An unparseable manifest whose *name* carries a screen-clearing sequence.
    // `ahu agents` reads every `.toml` here, so this is reached with no race
    // and no user action beyond running ahu in the checkout.
    repo.write(
        ".agents/ahu/agents/x\u{1b}[2J\u{1b}[1;31mSAFE.toml",
        "this is not valid toml\n",
    );

    let output = std::process::Command::new(env!("CARGO_BIN_EXE_ahu"))
        .arg("agents")
        .current_dir(repo.path())
        .env("AHU_STATE_DIR", repo.state_path())
        .output()
        .expect("ahu runs");

    assert!(
        !output.status.success(),
        "the bad manifest should be an error"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains('\u{1b}'),
        "a raw escape reached the terminal: {stderr:?}"
    );
    assert!(stderr.contains("\\x1b[2J"), "{stderr:?}");
}
