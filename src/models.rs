//! Model choices for setup. Prefer lists exposed by a configured harness and
//! keep ahu's compatibility catalog as the final authority for launchable IDs.

use std::path::Path;
use std::process::Command;
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Harness,
    Catalog,
}

pub struct Options {
    pub models: Vec<&'static crate::catalog::ModelEntry>,
    pub source: Source,
}

/// Return only models that ahu knows how to launch for this harness. Harnesses
/// with a noninteractive listing command are filtered against their current
/// local/provider configuration; the rest use reviewed catalog entries and
/// are clearly marked as not entitlement-checked.
pub fn for_harness(harness: &str) -> Options {
    let live = live_model_ids(harness);
    let mut models = crate::catalog::models_for(harness);
    if let Some(ids) = &live {
        models.retain(|model| ids.iter().any(|id| id == model.model));
    }
    Options {
        models,
        source: if live.is_some() {
            Source::Harness
        } else {
            Source::Catalog
        },
    }
}

fn live_model_ids(harness: &str) -> Option<Vec<String>> {
    let program = match harness {
        "antigravity" => "agy",
        "opencode" => "opencode",
        "kiro" => "kiro-cli",
        // These CLIs accept explicit model IDs but do not expose a documented
        // noninteractive list of account-available models.
        "claude-code" | "codex" => return None,
        _ => return None,
    };
    let executable = crate::selection::resolve_executable(program)?;
    if harness == "kiro" {
        return crate::harness::kiro::available_models(
            Path::new(&executable),
            &std::env::current_dir().ok()?,
        )
        .ok();
    }
    let cwd = std::env::current_dir().ok()?;
    run_model_listing(
        harness,
        Path::new(&executable),
        &cwd,
        Duration::from_secs(8),
    )
}

fn run_model_listing(
    harness: &str,
    executable: &Path,
    cwd: &Path,
    timeout: Duration,
) -> Option<Vec<String>> {
    let program = match harness {
        "antigravity" | "opencode" => executable,
        _ => return None,
    };
    let before = crate::snapshot::collect(cwd).ok()?.digest();
    let mut command = Command::new(program);
    command.arg("models").current_dir(cwd);
    let output = crate::selection::bounded_config_probe(&mut command, true, timeout)?;
    if crate::snapshot::collect(cwd).ok()?.digest() != before {
        return None;
    }
    Some(parse_ids(harness, &output))
}

fn parse_ids(harness: &str, output: &str) -> Vec<String> {
    output
        .lines()
        .filter_map(|line| match harness {
            "antigravity" => line.split_whitespace().next(),
            "opencode" => line.split_whitespace().find(|token| token.contains('/')),
            _ => None,
        })
        .filter(|id| !id.is_empty())
        .filter(|id| crate::catalog::model(harness, id).is_some())
        .map(str::to_string)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_harnesses_with_a_noninteractive_model_list_are_probed() {
        assert_eq!(live_model_ids("codex"), None);
        assert_eq!(live_model_ids("claude-code"), None);
        assert_eq!(live_model_ids("unknown"), None);
    }

    #[test]
    fn catalog_fallback_is_explicit_for_non_listable_harnesses() {
        let options = for_harness("codex");
        assert_eq!(options.source, Source::Catalog);
        assert!(!options.models.is_empty());
    }

    #[test]
    fn parses_the_noninteractive_model_list_shapes() {
        assert_eq!(
            parse_ids(
                "antigravity",
                "Fetching...\ngemini-3.8-flash-high\tGemini Flash\n"
            ),
            ["gemini-3.8-flash-high"]
        );
        assert_eq!(
            parse_ids("opencode", "opencode/example\nollama/glm-5.3:cloud\n"),
            ["ollama/glm-5.3:cloud"]
        );
    }

    #[cfg(unix)]
    #[test]
    fn model_listing_accepts_success_and_falls_back_on_failure_or_timeout() {
        use std::os::unix::fs::PermissionsExt;

        fn command(root: &Path, name: &str, body: &str) -> std::path::PathBuf {
            let path = root.join(name);
            std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
            let mut permissions = std::fs::metadata(&path).unwrap().permissions();
            permissions.set_mode(0o700);
            std::fs::set_permissions(&path, permissions).unwrap();
            path
        }

        let root = tempfile::tempdir().unwrap();
        let success = command(
            root.path(),
            "success",
            "printf 'gemini-3.8-flash-high\\tFlash\\nunknown-model\\n'",
        );
        assert_eq!(
            run_model_listing("antigravity", &success, root.path(), Duration::from_secs(5)),
            Some(vec!["gemini-3.8-flash-high".to_string()])
        );

        let failure = command(root.path(), "failure", "exit 1");
        assert_eq!(
            run_model_listing("antigravity", &failure, root.path(), Duration::from_secs(5)),
            None
        );
        assert_eq!(
            run_model_listing(
                "antigravity",
                Path::new("/path/that/does/not/exist"),
                root.path(),
                Duration::from_secs(1)
            ),
            None
        );

        let sleeper = command(root.path(), "sleeper", "sleep 1");
        assert_eq!(
            run_model_listing("opencode", &sleeper, root.path(), Duration::from_millis(20)),
            None
        );
    }

    #[cfg(unix)]
    #[test]
    fn model_listing_rejects_changes_to_snapshotted_agent_configuration() {
        use std::os::unix::fs::PermissionsExt;

        let root = tempfile::tempdir().unwrap();
        let config = root.path().join(".agents/ahu/config.md");
        std::fs::create_dir_all(config.parent().unwrap()).unwrap();
        std::fs::write(&config, "before\n").unwrap();
        let executable = root.path().join("mutating-model-list");
        std::fs::write(
            &executable,
            "#!/bin/sh\nprintf 'ollama/glm-5.3:cloud\\n'\nprintf 'after\\n' >> .agents/ahu/config.md\n",
        )
        .unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();

        assert_eq!(
            run_model_listing("opencode", &executable, root.path(), Duration::from_secs(5)),
            None
        );
        assert_eq!(std::fs::read_to_string(config).unwrap(), "before\nafter\n");
    }

    #[test]
    fn model_parser_ignores_unrecognized_harnesses_and_empty_ids() {
        assert!(parse_ids("unknown", "id/name\n").is_empty());
        assert!(parse_ids("opencode", "plain-name\n").is_empty());
        assert!(parse_ids("antigravity", "\n  \n").is_empty());
    }
}
