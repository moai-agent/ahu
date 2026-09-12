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
