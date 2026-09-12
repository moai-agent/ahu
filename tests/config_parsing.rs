//! Project configuration and agent manifest parsing.

mod common;

use ahu::{agent, catalog, config, selection, util};
use common::TestRepo;

#[test]
fn missing_config_is_the_signal_to_initialize_not_an_error() {
    let repo = TestRepo::new();
    let loaded = config::load(repo.path()).expect("load succeeds");
    assert!(loaded.is_none());
}

#[test]
fn valid_config_loads_with_a_digest_over_its_exact_bytes() {
    let repo = TestRepo::new();
    repo.init_config();
    let loaded = config::load(repo.path()).unwrap().unwrap();
    assert_eq!(loaded.config.harness_preferences, vec!["claude-code"]);
    assert_eq!(loaded.config.context_hygiene.review_interval_days, 7);
    assert_eq!(loaded.digest.len(), 64);

    // A byte change is a different policy snapshot, committed or not.
    let before = loaded.digest.clone();
    let text = repo.read(".agents/ahu/config.toml");
    repo.write(
        ".agents/ahu/config.toml",
        &text.replace("review_interval_days = 7", "review_interval_days = 14"),
    );
    let after = config::load(repo.path()).unwrap().unwrap();
    assert_ne!(before, after.digest);
}

#[test]
fn malformed_config_is_reported_and_never_rewritten() {
    let repo = TestRepo::new();
    repo.write(".agents/ahu/config.toml", "this is not toml = = =\n");
    let error = config::load(repo.path()).unwrap_err().to_string();
    assert!(error.contains("not valid ahu configuration"), "{error}");
    assert!(error.contains("will not rewrite or reset it"), "{error}");
    assert_eq!(
        repo.read(".agents/ahu/config.toml"),
        "this is not toml = = =\n"
    );
}

#[test]
fn unknown_schema_version_is_refused() {
    let repo = TestRepo::new();
    repo.init_config();
    let text = repo.read(".agents/ahu/config.toml");
    repo.write(
        ".agents/ahu/config.toml",
        &text.replace("schema_version = 1", "schema_version = 99"),
    );
    let error = config::load(repo.path()).unwrap_err().to_string();
    assert!(error.contains("schema_version 99"), "{error}");
}

#[test]
fn a_pinned_catalog_this_build_does_not_ship_blocks_selection() {
    let repo = TestRepo::new();
    repo.init_config();
    let text = repo.read(".agents/ahu/config.toml");
    repo.write(
        ".agents/ahu/config.toml",
        &text.replace(catalog::CATALOG_VERSION, "1999-01-01"),
    );
    let error = config::load(repo.path()).unwrap_err().to_string();
    assert!(error.contains("1999-01-01"), "{error}");
    assert!(error.contains("will not substitute"), "{error}");
}

#[test]
fn unknown_harness_or_model_in_the_ranking_is_refused() {
    let repo = TestRepo::new();
    repo.init_config();
    let text = repo.read(".agents/ahu/config.toml");
    repo.write(
        ".agents/ahu/config.toml",
        &text.replace("\"claude-opus-5\"", "\"gpt-hypothetical\""),
    );
    let error = config::load(repo.path()).unwrap_err().to_string();
    assert!(error.contains("gpt-hypothetical"), "{error}");
}

#[test]
fn empty_harness_preferences_is_refused() {
    let repo = TestRepo::new();
    repo.init_config();
    let text = repo.read(".agents/ahu/config.toml");
    repo.write(
        ".agents/ahu/config.toml",
        &text.replace(
            "harness_preferences = [\"claude-code\"]",
            "harness_preferences = []",
        ),
    );
    let error = config::load(repo.path()).unwrap_err().to_string();
    assert!(error.contains("harness_preferences is empty"), "{error}");
}

#[test]
fn writing_config_is_exclusive_so_a_concurrent_init_cannot_be_overwritten() {
    let repo = TestRepo::new();
    let new_config = config::ProjectConfig {
        schema_version: 1,
        harness_preferences: vec!["claude-code".to_string()],
        model_selection: "project-ranked".to_string(),
        catalog_version: catalog::CATALOG_VERSION.to_string(),
        model_rankings: [("claude-code".to_string(), vec!["claude-opus-5".to_string()])]
            .into_iter()
            .collect(),
        context_hygiene: config::ContextHygiene::default(),
        knowledge: config::Knowledge::default(),
    };
    let path = config::write_new(repo.path(), &new_config).expect("first write");
    assert!(path.exists());
    let first = std::fs::read_to_string(&path).unwrap();

    let error = config::write_new(repo.path(), &new_config)
        .unwrap_err()
        .to_string();
    assert!(error.contains("already exists"), "{error}");
    assert!(error.contains("nothing was overwritten"), "{error}");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), first);
}

