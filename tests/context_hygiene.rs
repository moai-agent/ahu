//! What the hygiene review is allowed to say, and where the advice lives.
//!
//! The binary reports facts about the sources it found; the recommendations and
//! the interpretation of those facts are the bundled `context-hygiene` skill's.
//! These tests hold that split in place: the scanner must still report the same
//! sources it always did, the review must name a control rather than prescribe
//! an action, and the skill must actually be delivered into a repository.

mod common;

use ahu::hooks::{self, Locations};
use ahu::hygiene::{self, Control};
use ahu::inventory::{self, Category, Visibility};
use ahu::{agent, config, mcp};
use common::TestRepo;
use std::path::Path;

fn locations(home: &Path) -> Locations {
    Locations {
        home: Some(home.to_path_buf()),
        managed: None,
        cmux_wrapper: false,
    }
}

/// Build the inventory and the review a `ahu hygiene @sable` would produce.
fn review_for(repo: &TestRepo) -> (inventory::Inventory, hygiene::Review) {
    let loaded = config::load(repo.path()).unwrap().unwrap();
    let found = agent::find(repo.path(), "sable").unwrap();
    let taken = ahu::snapshot::collect(repo.path()).unwrap();
    let adapter = ahu::harness::adapter_for("claude-code").unwrap();
    let enforcement = adapter
        .enforcement("claude-opus-5", Default::default())
        .unwrap();
    let home = tempfile::TempDir::new().unwrap();
    let hooks = hooks::collect_for(repo.path(), "claude-code", &locations(home.path())).unwrap();
    let built = inventory::build(&inventory::Subject {
        repo_root: repo.path(),
        loaded_config: &loaded,
        snapshot: &taken,
        agent: Some(&found),
        harness: "claude-code",
        model: "claude-opus-5",
        enforcement: &enforcement,
        hooks: &hooks,
        prompt: None,
    })
    .unwrap();
    let review = hygiene::review(
        "sable@1.0.0",
        &built,
        &enforcement,
        &loaded,
        &hygiene::ReviewState::default(),
    );
    (built, review)
}

fn fixture() -> TestRepo {
    let repo = TestRepo::new();
    repo.init_config();
    repo.add_agent("sable", "1.0.0", "claude-opus-5");
    repo.write(".claude/skills/review/SKILL.md", "a skill that may load\n");
    repo.write("CLAUDE.local.md", "local memory\n");
    repo.commit("fixture");
    repo
}

/// The review still reports every source the scanner finds, with the same
/// category, scope and sharing the inventory gives it.
#[test]
fn the_review_reports_exactly_the_scanner_facts() {
    let repo = fixture();
    let (built, review) = review_for(&repo);

    let candidates: Vec<_> = built
        .cleanup_candidates()
        .into_iter()
        .map(|item| {
            (
                item.name.clone(),
                item.category,
                item.scope.clone(),
                item.shared,
            )
        })
        .collect();
    let reported: Vec<_> = review
        .findings
        .iter()
        .map(|finding| {
            (
                finding.what.clone(),
                finding.category,
                finding.scope.clone(),
                finding.shared,
            )
        })
        .collect();
    assert_eq!(candidates, reported, "{review:#?}");

    let skill = review
        .findings
        .iter()
        .find(|finding| finding.what == ".claude/skills/review/SKILL.md")
        .expect("the repository skill is reported");
    assert_eq!(skill.category, Category::Skill);
    assert_eq!(skill.scope, "repository");
    assert_eq!(skill.control, Control::RepositoryEdit);

    let memory = review
        .findings
        .iter()
        .find(|finding| finding.what == "CLAUDE.local.md")
        .expect("the repository memory file is reported");
    assert_eq!(memory.category, Category::Memory);
    assert_eq!(memory.control, Control::RepositoryEdit);

    // The inventory still labels those same files, at the same visibility, so
    // moving the advice out did not move a fact out with it.
    for name in [".claude/skills/review/SKILL.md", "CLAUDE.local.md"] {
        let item = built
            .items
            .iter()
            .find(|item| item.name == name)
            .unwrap_or_else(|| panic!("{name} is inventoried"));
        assert_eq!(item.visibility, Visibility::Available, "{item:#?}");
        assert!(item.digest.is_some(), "{item:#?}");
    }
}

