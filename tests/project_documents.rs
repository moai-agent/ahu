mod common;
use ahu::{config, context_lock, okf, snapshot};
use common::TestRepo;

#[test]
fn new_policy_is_okf_strict_settings_and_inert_nested_metadata() {
    let repo = TestRepo::new();
    repo.init_config();
    let legacy = config::load(repo.path()).unwrap().unwrap();
    repo.remove(config::LEGACY_CONFIG_PATH);
    config::write_new(repo.path(), &legacy.config).unwrap();
    assert!(!repo.path().join(config::LEGACY_CONFIG_PATH).exists());
    let mut doc =
        okf::Document::parse(&repo.read(config::CONFIG_RELATIVE_PATH), "ahu:project").unwrap();
    doc.fields.insert(
        "execution".into(),
        serde_json::json!({"native_helpers": "bounded", "nested": ["inert"]}),
    );
    doc.body = "\nEnable nothing through prose.\n".into();
    repo.write(config::CONFIG_RELATIVE_PATH, &doc.render().unwrap());
    assert_eq!(
        config::load(repo.path()).unwrap().unwrap().config,
        legacy.config
    );
    doc.fields.get_mut("ahu").unwrap()["execution"] =
        serde_json::json!({"native_helpers": "bounded"});
    repo.write(config::CONFIG_RELATIVE_PATH, &doc.render().unwrap());
    assert!(config::load(repo.path()).is_err());
}

#[test]
fn lock_refresh_retains_unknown_metadata_and_detects_descriptive_drift() {
    let repo = TestRepo::new();
    repo.init_config();
    let settings = config::load(repo.path()).unwrap().unwrap().config;
    repo.remove(config::LEGACY_CONFIG_PATH);
    config::write_new(repo.path(), &settings).unwrap();
    repo.commit("OKF fixture");
    let git = ahu::git::discover(repo.path()).unwrap();
    let mut doc = okf::Document::parse(&repo.read(context_lock::LOCK_PATH), "ahu:lock").unwrap();
    doc.fields.insert(
        "notes".into(),
        serde_json::json!({"nested": [1, {"tag": "kept"}]}),
    );
    doc.body = "\nKeep this body.\n".into();
    repo.write(context_lock::LOCK_PATH, &doc.render().unwrap());
    let collected = snapshot::collect(repo.path()).unwrap();
    context_lock::refresh(&git, &collected).unwrap();
    assert_eq!(
        okf::Document::parse(&repo.read(context_lock::LOCK_PATH), "ahu:lock").unwrap(),
        doc
    );
    repo.commit("annotated lock");
    let mut policy = repo.read(config::CONFIG_RELATIVE_PATH);
    policy.push_str("\nDescriptive change.\n");
    repo.write(config::CONFIG_RELATIVE_PATH, &policy);
    assert!(
        !context_lock::check(&git, &snapshot::collect(repo.path()).unwrap())
            .unwrap()
            .current
    );
}

#[test]
fn canonical_policy_inherits_while_legacy_bytes_and_records_remain_readable() {
    let source = TestRepo::new();
    source.init_config();
    let old = config::load(source.path()).unwrap().unwrap();
    let child = TestRepo::new();
    child.write(
        config::LEGACY_CONFIG_PATH,
        &source.read(config::LEGACY_CONFIG_PATH),
    );
    let old_snapshot = snapshot::collect(child.path()).unwrap();
    let settings = old.config.clone();
    source.remove(config::LEGACY_CONFIG_PATH);
    config::write_new(source.path(), &settings).unwrap();
    // Reading legacy policy neither rewrites it nor changes the recorded digest.
    let loaded = config::load(child.path()).unwrap().unwrap();
    assert_eq!(old.digest, loaded.digest);
    assert_eq!(old_snapshot, snapshot::collect(child.path()).unwrap());
    // Materialization into a synthetic directory applies parent config deletion
    // and replacement; no live task worktree is touched.
    let snapshot = snapshot::collect(source.path()).unwrap();
    ahu::snapshot::materialize(source.path(), &snapshot, child.path()).unwrap();
    assert!(!child.path().join(config::LEGACY_CONFIG_PATH).exists());
    assert_eq!(
        config::load(child.path()).unwrap().unwrap().config,
        settings
    );
    assert_eq!(
        source.read(config::CONFIG_RELATIVE_PATH),
        child.read(config::CONFIG_RELATIVE_PATH)
    );
}

