mod common;

use ahu::{agent, agent_ref, git};

#[test]
fn references_are_stable_across_linked_checkouts_and_reject_stale_content() {
    let fixture = common::TestRepo::new();
    fixture.add_agent("builder", "1.0.0", "claude-sonnet-5");
    fixture.commit("register builder");
    let repo = git::discover(fixture.path()).unwrap();
    let resolved = agent::find(fixture.path(), "builder").unwrap();
    let reference = agent_ref::ensure(&repo, &resolved).unwrap();
    assert!(reference.starts_with("ahu:agent:"));
    assert_eq!(agent_ref::ensure(&repo, &resolved).unwrap(), reference);
    assert_eq!(
        agent_ref::resolve(&repo, &reference)
            .unwrap()
            .identity_digest(),
        resolved.identity_digest()
    );

    let linked = fixture.state_path().join("linked-agent");
    common::git(
        fixture.path(),
        &["worktree", "add", "--detach", linked.to_str().unwrap()],
    );
    let sibling = git::discover(&linked).unwrap();
    assert_eq!(
        agent_ref::resolve(&sibling, &reference)
            .unwrap()
            .identity_digest(),
        resolved.identity_digest()
    );

    fixture.write(
        ".claude/agents/builder.md",
        "---\nname: builder\nmodel: claude-sonnet-5\ntools: Read\n---\n\nChanged instructions.\n",
    );
    let changed = agent::find(fixture.path(), "builder").unwrap();
    assert_ne!(changed.identity_digest(), resolved.identity_digest());
    assert!(agent_ref::resolve(&repo, &reference).is_err());
    assert_ne!(agent_ref::ensure(&repo, &changed).unwrap(), reference);
}

#[test]
fn parser_requires_canonical_agent_references() {
    assert!(agent_ref::parse("ahu:agent:not-a-uuid").is_err());
    assert!(agent_ref::parse("ahu:task:00000000-0000-7000-8000-000000000000").is_err());
}