/// The rendered review names control keys and the skill, not a course of action,
/// and running it changes nothing on disk.
#[test]
fn the_rendered_review_points_at_the_skill_and_mutates_nothing() {
    let repo = fixture();
    let before = repo.read(".claude/skills/review/SKILL.md");
    let loaded = config::load(repo.path()).unwrap().unwrap();
    let (_, review) = review_for(&repo);
    let text = hygiene::render(&review, hygiene::Trigger::FirstLoad, &loaded);

    assert!(
        text.contains("Context hygiene review for sable@1.0.0"),
        "{text}"
    );
    assert!(text.contains(".claude/skills/review/SKILL.md"), "{text}");
    assert!(text.contains("[skill, repository, shared]"), "{text}");
    assert!(text.contains("control: repository-edit"), "{text}");
    assert!(text.contains("Nothing has been changed."), "{text}");
    assert!(
        text.contains("Per-agent controls ahu offers here: inventory-listing, cleanup-preview, repository-edit"),
        "{text}"
    );
    assert!(
        text.contains("does not expose: skill-disable, memory-toggle, shared-source-clear"),
        "{text}"
    );
    assert!(
        text.contains(".agents/skills/context-hygiene/SKILL.md"),
        "{text}"
    );
    // The prose that used to be compiled into the binary is the skill's now.
    assert!(
        !text.contains("ordinary Git change you review and commit yourself"),
        "{text}"
    );
    assert!(!text.contains("Start a fresh task"), "{text}");

    assert_eq!(repo.read(".claude/skills/review/SKILL.md"), before);
    assert!(repo.path().join("CLAUDE.local.md").exists());
}

/// `ahu mcp setup` delivers the skill, and the scanner then recognizes it as a
/// repository skill like any other.
#[test]
fn the_bundled_skill_is_delivered_and_recognized_as_a_repository_skill() {
    let repo = TestRepo::new();
    repo.init_config();
    repo.add_agent("sable", "1.0.0", "claude-opus-5");
    repo.commit("fixture");

    let output = common::ahu()
        .args(["mcp", "setup"])
        .current_dir(repo.path())
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");

    let path = mcp::skill_path(mcp::CONTEXT_HYGIENE_SKILL);
    assert_eq!(path, ".agents/skills/context-hygiene/SKILL.md");
    let delivered = repo.read(&path);
    assert!(delivered.contains("name: context-hygiene"), "{delivered}");
    assert!(delivered.contains("repository-edit"), "{delivered}");

    let (built, review) = review_for(&repo);
    let item = built
        .items
        .iter()
        .find(|item| item.name == path)
        .unwrap_or_else(|| panic!("the delivered skill is inventoried:\n{built:#?}"));
    assert_eq!(item.category, Category::Skill);
    assert_eq!(item.scope, "repository");
    let finding = review
        .findings
        .iter()
        .find(|finding| finding.what == path)
        .expect("the delivered skill is reviewed");
    assert_eq!(finding.control, Control::RepositoryEdit);

    // Writing it again is a no-op; a locally changed copy is refused, not
    // overwritten.
    let again = common::ahu()
        .args(["mcp", "setup"])
        .current_dir(repo.path())
        .output()
        .unwrap();
    assert!(again.status.success(), "{again:?}");
    assert_eq!(repo.read(&path), delivered);

    repo.write(&path, "local change\n");
    let refused = common::ahu()
        .args(["mcp", "setup"])
        .current_dir(repo.path())
        .output()
        .unwrap();
    assert!(!refused.status.success(), "{refused:?}");
    assert!(
        String::from_utf8_lossy(&refused.stderr).contains("refusing to overwrite changed skill"),
        "{refused:?}"
    );
    assert_eq!(repo.read(&path), "local change\n");
}