#[test]
fn rendered_config_round_trips() {
    let repo = TestRepo::new();
    let original = config::ProjectConfig {
        schema_version: 1,
        harness_preferences: vec!["claude-code".to_string()],
        model_selection: "project-ranked".to_string(),
        catalog_version: catalog::CATALOG_VERSION.to_string(),
        model_rankings: [(
            "claude-code".to_string(),
            vec!["claude-opus-5".to_string(), "claude-sonnet-5".to_string()],
        )]
        .into_iter()
        .collect(),
        context_hygiene: config::ContextHygiene {
            review_on_first_load: true,
            review_interval_days: 3,
        },
        // A non-default knowledge section so the round trip proves it is
        // rendered, not silently dropped back to the default. The non-ASCII
        // paths are the reason the bundles are rendered as TOML rather than
        // with `{:?}`: Rust's escapes for these are not TOML's, so a config
        // rendered with them would load once and then fail to parse.
        knowledge: config::Knowledge {
            bundles: vec![
                "docs/knowledge".to_string(),
                "docs/non\u{a0}breaking".to_string(),
                "docs/combin\u{301}ing".to_string(),
            ],
            fail_on_warnings: true,
        },
    };
    config::write_new(repo.path(), &original).unwrap();
    let loaded = config::load(repo.path()).unwrap().unwrap();
    assert_eq!(loaded.config, original);
}

// --- agent manifests ---

#[test]
fn a_registered_agent_resolves_its_native_definition() {
    let repo = TestRepo::new();
    repo.init_config();
    repo.add_agent("chris", "1.2.0", "claude-opus-5");
    let found = agent::find(repo.path(), "chris").expect("chris resolves");
    assert_eq!(found.label(), "chris@1.2.0");
    assert_eq!(found.manifest.model, "claude-opus-5");
    assert!(found.instructions.contains("You are chris."));
    assert_eq!(found.native_model.as_deref(), Some("claude-opus-5"));
    // Native fields are read, not removed: the file itself is untouched.
    assert!(found.native_settings.contains_key("tools"));
    assert!(
        repo.read(".claude/agents/chris.md")
            .contains("tools: Read, Edit")
    );
}

#[test]
fn a_native_model_that_disagrees_with_the_manifest_blocks_the_launch() {
    let repo = TestRepo::new();
    repo.init_config();
    repo.add_agent("chris", "1.0.0", "claude-opus-5");
    let native = repo.read(".claude/agents/chris.md");
    repo.write(
        ".claude/agents/chris.md",
        &native.replace("model: claude-opus-5", "model: claude-sonnet-5"),
    );
    let error = agent::find(repo.path(), "chris").unwrap_err().to_string();
    assert!(error.contains("will not rewrite either file"), "{error}");
}

#[test]
fn a_named_agent_without_a_semantic_version_is_refused() {
    let repo = TestRepo::new();
    repo.init_config();
    repo.add_agent("chris", "1.0.0", "claude-opus-5");
    let manifest = repo.read(".agents/ahu/agents/chris.toml");
    repo.write(
        ".agents/ahu/agents/chris.toml",
        &manifest.replace("version = \"1.0.0\"", "version = \"latest\""),
    );
    let error = agent::find(repo.path(), "chris").unwrap_err().to_string();
    assert!(error.contains("not a semantic version"), "{error}");
}

#[test]
fn manifest_stem_and_name_must_match() {
    let repo = TestRepo::new();
    repo.init_config();
    repo.add_agent("chris", "1.0.0", "claude-opus-5");
    let manifest = repo.read(".agents/ahu/agents/chris.toml");
    repo.write(".agents/ahu/agents/sam.toml", &manifest);
    let error = agent::load_all(repo.path()).unwrap_err().to_string();
    assert!(error.contains("does not match the file stem"), "{error}");
}

#[test]
fn a_model_outside_the_catalog_is_refused_rather_than_substituted() {
    let repo = TestRepo::new();
    repo.init_config();
    repo.add_agent("chris", "1.0.0", "claude-opus-5");
    for file in [".agents/ahu/agents/chris.toml", ".claude/agents/chris.md"] {
        let text = repo.read(file);
        repo.write(file, &text.replace("claude-opus-5", "claude-imaginary-9"));
    }
    let error = agent::find(repo.path(), "chris").unwrap_err().to_string();
    assert!(
        error.contains("will not substitute a different model"),
        "{error}"
    );
}

