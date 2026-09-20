//! `ahu knowledge lint`: configuration policy, bundle safety, and the exit
//! status the check reports.
//!
//! Every test here runs against a stub `okf` installed outside the repository,
//! so the assertions are about what ahu does with a validator's answers rather
//! than about any particular okf build. The two tests that need the real
//! contract instead assert on the stub's fidelity to okf 0.5, which is recorded
//! in `okf_fixture`.

mod common;

use std::path::{Path, PathBuf};
use std::process::Stdio;

use common::TestRepo;

/// One finding as okf reports it: severity, concept id, rule, message.
type Finding<'a> = (&'a str, &'a str, &'a str, &'a str);

/// Fixtures the stub okf answers from, plus the record of how it was called.
struct StubOkf {
    dir: tempfile::TempDir,
}

impl StubOkf {
    /// Install a stub `okf` on a PATH directory outside the repository.
    fn install() -> Self {
        let dir = tempfile::TempDir::new().expect("stub dir");
        let bin = dir.path().join("bin");
        std::fs::create_dir_all(&bin).expect("bin dir");
        // The bundle's own directory name selects the fixture, so one run can
        // answer differently for each configured bundle. Everything the stub
        // was asked is appended to `invocations`.
        std::fs::write(
            bin.join("okf"),
            "#!/bin/sh\n\
             sub=\"$1\"; bundle=\"$2\"\n\
             base=$(basename \"$bundle\")\n\
             printf '%s %s\\n' \"$sub\" \"$bundle\" >> \"$STUB_OKF_FIXTURES/invocations\"\n\
             out=\"$STUB_OKF_FIXTURES/$base.$sub.json\"\n\
             [ -f \"$out\" ] || out=\"$STUB_OKF_FIXTURES/$sub.json\"\n\
             [ -f \"$out\" ] && cat \"$out\"\n\
             code=\"$STUB_OKF_FIXTURES/$base.$sub.exit\"\n\
             [ -f \"$code\" ] || code=\"$STUB_OKF_FIXTURES/$sub.exit\"\n\
             [ -f \"$code\" ] && exit \"$(cat \"$code\")\"\n\
             exit 0\n",
        )
        .expect("write stub okf");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(bin.join("okf"), std::fs::Permissions::from_mode(0o755))
                .expect("chmod stub okf");
        }
        StubOkf { dir }
    }

    fn bin(&self) -> PathBuf {
        self.dir.path().join("bin")
    }

    fn fixtures(&self) -> &Path {
        self.dir.path()
    }

    /// Answer for `bundle` exactly as okf 0.5 does for these findings.
    fn answers(&self, bundle: &str, findings: &[Finding<'_>]) {
        let errors = findings.iter().filter(|f| f.0 == "ERROR").count();
        let warnings = findings.iter().filter(|f| f.0 == "WARN").count();
        // `validate` lists everything and exits 1 when there are errors.
        self.write(
            &format!("{bundle}.validate.json"),
            &okf_fixture("validate", errors, warnings, findings),
        );
        self.write(
            &format!("{bundle}.validate.exit"),
            if errors > 0 { "1" } else { "0" },
        );
        // `lint` keeps the same counts but suppresses the error findings, and
        // exits 0 regardless. That asymmetry is why ahu runs both.
        let lint: Vec<Finding<'_>> = findings
            .iter()
            .copied()
            .filter(|f| f.0 != "ERROR")
            .collect();
        self.write(
            &format!("{bundle}.lint.json"),
            &okf_fixture("lint", errors, warnings, &lint),
        );
        self.write(&format!("{bundle}.lint.exit"), "0");
    }

    /// Answer with something other than a well-formed okf report.
    fn answers_raw(&self, bundle: &str, subcommand: &str, stdout: &str, exit: &str) {
        self.write(&format!("{bundle}.{subcommand}.json"), stdout);
        self.write(&format!("{bundle}.{subcommand}.exit"), exit);
    }

    fn write(&self, name: &str, contents: &str) {
        std::fs::write(self.dir.path().join(name), contents).expect("write fixture");
    }

    fn invocations(&self) -> String {
        std::fs::read_to_string(self.dir.path().join("invocations")).unwrap_or_default()
    }
}

