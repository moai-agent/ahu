mod common;
use common::TestRepo;

#[test]
fn native_mcp_changes_require_private_acceptance_without_shared_lock_churn() {
    let repo = TestRepo::new();
    install_private_runtime(&repo);
    repo.write(".gitignore", ".invented/local.json\n");
    repo.write(".invented/local.json", "private project context");
    let home = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let native = home.path().join(".invented/native-config.json");
    std::fs::create_dir_all(native.parent().unwrap()).unwrap();
    std::fs::write(
        &native,
        br#"{"mcpServers":{"ahu":{"command":"SYNTHETIC-PRIVATE-FIRST"}}}"#,
    )
    .unwrap();
    let run = |args: &[&str]| {
        common::ahu()
            .current_dir(repo.path())
            .env("HOME", home.path())
            .env("XDG_STATE_HOME", state.path())
            .args(args)
            .output()
            .unwrap()
    };
    let update = run(&["lock", "--update"]);
    assert!(
        update.status.success(),
        "{}",
        String::from_utf8_lossy(&update.stderr)
    );
    common::git(repo.path(), &["add", ".gitignore", ".agents"]);
    common::git(repo.path(), &["commit", "-qm", "lock fixture context"]);
    let shared = std::fs::read(repo.path().join(".agents/ahu/lock.md")).unwrap();
    assert!(!String::from_utf8_lossy(&shared).contains("SYNTHETIC-PRIVATE"));
    assert!(!String::from_utf8_lossy(&shared).contains("native:invented"));
    assert!(!String::from_utf8_lossy(&shared).contains(".invented/local.json"));
    assert!(run(&["lock"]).status.success());
    std::fs::write(
        &native,
        br#"{"mcpServers":{"ahu":{"command":"SYNTHETIC-PRIVATE-SECOND"}}}"#,
    )
    .unwrap();
    let changed = run(&["lock"]);
    assert!(!changed.status.success());
    assert!(!String::from_utf8_lossy(&changed.stdout).contains("SYNTHETIC-PRIVATE"));
    assert!(run(&["lock", "--update"]).status.success());
    assert!(run(&["lock"]).status.success());
    assert_eq!(
        std::fs::read(repo.path().join(".agents/ahu/lock.md")).unwrap(),
        shared
    );
    std::fs::remove_file(&native).unwrap();
    assert!(!run(&["lock"]).status.success());
    assert!(run(&["lock", "--update"]).status.success());
    assert!(run(&["lock"]).status.success());
    assert_eq!(
        std::fs::read(repo.path().join(".agents/ahu/lock.md")).unwrap(),
        shared
    );
}

// Independent protocol peer: no native binary or provider implementation link.
fn install_private_runtime(repo: &TestRepo) {
    use std::os::unix::fs::PermissionsExt;
    let package = tempfile::tempdir().unwrap();
    let root = package.path().canonicalize().unwrap();
    let descriptor = serde_json::json!({"id":"invented","display_name":"Invented","program":"missing-native-fixture-executable","modes":["interactive"],"operations":["native_sources"],"context_paths":[".invented/"],"private_context_paths":[".invented/local.json"]});
    let declaration = serde_json::json!({"protocol_version":1,"path":"peer","timeout_ms":3000,"descriptor":descriptor});
    std::fs::write(
        root.join("Cargo.toml"),
        "[package]\nname='private-fixture'\nversion='1.0.0'\nedition='2024'\n",
    )
    .unwrap();
    std::fs::write(root.join("ahu-plugin.md"),format!("---\nokf_version: '0.2'\ntype: ahu:plugin\nschema_version: 1\nid: private-fixture\npackage: private-fixture\nhost_api: 1\ncargo_manifest: Cargo.toml\nharness_runtime: {declaration}\n---\nSynthetic fixture.\n")).unwrap();
    std::fs::write(
        root.join("peer"),
        r#"#!/usr/bin/python3
import sys,json,pathlib,hashlib
for line in sys.stdin:
 r=json.loads(line); p=r['params']
 assert r['operation']=='native_sources' and p['settings']['view']=='private_fingerprints'
 path=pathlib.Path(p['home'])/'.invented/native-config.json'
 pins={'native:invented:0':hashlib.sha256(path.read_bytes()).hexdigest()} if path.exists() else {}
 print(json.dumps({'protocol_version':1,'id':r['id'],'result':{'fingerprints':pins}}),flush=True)
"#,
    )
    .unwrap();
    std::fs::set_permissions(root.join("peer"), std::fs::Permissions::from_mode(0o755)).unwrap();
    let digest = ahu::plugin_install::digest(&root).unwrap();
    let binding = ".agents/ahu/plugins/private-fixture.md";
    repo.write(binding,&format!("---\nokf_version: '0.2'\ntype: ahu:plugin-binding\nahu:\n  schema_version: 1\n  id: private-fixture\n  version: '1.0.0'\n  source:\n    kind: local\n    repository: {}\n    revision: '{digest}'\n    sha256: '{digest}'\n  compatibility:\n    ahu: '{}'\n    host_api: 1\n  enabled: true\n  grants: [harness-runtime]\n---\nFixture.\n",serde_json::json!(root),env!("CARGO_PKG_VERSION")));
    common::git(repo.path(), &["add", "-f", binding]);
    common::git(
        repo.path(),
        &["commit", "-qm", "install private fixture", "--", binding],
    );
    ahu::plugin_install::run(repo.path(), "install", "private-fixture").unwrap();
}

#[test]
fn first_working_activation_never_publishes_private_context() {
    let repo = TestRepo::new();
    install_private_runtime(&repo);
    let binding = ".agents/ahu/plugins/private-fixture.md";
    common::git(repo.path(), &["rm", "--cached", binding]);
    common::git(
        repo.path(),
        &["commit", "-qm", "leave activation uncommitted"],
    );
    repo.write(".invented/local.json", "PRIVATE-FIRST-ACTIVATION");
    let home = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let output = common::ahu()
        .current_dir(repo.path())
        .env("HOME", home.path())
        .env("XDG_STATE_HOME", state.path())
        .args(["lock", "--update"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let lock = repo.read(".agents/ahu/lock.md");
    assert!(!lock.contains(".invented/local.json"));
    assert!(!lock.contains(&ahu::util::digest_bytes(b"PRIVATE-FIRST-ACTIVATION")));
}
