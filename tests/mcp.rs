mod common;

use std::io::Write;
use std::process::Stdio;

#[test]
fn stdio_server_negotiates_and_lists_repository_agents_and_tasks() {
    let repo = common::TestRepo::new();
    repo.add_agent("reviewer", "1.0.0", "claude-opus-5");
    let mut child = common::ahu()
        .args(["mcp", "serve"])
        .current_dir(repo.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    for request in [
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#,
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}"#,
        r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"ahu_agents_list","arguments":{}}}"#,
        r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"ahu_tasks_list","arguments":{}}}"#,
    ] {
        writeln!(input, "{request}").unwrap();
    }
    drop(input);
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    let rows: Vec<serde_json::Value> = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(rows[0]["result"]["serverInfo"]["name"], "ahu");
    assert_eq!(rows[1]["result"]["tools"].as_array().unwrap().len(), 3);
    assert_eq!(
        rows[2]["result"]["structuredContent"]["agents"][0]["name"],
        "@reviewer"
    );
    assert_eq!(
        rows[3]["result"]["structuredContent"]["tasks"]
            .as_array()
            .unwrap()
            .len(),
        0
    );
}

#[test]
fn setup_materializes_the_bundled_skill_trees_without_overwriting_changes() {
    let repo = common::TestRepo::new();
    let output = common::ahu()
        .args(["mcp", "setup"])
        .current_dir(repo.path())
        .output()
        .unwrap();
    assert!(output.status.success());
    let agents = repo.read(".agents/skills/discover-requirements/SKILL.md");
    assert!(!repo.path().join(".claude/skills").exists());
    assert!(agents.contains("# Discover requirements"));

    repo.write(
        ".agents/skills/discover-requirements/SKILL.md",
        "local change\n",
    );
    let refused = common::ahu()
        .args(["mcp", "setup"])
        .current_dir(repo.path())
        .output()
        .unwrap();
    assert!(!refused.status.success());
    assert!(
        String::from_utf8_lossy(&refused.stderr).contains("refusing to overwrite changed skill")
    );
}
