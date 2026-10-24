//! Independent example package exercised only through the public CLI.
//! All data, startup markers and ordinary Git repositories are temporary/offline.
mod common;

use serde_json::{Value, json};
use std::{fs, os::unix::fs::PermissionsExt, path::Path, process::Output};

const ID: &str = "text-stats";
const BINDING: &str = ".agents/ahu/plugins/text-stats.md";

fn run(root: &Path, args: &[&str]) -> Output {
    common::ahu()
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("OTEL_SDK_DISABLED", "true")
        .arg("--repo")
        .arg(root)
        .args(args)
        .output()
        .unwrap()
}

fn success(output: Output) -> String {
    assert!(output.status.success(), "{output:?}");
    assert!(output.stderr.is_empty(), "{output:?}");
    String::from_utf8(output.stdout).unwrap()
}

fn commit_binding(root: &Path) {
    common::git(root, &["add", "--", BINDING]);
    common::git(
        root,
        &[
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-qm",
            "Synthetic example policy",
        ],
    );
}

fn invoke(root: &Path, input: Value) -> Output {
    fs::write(root.join("input.json"), input.to_string()).unwrap();
    run(
        root,
        &["plugins", "invoke", ID, "--input-file", "input.json"],
    )
}

fn inventory(root: &Path) -> Value {
    serde_json::from_str(&success(run(
        root,
        &["plugins", "installed", "--output", "json"],
    )))
    .unwrap()
}

#[test]
fn independent_example_installs_invokes_disables_and_removes_offline() {
    let temp = tempfile::tempdir().unwrap();
    let base = temp.path().canonicalize().unwrap();
    let root = base.join("project");
    let package = base.join("package");
    let marker = base.join("started");
    fs::create_dir_all(root.join(".agents/ahu/plugins")).unwrap();
    fs::create_dir(&package).unwrap();
    common::git(&root, &["init", "-q"]);

    let example = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/plugins/text-stats");
    for entry in fs::read_dir(example).unwrap() {
        let entry = entry.unwrap();
        fs::copy(entry.path(), package.join(entry.file_name())).unwrap();
    }
    let entrypoint = package.join("text-stats.py");
    assert_ne!(
        fs::metadata(&entrypoint).unwrap().permissions().mode() & 0o111,
        0
    );
    // Observe every startup, including a hypothetical start that immediately
    // fails. Instrument only the disposable copy, before calculating its pin.
    let script = fs::read_to_string(&entrypoint).unwrap();
    let (shebang, body) = script.split_once('\n').unwrap();
    fs::write(
        &entrypoint,
        format!(
            "{shebang}\nwith open({}, 'a') as marker:\n    marker.write('started\\n')\n{body}",
            json!(marker)
        ),
    )
    .unwrap();
    let digest = success(run(
        &root,
        &["plugins", "digest", package.to_str().unwrap()],
    ));
    let digest = digest.trim();
    fs::write(
        root.join(BINDING),
        format!(
            "---\nokf_version: '0.2'\ntype: ahu:plugin-binding\nahu:\n  schema_version: 1\n  id: text-stats\n  version: '1.0.0'\n  source:\n    kind: local\n    repository: {}\n    revision: '{digest}'\n    sha256: '{digest}'\n  compatibility:\n    ahu: '{}'\n    host_api: 1\n  enabled: false\n  grants: [execute]\n---\nSynthetic offline example.\n",
            json!(package),
            env!("CARGO_PKG_VERSION")
        ),
    )
    .unwrap();
    commit_binding(&root);

    success(run(&root, &["plugins", "install", ID]));
    let installed = inventory(&root);
    let rows = installed["installations"].as_array().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["id"], ID);
    assert_eq!(rows[0]["source_verified"], true);
    assert_eq!(rows[0]["enabled"], false);
    success(run(&root, &["plugins", "enable", ID]));
    commit_binding(&root);
    assert!(
        !marker.exists(),
        "installation/activation executed the package"
    );

    // An installed package must remain usable without its source directory.
    fs::remove_dir_all(&package).unwrap();
    for (text, characters, words) in [
        ("hello offline world", 19, 3),
        ("", 0, 0),
        ("café\n🦀", 6, 2),
    ] {
        let result: Value =
            serde_json::from_str(&success(invoke(&root, json!({"text": text})))).unwrap();
        assert_eq!(
            result,
            json!({"schema_version":1,"status":"ok","result":{"characters":characters,"words":words}})
        );
    }
    let invalid = invoke(&root, json!({"text": 42}));
    assert_eq!(invalid.status.code(), Some(5));
    assert!(invalid.stdout.is_empty());
    assert_eq!(
        serde_json::from_slice::<Value>(&invalid.stderr).unwrap(),
        json!({"schema_version":1,"status":"error","code":"failed","outcome_may_be_unknown":true})
    );
    assert_eq!(fs::read_to_string(&marker).unwrap(), "started\n".repeat(4));
    fs::remove_file(&marker).unwrap();

    success(run(&root, &["plugins", "disable", ID]));
    assert!(!marker.exists(), "disable executed the package");
    commit_binding(&root);
    let disabled_binding = fs::read(root.join(BINDING)).unwrap();
    assert_eq!(inventory(&root)["installations"][0]["enabled"], false);
    let denied = invoke(&root, json!({"text": "disabled"}));
    assert_eq!(denied.status.code(), Some(4));
    assert!(denied.stdout.is_empty());
    assert_eq!(
        serde_json::from_slice::<Value>(&denied.stderr).unwrap(),
        json!({"schema_version":1,"status":"error","code":"denied","outcome_may_be_unknown":false})
    );
    assert!(!marker.exists(), "disabled invocation executed the package");

    success(run(&root, &["plugins", "remove", ID]));
    assert_eq!(inventory(&root)["installations"], json!([]));
    assert_eq!(fs::read(root.join(BINDING)).unwrap(), disabled_binding);
    assert!(!marker.exists(), "inventory/removal executed the package");
}