#[test]
fn a_source_path_outside_the_repository_is_refused() {
    let repo = TestRepo::new();
    repo.init_config();
    repo.write(
        ".agents/ahu/agents/escape.toml",
        "schema_version = 1\n\
         name = \"escape\"\n\
         version = \"0.1.0\"\n\
         harness = \"claude-code\"\n\
         model = \"claude-opus-5\"\n\
         \n[source]\n\
         format = \"markdown\"\n\
         path = \"../outside.md\"\n",
    );
    let error = agent::load_all(repo.path()).unwrap_err().to_string();
    assert!(error.contains("escapes the repository root"), "{error}");
}

#[test]
fn an_uncommitted_agent_is_immediately_usable() {
    let repo = TestRepo::new();
    repo.init_config();
    repo.add_agent("chris", "0.1.0", "claude-opus-5");
    // Nothing has been committed or staged.
    let status = common::git(repo.path(), &["status", "--porcelain"]);
    assert!(status.contains(".agents/"), "{status}");
    assert!(agent::find(repo.path(), "chris").is_ok());
}

#[test]
fn a_missing_agent_never_falls_back_to_automatic_selection() {
    let repo = TestRepo::new();
    repo.init_config();
    repo.add_agent("chris", "0.1.0", "claude-opus-5");
    let error = agent::find(repo.path(), "sam").unwrap_err().to_string();
    assert!(error.contains("never substitutes"), "{error}");
    assert!(error.contains("@chris"), "{error}");
}

#[test]
fn a_manifest_may_not_point_at_another_harnesss_definition_format() {
    let repo = TestRepo::new();
    repo.init_config();
    repo.write(".codex/agents/sam.toml", "model = \"something\"\n");
    repo.write(
        ".agents/ahu/agents/sam.toml",
        "schema_version = 1\n\
         name = \"sam\"\n\
         version = \"0.1.0\"\n\
         harness = \"claude-code\"\n\
         model = \"claude-opus-5\"\n\
         \n[source]\n\
         format = \"codex-agent\"\n\
         path = \".codex/agents/sam.toml\"\n",
    );
    let error = agent::load_all(repo.path()).unwrap_err().to_string();
    assert!(
        error.contains("does not translate an agent from one harness to another"),
        "{error}"
    );
}

#[test]
fn each_supported_harness_pins_models_from_its_own_catalog_entry() {
    // A model belonging to another harness is refused rather than substituted.
    let repo = TestRepo::new();
    repo.init_config();
    repo.write(".agents/ahu/instructions/sam.md", "You are sam.\n");
    repo.write(
        ".agents/ahu/agents/sam.toml",
        "schema_version = 1\n\
         name = \"sam\"\n\
         version = \"0.1.0\"\n\
         harness = \"codex\"\n\
         model = \"claude-opus-5\"\n\
         \n[source]\n\
         format = \"markdown\"\n\
         path = \".agents/ahu/instructions/sam.md\"\n",
    );
    let error = agent::load_all(repo.path()).unwrap_err().to_string();
    assert!(
        error.contains("is not a catalog model for harness"),
        "{error}"
    );
    assert!(
        error.contains("will not substitute a different model"),
        "{error}"
    );
}

// --- automatic selection ---

#[test]
fn automatic_selection_follows_the_project_order_deterministically() {
    let repo = TestRepo::new();
    repo.init_config();
    let loaded = config::load(repo.path()).unwrap().unwrap();
    let pair = selection::resolve_automatic(&loaded).expect("resolves");
    assert_eq!(pair.harness, "claude-code");
    assert_eq!(pair.model, "claude-opus-5");
    assert_eq!(pair.policy_digest, loaded.digest);
    assert!(pair.basis.contains("harness_preferences"), "{}", pair.basis);
    // Repeating the resolution gives the same answer.
    assert_eq!(selection::resolve_automatic(&loaded).unwrap(), pair);
}

#[test]
fn a_project_order_naming_only_unsupported_harnesses_reports_a_policy_problem() {
    let repo = TestRepo::new();
    repo.write(
        ".agents/ahu/config.toml",
        &format!(
            "schema_version = 1\n\
             harness_preferences = [\"codex\"]\n\
             model_selection = \"project-ranked\"\n\
             catalog_version = \"{}\"\n\
             \n[context_hygiene]\n\
             review_on_first_load = true\n\
             review_interval_days = 7\n",
            catalog::CATALOG_VERSION
        ),
    );
    let loaded = config::load(repo.path()).unwrap().unwrap();
    let error = selection::resolve_automatic(&loaded)
        .unwrap_err()
        .to_string();
    assert!(error.contains("project policy question"), "{error}");
}

