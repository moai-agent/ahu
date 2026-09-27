//! Model choices for setup. Prefer lists exposed by a configured harness and
//! keep ahu's compatibility catalog as the final authority for launchable IDs.

use std::io::Read;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

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
    let (program, args): (&str, &[&str]) = match harness {
        "antigravity" => ("agy", &["models"]),
        "opencode" => ("opencode", &["models"]),
        // These CLIs accept explicit model IDs but do not expose a documented
        // noninteractive list of account-available models.
        "claude-code" | "codex" => return None,
        _ => return None,
    };
    let executable = crate::selection::resolve_executable(program)?;
    let mut child = Command::new(executable)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut stdout = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut output = String::new();
        stdout.read_to_string(&mut output).ok().map(|_| output)
    });
    let deadline = Instant::now() + Duration::from_secs(8);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(40));
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
        }
    }?;
    let output = reader.join().ok()??;
    if !status.success() {
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
}
