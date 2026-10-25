//! Independent development-only installed runtime for core lifecycle tests.
//! It deliberately does not use provider implementation crates or wire serializers.
use super::core_api as ahu;
use std::path::Path;

pub fn install(
    root: &Path,
    harness: &str,
    program: &str,
    source_dir: &str,
    format: &str,
    models: &[&str],
) {
    use std::os::unix::fs::PermissionsExt;
    let id = format!("fixture-{harness}");
    if ahu::plugin_binding::load(root, &id).unwrap().is_some() {
        return;
    }
    let package = tempfile::tempdir().unwrap();
    let package_root = package.path().canonicalize().unwrap();
    let descriptor = serde_json::json!({
        "id":harness,"display_name":format!("Synthetic {harness}"),"program":program,
        "aliases":[],"models":models,"modes":["interactive","direct","headless"],
        "operations":["probe","models","launch","admission","setup","native_sources","auth","budget","trust_prepare","trust_cleanup","decode_start","decode_event","decode_finish","recover"],
        "context_paths":[format!("{source_dir}/"),".fixture/"],"native_source_paths":[source_dir],
        "skill_paths":[".fixture/skills"],"environment_keys":["FIXTURE_CONFIG_DIR"],"tested_versions":["1.2.3"]
    });
    let declaration = serde_json::json!({"protocol_version":1,"path":"peer","timeout_ms":3000,"descriptor":descriptor});
    std::fs::write(
        package_root.join("Cargo.toml"),
        "[package]\nname='test-runtime'\nversion='1.0.0'\nedition='2024'\n",
    )
    .unwrap();
    std::fs::write(package_root.join("ahu-plugin.md"), format!("---\nokf_version: '0.2'\ntype: ahu:plugin\nschema_version: 1\nid: {id}\npackage: test-runtime\nhost_api: 1\ncargo_manifest: Cargo.toml\nharness_runtime: {declaration}\n---\nTest runtime.\n")).unwrap();
    let fixture =
        serde_json::json!({"descriptor":descriptor,"source_dir":source_dir,"format":format});
    std::fs::write(
        package_root.join("peer"),
        format!(
            "#!/usr/bin/python3\nimport json\nFIXTURE = json.loads({})\n{}",
            serde_json::to_string(&fixture.to_string()).unwrap(),
            include_str!("runtime.py")
        ),
    )
    .unwrap();
    std::fs::set_permissions(
        package_root.join("peer"),
        std::fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    let digest = ahu::plugin_install::digest(&package_root).unwrap();
    let binding = format!(".agents/ahu/plugins/{id}.md");
    std::fs::create_dir_all(root.join(".agents/ahu/plugins")).unwrap();
    std::fs::write(root.join(&binding), format!("---\nokf_version: '0.2'\ntype: ahu:plugin-binding\nahu:\n  schema_version: 1\n  id: {id}\n  version: '1.0.0'\n  source:\n    kind: local\n    repository: {}\n    revision: '{digest}'\n    sha256: '{digest}'\n  compatibility:\n    ahu: '{}'\n    host_api: 1\n  enabled: true\n  grants: [harness-runtime]\n---\nTest policy.\n", serde_json::json!(package_root), env!("CARGO_PKG_VERSION"))).unwrap();
    super::git(root, &["add", "-f", &binding]);
    super::git(
        root,
        &[
            "commit",
            "-q",
            "--only",
            "-m",
            "install synthetic runtime",
            "--",
            &binding,
        ],
    );
    ahu::plugin_install::run(root, "install", &id).unwrap();
}