#[test]
fn semantic_version_validation_accepts_and_rejects_the_expected_forms() {
    for good in ["0.1.0", "1.2.3", "10.20.30", "1.0.0-rc.1", "1.0.0+build.5"] {
        assert!(util::is_semver(good), "{good} should be valid");
    }
    for bad in [
        "", "1", "1.2", "1.2.3.4", "v1.2.3", "01.2.3", "1.2.x", "latest",
    ] {
        assert!(!util::is_semver(bad), "{bad} should be invalid");
    }
}

/// A repository must not be able to choose which file ahu opens.
///
/// Every *write* inside a repository goes through `util::resolve_within`, which
/// refuses a symlinked component. The read side did not, so a committed
/// `.agents/ahu/agents/<name>.toml` symlink pointed anywhere made
/// `agent::load_all` open that file — and `toml`'s parse error quotes the
/// offending source line verbatim, so the contents were printed. `load_all`
/// runs on plain `ahu` before any prompt is typed, and on `ahu agents`.
#[test]
fn a_symlinked_manifest_is_refused_rather_than_read_through() {
    let repo = TestRepo::new();
    repo.init_config();

    let secret = repo.path().join("outside-the-repo.txt");
    std::fs::write(&secret, "SYNTHETIC_TOKEN = \"not-a-real-credential\"\n").unwrap();
    std::fs::create_dir_all(repo.path().join(".agents/ahu/agents")).unwrap();
    std::os::unix::fs::symlink(&secret, repo.path().join(".agents/ahu/agents/leak.toml")).unwrap();

    let error = agent::load_all(repo.path()).expect_err("a symlinked manifest must be refused");
    let message = error.to_string();
    assert!(
        message.contains("refusing to act through a symlink"),
        "{message}"
    );
    assert!(
        !message.contains("SYNTHETIC_TOKEN"),
        "the target's contents leaked into the error: {message}"
    );
}

/// The same rule for the project configuration itself.
#[test]
fn a_symlinked_config_is_refused_rather_than_read_through() {
    let repo = TestRepo::new();
    let secret = repo.path().join("outside-the-repo.txt");
    std::fs::write(&secret, "SYNTHETIC_TOKEN = \"not-a-real-credential\"\n").unwrap();
    std::fs::create_dir_all(repo.path().join(".agents/ahu")).unwrap();
    std::os::unix::fs::symlink(&secret, repo.path().join(".agents/ahu/config.toml")).unwrap();

    let error = config::load(repo.path()).expect_err("a symlinked config must be refused");
    let message = error.to_string();
    assert!(
        message.contains("refusing to act through a symlink"),
        "{message}"
    );
    assert!(!message.contains("SYNTHETIC_TOKEN"), "{message}");
}

/// A symlinked `.agents` directory redirects the whole registry, not one file.
#[test]
fn a_symlinked_agents_directory_is_refused() {
    let repo = TestRepo::new();
    repo.init_config();
    let elsewhere = repo.path().join("elsewhere");
    std::fs::create_dir_all(&elsewhere).unwrap();
    std::fs::write(elsewhere.join("x.toml"), "SYNTHETIC = 1\n").unwrap();

    // Replace `.agents/ahu/agents` with a link to a directory ahu never vetted.
    let agents = repo.path().join(".agents/ahu/agents");
    std::fs::create_dir_all(agents.parent().unwrap()).unwrap();
    let _ = std::fs::remove_dir_all(&agents);
    std::os::unix::fs::symlink(&elsewhere, &agents).unwrap();

    let error = agent::load_all(repo.path()).expect_err("a symlinked registry must be refused");
    assert!(
        error
            .to_string()
            .contains("refusing to act through a symlink"),
        "{error}"
    );
}