/// The okf 0.5 JSON report contract, as observed from okf 0.5.0.
fn okf_fixture(command: &str, errors: usize, warnings: usize, findings: &[Finding<'_>]) -> String {
    let findings: Vec<String> = findings
        .iter()
        .map(|(severity, concept_id, rule, message)| {
            format!(
                "{{\"concept_id\":{},\"message\":{},\"rule\":{},\"severity\":{}}}",
                json_string(concept_id),
                json_string(message),
                json_string(rule),
                json_string(severity)
            )
        })
        .collect();
    format!(
        "{{\"bundle\":\"/fixture\",\"command\":\"{command}\",\"errors\":{errors},\
         \"findings\":[{}],\"valid\":{},\"warnings\":{warnings}}}\n",
        findings.join(","),
        errors == 0
    )
}

/// Quote a value the way TOML quotes it, so a fixture path carrying a
/// non-ASCII character reaches `config::load` as ahu's own validation sees it
/// rather than as a TOML parse error.
fn toml_string(value: &str) -> String {
    toml::Value::String(value.to_string()).to_string()
}

fn json_string(value: &str) -> String {
    serde_json::to_string(value).expect("string encodes")
}

/// Write a bundle holding one concept document per entry.
fn bundle(repo: &TestRepo, path: &str, concepts: &[(&str, &str)]) {
    for (name, body) in concepts {
        repo.write(&format!("{path}/{name}"), body);
    }
}

/// A minimal concept document, enough for the tree scan to count it.
const CONCEPT: &str = "---\ntype: table\ntitle: Fixture\n---\n\nBody.\n";

fn configure(repo: &TestRepo, bundles: &[&str], fail_on_warnings: bool) {
    repo.init_config();
    let text = repo.read(".agents/ahu/config.toml");
    repo.write(
        ".agents/ahu/config.toml",
        &format!(
            "{text}\n[knowledge]\nbundles = [{}]\nfail_on_warnings = {fail_on_warnings}\n",
            bundles
                .iter()
                .map(|b| toml_string(b))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    );
}

struct Run {
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

impl Run {
    fn combined(&self) -> String {
        format!("{}{}", self.stdout, self.stderr)
    }
}

/// Run ahu in `cwd` with the stub okf first on PATH.
fn run(repo: &TestRepo, stub: &StubOkf, cwd: &Path, args: &[&str]) -> Run {
    let output = common::ahu()
        .current_dir(cwd)
        .args(args)
        .env("NO_COLOR", "1")
        // Nothing here may need cmux or a harness, so both are pointed at
        // paths that do not exist.
        .env("AHU_CMUX_BIN", repo.state_path().join("missing-cmux"))
        .env("STUB_OKF_FIXTURES", stub.fixtures())
        .env(
            "PATH",
            format!(
                "{}:{}",
                stub.bin().display(),
                std::env::var("PATH").unwrap()
            ),
        )
        .stdin(Stdio::null())
        .output()
        .expect("ahu runs");
    Run {
        code: output.status.code(),
        stdout: String::from_utf8_lossy(&output.stdout).to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).to_string(),
    }
}

// --- configuration ---

#[test]
fn an_omitted_knowledge_section_leaves_existing_configurations_valid() {
    let repo = TestRepo::new();
    repo.init_config();
    let loaded = ahu::config::load(repo.path()).unwrap().unwrap();
    assert!(loaded.config.knowledge.bundles.is_empty());
    assert!(!loaded.config.knowledge.fail_on_warnings);
}

#[test]
fn knowledge_accepts_only_the_fields_it_defines() {
    let repo = TestRepo::new();
    repo.init_config();
    let text = repo.read(".agents/ahu/config.toml");
    repo.write(
        ".agents/ahu/config.toml",
        &format!("{text}\n[knowledge]\nbundles = [\"docs/knowledge\"]\nokf_path = \"/tmp/okf\"\n"),
    );
    let error = ahu::config::load(repo.path()).unwrap_err().to_string();
    assert!(error.contains("okf_path"), "{error}");
}

#[test]
fn a_bundle_path_must_be_a_plain_location_inside_the_checkout() {
    let cases: &[(&str, &str)] = &[
        ("/etc", "is absolute"),
        ("../outside", "\".\" or \"..\" segment"),
        ("docs/../../outside", "\".\" or \"..\" segment"),
        ("./docs", "\".\" or \"..\" segment"),
        ("", "is empty"),
        ("docs//knowledge", "empty path segment"),
        ("docs/knowledge/", "empty path segment"),
        ("docs\\knowledge", "contains a backslash"),
        (
            "docs/kn\u{202e}owledge",
            "control or direction-changing character",
        ),
    ];
    for (bundle, expected) in cases {
        let repo = TestRepo::new();
        repo.init_config();
        let text = repo.read(".agents/ahu/config.toml");
        repo.write(
            ".agents/ahu/config.toml",
            &format!("{text}\n[knowledge]\nbundles = [{}]\n", toml_string(bundle)),
        );
        let error = ahu::config::load(repo.path()).unwrap_err().to_string();
        assert!(error.contains(expected), "{bundle:?}: {error}");
    }
}

#[test]
fn a_bundle_listed_twice_is_refused() {
    let repo = TestRepo::new();
    configure(&repo, &["docs/knowledge", "docs/knowledge"], false);
    let error = ahu::config::load(repo.path()).unwrap_err().to_string();
    assert!(error.contains("twice"), "{error}");
}

// --- prerequisites ---

#[test]
fn prerequisites_are_reported_as_prerequisites_not_as_findings() {
    let stub = StubOkf::install();

    let uninitialized = TestRepo::new();
    let missing_config = run(
        &uninitialized,
        &stub,
        uninitialized.path(),
        &["knowledge", "lint"],
    );
    assert_eq!(missing_config.code, Some(4), "{}", missing_config.stderr);
    assert!(
        missing_config.stderr.contains("ahu init"),
        "{}",
        missing_config.stderr
    );

    // A configuration with no bundles is an actionable gap, not a pass.
    let empty = TestRepo::new();
    empty.init_config();
    let no_bundles = run(&empty, &stub, empty.path(), &["knowledge", "lint"]);
    assert_eq!(no_bundles.code, Some(4), "{}", no_bundles.stderr);
    assert!(
        no_bundles
            .stderr
            .contains("no knowledge bundles are configured"),
        "{}",
        no_bundles.stderr
    );
    assert!(stub.invocations().is_empty(), "okf must not have been run");
}

#[test]
fn a_missing_okf_is_a_prerequisite_and_nothing_is_checked() {
    let repo = TestRepo::new();
    configure(&repo, &["docs/knowledge"], false);
    bundle(&repo, "docs/knowledge", &[("a.md", CONCEPT)]);

    // A PATH holding only the `git` ahu needs to discover the repository.
    let bare = tempfile::TempDir::new().unwrap();
    // Use the system installation: a caller's first Git can itself live inside
    // a package manager's Git checkout, which utility resolution now refuses.
    #[cfg(unix)]
    std::os::unix::fs::symlink("/usr/bin/git", bare.path().join("git")).unwrap();

    let output = common::ahu()
        .current_dir(repo.path())
        .args(["knowledge", "lint"])
        .env("NO_COLOR", "1")
        .env("PATH", bare.path())
        .stdin(Stdio::null())
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(4), "{stderr}");
    assert!(stderr.contains("okf was not found on PATH"), "{stderr}");
}

// --- results ---

#[test]
fn a_clean_bundle_passes_and_both_okf_subcommands_ran() {
    let repo = TestRepo::new();
    configure(&repo, &["docs/knowledge"], false);
    bundle(&repo, "docs/knowledge", &[("a.md", CONCEPT)]);
    let stub = StubOkf::install();
    stub.answers("knowledge", &[]);

    let result = run(&repo, &stub, repo.path(), &["knowledge", "lint"]);
    assert_eq!(result.code, Some(0), "{}", result.combined());
    assert!(
        result.stdout.contains("No knowledge findings."),
        "{}",
        result.stdout
    );
    let invocations = stub.invocations();
    assert!(invocations.contains("validate "), "{invocations}");
    assert!(invocations.contains("lint "), "{invocations}");
}

#[test]
fn warnings_pass_by_default_and_fail_when_the_project_says_so() {
    let findings: &[Finding<'_>] = &[
        (
            "WARN",
            "tables/a",
            "okf/frontmatter/tags-recommended",
            "frontmatter: 'tags' is recommended",
        ),
        (
            "WARN",
            "tables/a",
            "okf/body/empty",
            "body is empty - structural markdown is recommended",
        ),
    ];
    for (fail_on_warnings, expected) in [(false, 0), (true, 5)] {
        let repo = TestRepo::new();
        configure(&repo, &["docs/knowledge"], fail_on_warnings);
        bundle(&repo, "docs/knowledge", &[("a.md", CONCEPT)]);
        let stub = StubOkf::install();
        stub.answers("knowledge", findings);

        let result = run(&repo, &stub, repo.path(), &["knowledge", "lint"]);
        assert_eq!(
            result.code,
            Some(expected),
            "fail_on_warnings = {fail_on_warnings}: {}",
            result.combined()
        );
        // The finding itself is reported either way; only the verdict changes.
        assert!(
            result.combined().contains("okf/body/empty"),
            "{}",
            result.combined()
        );
        assert!(
            result.combined().contains("2 warning(s)"),
            "{}",
            result.combined()
        );
    }
}

#[test]
fn errors_fail_the_check_and_are_recovered_from_validate_alone() {
    let repo = TestRepo::new();
    // Warnings are allowed by policy, so the failure can only come from the
    // error finding -- which `okf lint` suppresses and only `validate` lists.
    configure(&repo, &["docs/knowledge"], false);
    bundle(&repo, "docs/knowledge", &[("a.md", CONCEPT)]);
    let stub = StubOkf::install();
    stub.answers(
        "knowledge",
        &[
            (
                "ERROR",
                "tables/a",
                "okf/frontmatter/type-required",
                "frontmatter: 'type' is required",
            ),
            (
                "WARN",
                "tables/a",
                "okf/frontmatter/tags-recommended",
                "frontmatter: 'tags' is recommended",
            ),
        ],
    );

    let result = run(&repo, &stub, repo.path(), &["knowledge", "lint"]);
    assert_eq!(result.code, Some(5), "{}", result.combined());
    assert!(
        result.stdout.contains("okf/frontmatter/type-required"),
        "{}",
        result.stdout
    );
    assert!(
        result.stdout.contains("1 error(s) must be fixed"),
        "{}",
        result.stdout
    );
}

#[test]
fn a_finding_reported_by_both_subcommands_is_counted_once() {
    let repo = TestRepo::new();
    configure(&repo, &["docs/knowledge"], true);
    bundle(&repo, "docs/knowledge", &[("a.md", CONCEPT)]);
    let stub = StubOkf::install();
    // The same warning appears in both answers, as real okf reports it.
    stub.answers(
        "knowledge",
        &[(
            "WARN",
            "tables/a",
            "okf/frontmatter/tags-recommended",
            "frontmatter: 'tags' is recommended",
        )],
    );

    let result = run(&repo, &stub, repo.path(), &["knowledge", "lint"]);
    assert_eq!(result.code, Some(5), "{}", result.combined());
    assert!(
        result.combined().contains("1 warning(s)"),
        "{}",
        result.combined()
    );
    assert_eq!(
        result
            .combined()
            .matches("okf/frontmatter/tags-recommended")
            .count(),
        1,
        "{}",
        result.combined()
    );
}

#[test]
fn every_configured_bundle_is_checked_and_reported_separately() {
    let repo = TestRepo::new();
    configure(&repo, &["docs/knowledge", "docs/atlas"], false);
    bundle(&repo, "docs/knowledge", &[("a.md", CONCEPT)]);
    bundle(&repo, "docs/atlas", &[("b.md", CONCEPT)]);
    let stub = StubOkf::install();
    stub.answers("knowledge", &[]);
    stub.answers(
        "atlas",
        &[(
            "ERROR",
            "tables/b",
            "okf/frontmatter/type-required",
            "frontmatter: 'type' is required",
        )],
    );

    let result = run(&repo, &stub, repo.path(), &["knowledge", "lint"]);
    assert_eq!(result.code, Some(5), "{}", result.combined());
    assert!(result.stdout.contains("2 bundle(s)"), "{}", result.stdout);
    assert!(
        result.stdout.contains("docs/knowledge  0 error(s)"),
        "{}",
        result.stdout
    );
    assert!(
        result.stdout.contains("docs/atlas  1 error(s)"),
        "{}",
        result.stdout
    );
}

// --- bundle trees ahu refuses to hand over ---

#[test]
fn an_unusable_bundle_directory_fails_without_running_okf() {
    let cases: &[(&str, &str)] = &[
        ("absent", "does not exist in this checkout"),
        ("empty", "holds no concept documents"),
        ("index-only", "holds no concept documents"),
        ("a-file", "is not a directory"),
    ];
    for (case, expected) in cases {
        let repo = TestRepo::new();
        configure(&repo, &["docs/knowledge"], false);
        match *case {
            "absent" => {}
            "empty" => std::fs::create_dir_all(repo.path().join("docs/knowledge")).unwrap(),
            // okf calls a bundle of nothing but reserved files valid, so ahu
            // has to be the one to say that checking it proved nothing.
            "index-only" => {
                bundle(
                    &repo,
                    "docs/knowledge",
                    &[("index.md", "# Index\n"), ("log.md", "# Log\n")],
                );
            }
            "a-file" => {
                repo.write("docs/knowledge", "not a bundle\n");
            }
            other => unreachable!("{other}"),
        }
        let stub = StubOkf::install();
        stub.answers("knowledge", &[]);

        let result = run(&repo, &stub, repo.path(), &["knowledge", "lint"]);
        assert_eq!(result.code, Some(5), "{case}: {}", result.combined());
        assert!(
            result.stderr.contains(expected),
            "{case}: {}",
            result.stderr
        );
        assert!(
            stub.invocations().is_empty(),
            "{case}: okf ran on an unusable bundle: {}",
            stub.invocations()
        );
    }
}

#[cfg(unix)]
#[test]
fn a_symlink_anywhere_in_a_bundle_refuses_the_check_before_okf_sees_it() {
    let outside = tempfile::TempDir::new().unwrap();
    std::fs::write(
        outside.path().join("secret.md"),
        "---\ntype: table\n---\nx\n",
    )
    .unwrap();

    // The bundle root itself, then a nested entry inside an otherwise ordinary
    // bundle. okf would follow either one and quote what it read back.
    for nested in [false, true] {
        let repo = TestRepo::new();
        configure(&repo, &["docs/knowledge"], false);
        if nested {
            bundle(&repo, "docs/knowledge", &[("a.md", CONCEPT)]);
            std::os::unix::fs::symlink(
                outside.path().join("secret.md"),
                repo.path().join("docs/knowledge/linked.md"),
            )
            .unwrap();
        } else {
            std::fs::create_dir_all(repo.path().join("docs")).unwrap();
            std::os::unix::fs::symlink(outside.path(), repo.path().join("docs/knowledge")).unwrap();
        }
        let stub = StubOkf::install();
        stub.answers("knowledge", &[]);

        let result = run(&repo, &stub, repo.path(), &["knowledge", "lint"]);
        assert_eq!(
            result.code,
            Some(5),
            "nested={nested}: {}",
            result.combined()
        );
        assert!(
            result.stderr.contains("refusing to act through a symlink"),
            "nested={nested}: {}",
            result.stderr
        );
        assert!(
            stub.invocations().is_empty(),
            "nested={nested}: okf ran on a bundle with a symlink in it"
        );
    }
}

// --- validator answers ahu will not act on ---

#[test]
fn a_validator_answer_ahu_cannot_trust_fails_the_check() {
    let cases: &[(&str, &str, &str, &str)] = &[
        // subcommand, stdout, exit, expected diagnostic
        ("validate", "", "0", "could not interpret"),
        ("validate", "not json at all\n", "0", "could not interpret"),
        ("validate", "{}\n", "0", "could not interpret"),
        (
            "validate",
            "{\"bundle\":\"/fixture\",\"command\":\"validate\",\"errors\":0,\"valid\":true,\"warnings\":0}\n",
            "0",
            "could not interpret",
        ),
        (
            "validate",
            "{\"bundle\":\"/fixture\",\"command\":\"validate\",\"errors\":0,\"findings\":[{\"concept_id\":\"a\",\"message\":\"m\",\"rule\":\"r\",\"severity\":\"NOTE\"}],\"valid\":true,\"warnings\":0}\n",
            "0",
            "could not interpret",
        ),
        (
            "validate",
            "{\"bundle\":\"/fixture\",\"command\":\"validate\",\"errors\":2,\"findings\":[],\"valid\":false,\"warnings\":0}\n",
            "1",
            "reports 2 error(s) but lists 0",
        ),
        (
            "validate",
            "{\"bundle\":\"/fixture\",\"command\":\"validate\",\"errors\":0,\"findings\":[],\"valid\":false,\"warnings\":0}\n",
            "0",
            "valid = false alongside 0 error(s)",
        ),
        (
            "validate",
            "{\"bundle\":\"/fixture\",\"command\":\"lint\",\"errors\":0,\"findings\":[],\"valid\":true,\"warnings\":0}\n",
            "0",
            "reports command \"lint\"",
        ),
        (
            "validate",
            "{\"bundle\":\"/fixture\",\"command\":\"validate\",\"errors\":0,\"findings\":[],\"valid\":true,\"warnings\":0}\n",
            "1",
            "exited 1 where 0 was the status",
        ),
        (
            "validate",
            "{\"error\":{\"code\":500,\"kind\":\"io\",\"message\":\"stat bundle root\",\"reason\":\"ioError\"}}\n",
            "2",
            "stat bundle root",
        ),
        // `lint` suppresses errors, so listing one is a report ahu will not use.
        (
            "lint",
            "{\"bundle\":\"/fixture\",\"command\":\"lint\",\"errors\":1,\"findings\":[{\"concept_id\":\"a\",\"message\":\"m\",\"rule\":\"r\",\"severity\":\"ERROR\"}],\"valid\":false,\"warnings\":0}\n",
            "0",
            "reports 1 error(s) but lists 1",
        ),
    ];
    for (subcommand, stdout, exit, expected) in cases {
        let repo = TestRepo::new();
        configure(&repo, &["docs/knowledge"], false);
        bundle(&repo, "docs/knowledge", &[("a.md", CONCEPT)]);
        let stub = StubOkf::install();
        stub.answers("knowledge", &[]);
        stub.answers_raw("knowledge", subcommand, stdout, exit);

        let result = run(&repo, &stub, repo.path(), &["knowledge", "lint"]);
        assert_eq!(
            result.code,
            Some(5),
            "{subcommand} {stdout:?}: {}",
            result.combined()
        );
        assert!(
            result.stderr.contains(expected),
            "{subcommand} {stdout:?}: {}",
            result.stderr
        );
    }
}

// --- interface ---

#[test]
fn the_json_report_is_versioned_and_keeps_stdout_to_itself() {
    let repo = TestRepo::new();
    configure(&repo, &["docs/knowledge"], true);
    bundle(&repo, "docs/knowledge", &[("a.md", CONCEPT)]);
    let stub = StubOkf::install();
    stub.answers(
        "knowledge",
        &[(
            "WARN",
            "tables/a",
            "okf/frontmatter/tags-recommended",
            "frontmatter: 'tags' is recommended",
        )],
    );

    let result = run(
        &repo,
        &stub,
        repo.path(),
        &["knowledge", "lint", "--output", "json"],
    );
    assert_eq!(result.code, Some(5), "{}", result.combined());
    let report: serde_json::Value = serde_json::from_str(result.stdout.trim())
        .unwrap_or_else(|e| panic!("{e}: {}", result.stdout));
    assert_eq!(report["schema_version"], 1);
    assert_eq!(report["command"], "knowledge lint");
    assert_eq!(report["warnings"], 1);
    assert_eq!(report["errors"], 0);
    assert_eq!(report["fail_on_warnings"], true);
    assert_eq!(report["passed"], false);
    assert_eq!(report["bundles"][0]["path"], "docs/knowledge");
    assert_eq!(
        report["bundles"][0]["findings"][0]["rule"],
        "okf/frontmatter/tags-recommended"
    );
    assert_eq!(report["bundles"][0]["findings"][0]["severity"], "WARN");
    // The readable findings went to stderr so stdout stays parseable.
    assert!(result.stderr.contains("knowledge"), "{}", result.stderr);
}

#[test]
fn the_command_resolves_the_repository_root_from_a_subdirectory() {
    let repo = TestRepo::new();
    configure(&repo, &["docs/knowledge"], false);
    bundle(&repo, "docs/knowledge", &[("a.md", CONCEPT)]);
    repo.write("src/deep/marker.txt", "x\n");
    let stub = StubOkf::install();
    stub.answers("knowledge", &[]);

    let result = run(
        &repo,
        &stub,
        &repo.path().join("src/deep"),
        &["knowledge", "lint"],
    );
    assert_eq!(result.code, Some(0), "{}", result.combined());
    // The bundle okf was pointed at is the one under the repository root, not
    // one resolved against the working directory.
    assert!(
        stub.invocations()
            .contains(&format!("{}/docs/knowledge", repo.path().display())),
        "{}",
        stub.invocations()
    );
}

#[test]
fn the_command_rejects_arguments_it_does_not_define() {
    let repo = TestRepo::new();
    configure(&repo, &["docs/knowledge"], false);
    bundle(&repo, "docs/knowledge", &[("a.md", CONCEPT)]);
    let stub = StubOkf::install();
    stub.answers("knowledge", &[]);
    let cases: &[(&[&str], &str)] = &[
        (&["knowledge"], "needs a subcommand"),
        (&["knowledge", "fix"], "unknown subcommand"),
        (
            &["knowledge", "lint", "docs/knowledge"],
            "unknown or repeated",
        ),
        (&["knowledge", "lint", "--output"], "--output needs a value"),
        (&["knowledge", "lint", "--output", "sarif"], "expected json"),
        (
            &["knowledge", "lint", "--output", "json", "--output", "json"],
            "unknown or repeated",
        ),
    ];
    for (args, expected) in cases {
        let result = run(&repo, &stub, repo.path(), args);
        assert_eq!(result.code, Some(2), "{args:?}: {}", result.combined());
        assert!(
            result.stderr.contains(expected),
            "{args:?}: {}",
            result.stderr
        );
    }
    assert!(stub.invocations().is_empty());
}

#[test]
fn the_check_neither_starts_a_session_nor_touches_the_bundle() {
    let repo = TestRepo::new();
    configure(&repo, &["docs/knowledge"], false);
    bundle(&repo, "docs/knowledge", &[("a.md", CONCEPT)]);
    repo.commit("bundle");
    let stub = StubOkf::install();
    stub.answers("knowledge", &[]);

    let result = run(&repo, &stub, repo.path(), &["knowledge", "lint"]);
    assert_eq!(result.code, Some(0), "{}", result.combined());
    // No harness and no cmux were needed, no index was generated, and the
    // bundle is byte-for-byte what it was.
    assert_eq!(repo.read("docs/knowledge/a.md"), CONCEPT);
    assert!(!repo.path().join("docs/knowledge/index.md").exists());
    assert_eq!(
        common::git(repo.path(), &["status", "--porcelain"]),
        "",
        "the check modified the checkout"
    );
    assert_eq!(
        common::git(repo.path(), &["worktree", "list", "--porcelain"])
            .lines()
            .filter(|line| line.starts_with("worktree "))
            .count(),
        1
    );
}

#[test]
fn redirected_lint_preserves_plain_layout_and_json_values() {
    let repo = TestRepo::new();
    configure(&repo, &["docs/knowledge"], false);
    bundle(&repo, "docs/knowledge", &[("a.md", CONCEPT)]);
    let stub = StubOkf::install();
    let hostile = "BEGIN\x1b[2J\u{202e}\u{200b}END";
    for findings in [
        vec![],
        vec![("WARN", hostile, hostile, hostile)],
        vec![("ERROR", hostile, hostile, hostile)],
    ] {
        stub.answers("knowledge", &findings);
        for json in [false, true] {
            let render = |color: &str| {
                let stdout = tempfile::NamedTempFile::new().unwrap();
                let mut command = common::ahu();
                command.args([color, "knowledge", "lint"]);
                if json {
                    command.args(["--output", "json"]);
                }
                let result = command
                    .current_dir(repo.path())
                    .env("STUB_OKF_FIXTURES", stub.fixtures())
                    .env("PATH", format!("{}:/usr/bin:/bin", stub.bin().display()))
                    .env("TERM", "xterm-256color")
                    .env_remove("NO_COLOR")
                    .stdin(Stdio::null())
                    .stdout(stdout.reopen().unwrap())
                    .output()
                    .unwrap();
                (
                    result.status.code(),
                    std::fs::read(stdout.path()).unwrap(),
                    result.stderr,
                )
            };
            let plain = render("--color=never");
            assert_eq!(render("--color=auto"), plain);
            assert_eq!(
                plain.0,
                Some(if findings.first().is_some_and(|f| f.0 == "ERROR") {
                    5
                } else {
                    0
                })
            );
            let colored = render("--color=always");
            assert_eq!(colored.0, plain.0);
            let human = if json { &colored.2 } else { &colored.1 };
            let plain_human = if json { &plain.2 } else { &plain.1 };
            let text = String::from_utf8(human.clone()).unwrap();
            assert!(text.contains('\x1b'));
            let mut stripped = text.clone();
            for sgr in [
                "\x1b[1m",
                "\x1b[2m",
                "\x1b[33m",
                "\x1b[1;31m",
                "\x1b[32m",
                "\x1b[0m",
            ] {
                stripped = stripped.replace(sgr, "");
            }
            assert_eq!(stripped.as_bytes(), plain_human);
            assert!(!stripped.contains('\x1b'));
            assert!(!text.contains('\u{202e}'));
            assert!(!text.contains('\u{200b}'));
            if !findings.is_empty() {
                assert_eq!(text.matches(&ahu::util::display_safe(hostile)).count(), 3);
            }
            if json {
                assert_eq!(colored.1, plain.1);
                let value: serde_json::Value = serde_json::from_slice(&colored.1).unwrap();
                assert_eq!(value["schema_version"], 1);
                if !findings.is_empty() {
                    for field in ["concept_id", "rule", "message"] {
                        assert_eq!(value["bundles"][0]["findings"][0][field], hostile);
                    }
                }
            }
        }
    }
}