#[test]
fn dual_formats_and_parent_symlinks_are_refused_without_writes() {
    let repo = TestRepo::new();
    repo.init_config();
    let old = config::load(repo.path()).unwrap().unwrap();
    repo.write(config::CONFIG_RELATIVE_PATH, &config::render(&old.config));
    assert!(config::load(repo.path()).is_err());
    assert!(config::write_new(repo.path(), &old.config).is_err());
    let other = tempfile::tempdir().unwrap();
    let root = tempfile::tempdir().unwrap();
    std::os::unix::fs::symlink(other.path(), root.path().join(".agents")).unwrap();
    assert!(config::load(root.path()).is_err());
    assert!(config::write_new(root.path(), &old.config).is_err());
    assert_eq!(std::fs::read_dir(other.path()).unwrap().count(), 0);
}

#[test]
fn malformed_versions_nested_lock_fields_and_oversized_policy_fail_closed() {
    let repo = TestRepo::new();
    repo.init_config();
    let settings = config::load(repo.path()).unwrap().unwrap().config;
    repo.remove(config::LEGACY_CONFIG_PATH);
    config::write_new(repo.path(), &settings).unwrap();
    let policy = repo.read(config::CONFIG_RELATIVE_PATH);
    for text in [
        policy.replace("schema_version: 1", "schema_version: 999"),
        policy.replace("okf_version: '0.2'", "okf_version: '999'"),
        format!("{policy}{}", "x".repeat(okf::MAX_BYTES as usize)),
    ] {
        repo.write(config::CONFIG_RELATIVE_PATH, &text);
        assert!(config::load(repo.path()).is_err());
    }
    repo.write(config::CONFIG_RELATIVE_PATH, &policy);
    let git = ahu::git::discover(repo.path()).unwrap();
    let snapshot = snapshot::collect(repo.path()).unwrap();
    context_lock::refresh(&git, &snapshot).unwrap();
    let mut doc = okf::Document::parse(&repo.read(context_lock::LOCK_PATH), "ahu:lock").unwrap();
    doc.fields.get_mut("ahu").unwrap()["entries"][0]["grant"] = true.into();
    repo.write(context_lock::LOCK_PATH, &doc.render().unwrap());
    assert!(context_lock::check(&git, &snapshot).is_err());
    assert!(context_lock::refresh(&git, &snapshot).is_err());
    repo.write(context_lock::LEGACY_LOCK_PATH, "schema_version = 1\n");
    assert!(context_lock::check(&git, &snapshot).is_err());
    assert!(context_lock::refresh(&git, &snapshot).is_err());
}

#[test]
fn migration_cli_preview_apply_and_legacy_refresh_boundary() {
    let repo = TestRepo::new();
    repo.init_config();
    // A legacy project without a lock can explicitly create its first OKF lock.
    repo.remove(context_lock::LOCK_PATH);
    let before = common::git(repo.path(), &["status", "--porcelain"]);
    let command = |args: &[&str]| {
        common::ahu()
            .current_dir(repo.path())
            .args(args)
            .output()
            .unwrap()
    };
    let preview = command(&["migrate", "--preview"]);
    assert!(
        preview.status.success(),
        "{}",
        String::from_utf8_lossy(&preview.stderr)
    );
    let text = String::from_utf8(preview.stdout).unwrap();
    assert!(text.contains("Effective typed policy parity: identical"));
    assert!(text.contains(config::CONFIG_RELATIVE_PATH));
    assert_eq!(before, common::git(repo.path(), &["status", "--porcelain"]));
    assert!(!repo.path().join(".ahu").exists());
    let applied = command(&["migrate", "--apply"]);
    assert!(
        applied.status.success(),
        "{}",
        String::from_utf8_lossy(&applied.stderr)
    );
    assert!(String::from_utf8_lossy(&applied.stdout).contains(&text));
    assert!(!repo.path().join(config::LEGACY_CONFIG_PATH).exists());
    assert!(common::git(repo.path(), &["diff", "--cached", "--name-only"]).is_empty());
    assert!(config::load(repo.path()).unwrap().is_some());
    assert!(!command(&["migrate", "--rollback"]).status.success());
    assert!(!command(&["migrate", "--unknown"]).status.success());
    // An old root lock is never rewritten by ordinary refresh.
    repo.write(context_lock::LEGACY_LOCK_PATH, "schema_version = 1\n");
    let old = repo.read(context_lock::LEGACY_LOCK_PATH);
    assert!(!command(&["lock", "--update"]).status.success());
    assert_eq!(old, repo.read(context_lock::LEGACY_LOCK_PATH));
}

#[test]
fn new_policy_never_writes_a_document_its_reader_will_refuse() {
    let repo = TestRepo::new();
    repo.init_config();
    let mut settings = config::load(repo.path()).unwrap().unwrap().config;
    repo.remove(config::LEGACY_CONFIG_PATH);
    settings.knowledge.bundles = vec!["a".repeat(1024 * 1024)];
    assert!(config::write_new(repo.path(), &settings).is_err());
    assert!(!repo.path().join(config::CONFIG_RELATIVE_PATH).exists());
}