/// ahu resolves a harness in order to execute it, so a `PATH` entry that is
/// empty or relative must not be honoured.
///
/// `PATH=/usr/bin:` has an empty trailing entry. Treated the way a shell treats
/// it, `claude` resolves against the current working directory — which for
/// `ahu run-task` is the task worktree, a checkout of the repository. The name
/// check in `run_task` cannot catch that, because the file name of the bare
/// relative path `claude` is exactly `claude`.
#[test]
fn a_relative_path_entry_never_resolves_a_harness() {
    let repo = TestRepo::new();
    let planted = repo.path().join("claude");
    std::fs::write(&planted, "#!/bin/sh\nexit 0\n").unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&planted, std::fs::Permissions::from_mode(0o755)).unwrap();

    // Resolution reads the process environment, so drive it through a child
    // process rather than mutating this test binary's own PATH.
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_ahu"))
        .arg("doctor")
        .current_dir(repo.path())
        .env("AHU_STATE_DIR", repo.state_path())
        .env("PATH", format!("{}:", std::env::var("PATH").unwrap()))
        .output()
        .expect("ahu runs");
    let combined = String::from_utf8_lossy(&output.stdout).to_string();

    // `doctor` prints the resolved path for each harness. Whatever it found, it
    // must not be the repository's own file.
    assert!(
        !combined.contains(&format!("{}/claude", repo.path().display()))
            && !combined.contains(" claude "),
        "a harness was resolved from the repository: {combined}"
    );
}

/// Hooks must not be read through a symlinked `.claude`.
///
/// `Scope::Project` is the one scope whose `why_not_project_policy` is "it is
/// project policy", so `outside_project_policy` excludes it and the
/// non-project-hook warning is suppressed. Reading through a symlink labelled
/// hooks from outside the repository as project policy, and the preview also
/// claimed they travel into the task worktree — which is false, because
/// `materialize` deletes configuration symlinks from it. Both errors pointed
/// the permissive way.
#[test]
fn hooks_are_not_read_through_a_symlinked_claude_directory() {
    let repo = TestRepo::new();
    let outside = repo.path().join("outside-claude");
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(
        outside.join("settings.json"),
        r#"{"hooks":{"PreToolUse":[{"matcher":"*","hooks":[{"type":"command","command":"/usr/bin/synthetic-marker"}]}]}}"#,
    )
    .unwrap();
    std::os::unix::fs::symlink(&outside, repo.path().join(".claude")).unwrap();

    let found = ahu::hooks::collect(repo.path(), "claude-code").expect("collect succeeds");
    assert!(
        !found
            .hooks
            .iter()
            .any(|h| h.command.as_deref() == Some("/usr/bin/synthetic-marker")),
        "a hook was read through a symlinked .claude: {:?}",
        found.hooks
    );
    assert!(
        found.unreadable.iter().any(|u| u.contains("symlink")),
        "the refusal must be disclosed, not silent: {:?}",
        found.unreadable
    );
}

/// A proposed manifest must be parseable by the loader that will read it back.
///
/// `proposed_manifest` built its TOML with Rust's `{:?}`, which escapes a
/// non-printable character as `\u{XXXX}`; TOML's escape is `\uXXXX`. So a
/// `description:` in a native definition's frontmatter carrying a bidi
/// override, an ESC, or a zero-width character made `onboard --register` write
/// a manifest `agent::load_all` could not parse — and one unparseable manifest
/// fails the entire registry load, so `ahu agents`, `ahu onboard` and the
/// launcher's agent list all stayed broken until the file was deleted by hand.
#[test]
fn a_proposed_manifest_round_trips_through_the_loader() {
    let repo = TestRepo::new();
    repo.init_config();

    for hostile in [
        "plain description",
        "bidi \u{202e}override",
        "escape \u{1b}[2J here",
        "zero \u{200b} width",
        "quotes \" and \\ backslash",
        "newline \n inside",
    ] {
        repo.write(
            ".claude/agents/probe.md",
            &format!(
                "---\nname: probe\ndescription: {hostile}\nmodel: claude-opus-5\n---\n\nBody.\n"
            ),
        );
        let candidates = ahu::onboard::preview(repo.path()).expect("preview");
        let candidate = candidates
            .iter()
            .find(|c| c.name == "probe")
            .expect("probe is a candidate");

        let body = ahu::onboard::proposed_manifest(candidate, "claude-opus-5", "0.1.0");
        let parsed: Result<toml::Value, _> = toml::from_str(&body);
        assert!(
            parsed.is_ok(),
            "proposed manifest is not valid TOML for description {hostile:?}:\n{body}\n{:?}",
            parsed.err().map(|e| e.to_string())
        );

        // And it must survive the real loader, not just a generic TOML parse.
        let written = repo.path().join(".agents/ahu/agents/probe.toml");
        std::fs::create_dir_all(written.parent().unwrap()).unwrap();
        std::fs::write(&written, &body).unwrap();
        let loaded = agent::load_all(repo.path());
        assert!(
            loaded.is_ok(),
            "the registry no longer loads after registering description {hostile:?}: {:?}",
            loaded.err().map(|e| e.to_string())
        );
        std::fs::remove_file(&written).unwrap();
    }
}
