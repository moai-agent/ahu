//! Argument parsing and help text.

use std::path::PathBuf;

use crate::bail;
use crate::util::Result;

pub const HELP: &str = "ahu - The moai-agent command-line interface

Launch repository-defined agents in fresh Git worktrees and organise their
interactive sessions in cmux.

Usage: ahu [COMMAND]

Running `ahu` with no command opens the interactive launcher: pick an agent with
`@name` (or leave it blank for the project's automatic selection), paste a task,
and submit it. Pasting never submits by itself: the preview ends with a
confirmation code generated after your prompt was read, and only that code
submits, so no pasted text can answer for you.

An agent's instructions and ahu's delegation contract are delivered as prompt
text on every harness, fenced with a per-launch nonce. ahu passes no
agent-selection or system-prompt flag anywhere, so none of it is enforced by the
harness -- the preview says so as a gap on every launch. What ahu does pin with
real flags is the harness, the exact model, and any permission widening a
committed manifest asks for.

Commands:
  help                  Print this help message
  explain               Architecture overview and Mermaid diagrams
  init                  Record this project's agreed harness and model order
  launch @name [--prompt <text> | --prompt-file <path>] [--dry-run]
                        Assign work in a separate cmux session. Reads no
                        confirmation, so approval widening needs an explicit flag
  agents                List the agents registered for this repository
  onboard               Preview native agent definitions that could be registered
  inventory [@agent]    Show everything that can influence an agent's context
  hygiene [@agent]      Run the context hygiene review now
  tasks                 List tasks launched from this repository
  task <task-id> [--output json]
                        Inspect a task's recorded session state and locations
  diff <task-id>         Review tracked changes since launch; list untracked files
  focus <task-id>       Bring a task's cmux session to the front
  doctor                Check repository, configuration, harness, and cmux
  codex                 Open Codex here with workspace-write sandboxing and
                        on-request approvals (uses Codex's configured model)
  run-task              Internal: run a prepared task (used by cmux)

Options:
  -h, --help            Print this help message
  -V, --version         Print the version
  --color <choice>      auto, always, or never (also --color=<choice>).
                        Always/never override NO_COLOR. Auto honors any
                        NO_COLOR value and requires stdout to be a
                        terminal and TERM to differ from dumb.

explain options:
  --markdown            Print the overview as a Markdown document
  --mermaid             Print only the diagrams, as fenced Mermaid blocks
  --open                Render it in cmux's Markdown viewer, diagrams and all

onboard options:
  --register <name>     Register a previewed native definition
  --remove <name>       Remove one ahu registration (native files are untouched)
  --model <id>          Exact model identifier for a registration
  --agent-version <v>   Semantic version for a new registration (default 0.1.0)

launcher options:
  --no-focus            Do not switch to the new session after launching

launch options:
  --prompt <text>       Use an inline prompt (conflicts with --prompt-file)
  --prompt-file <path>  Read a UTF-8 prompt file
                        With neither option, read non-terminal stdin to EOF.
                        Explicit sources take precedence over unread stdin.
  --output json        Emit a versioned JSON plan; requires --dry-run.
                        JSON goes to stdout, diagnostics to stderr.
  --dry-run             Show the preview and create nothing
  --allow-widened-approvals
                        Required to launch an agent whose manifest declares
                        permissions = auto or accept-edits. `ahu launch` reads no
                        confirmation, so widening is opt-in on the command line

Exit codes:
  0 success; 1 cancelled; 2 usage error; 3 unknown agent;
  4 missing prerequisite; 5 run failure.

run-task options:
  --task-dir <path>     Directory holding the prepared task record";

/// How `ahu explain` should present itself.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum ExplainFormat {
    /// Plain text for a terminal.
    Terminal,
    /// The same document as Markdown, on stdout.
    Markdown,
    /// Only the diagrams, as fenced Mermaid blocks.
    Mermaid,
    /// Written to ahu's state directory and opened in cmux's Markdown viewer.
    OpenInCmux,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Command {
    Help,
    Version,
    Explain {
        format: ExplainFormat,
    },
    Interactive {
        focus: bool,
    },
    Init,
    Launch {
        agent: String,
        prompt: PromptSource,
        output_json: bool,
        dry_run: bool,
        /// Opt in to launching an agent whose manifest widens the harness's own
        /// approval boundary. Required on this path because it has no
        /// interactive confirmation.
        allow_widened_approvals: bool,
    },
    Agents,
    Onboard {
        register: Option<String>,
        remove: Option<String>,
        model: Option<String>,
        version: String,
    },
    Inventory {
        agent: Option<String>,
    },
    Hygiene {
        agent: Option<String>,
    },
    Tasks,
    Task {
        task_id: String,
        output_json: bool,
    },
    Diff {
        task_id: String,
    },
    Focus {
        task_id: String,
    },
    Doctor,
    Codex,
    RunTask {
        task_dir: PathBuf,
    },
}

/// The explicit source wins over stdin; stdin is only a fallback.
#[derive(Debug, PartialEq, Eq)]
pub enum PromptSource {
    File(PathBuf),
    Inline(String),
    Stdin,
}

impl PromptSource {
    pub fn read(&self, input: &mut impl std::io::Read, stdin_is_terminal: bool) -> Result<String> {
        let prompt = match self {
            Self::File(path) => std::fs::read_to_string(path).map_err(|error| {
                crate::util::Error::new(format!(
                    "cannot read prompt file {}: {error}",
                    path.display()
                ))
            })?,
            Self::Inline(text) => text.clone(),
            Self::Stdin => {
                if stdin_is_terminal {
                    return Err(crate::util::Error::new(
                        "ahu launch needs --prompt or --prompt-file when stdin is a terminal.",
                    )
                    .with_kind(crate::util::ErrorKind::Usage));
                }
                let mut text = String::new();
                input.read_to_string(&mut text)?;
                text
            }
        };
        if prompt.trim().is_empty() {
            return Err(
                crate::util::Error::new("the task prompt is empty; nothing was launched.")
                    .with_kind(crate::util::ErrorKind::Usage),
            );
        }
        Ok(prompt)
    }
}

/// Parse `args`, which excludes the executable name.
pub fn parse<I, S>(args: I) -> Result<Command>
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    parse_with_stdin(args, false)
}

pub fn parse_with_stdin<I, S>(args: I, stdin_available: bool) -> Result<Command>
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    parse_inner(args.into_iter().map(Into::into).collect(), stdin_available)
        .map_err(|e| e.with_kind(crate::util::ErrorKind::Usage))
}

fn parse_inner(args: Vec<String>, stdin_available: bool) -> Result<Command> {
    let Some(first) = args.first().map(String::as_str) else {
        return Ok(Command::Interactive { focus: true });
    };
    match first {
        "help" | "-h" | "--help" => {
            expect_no_more(&args[1..])?;
            Ok(Command::Help)
        }
        "-V" | "--version" => {
            expect_no_more(&args[1..])?;
            Ok(Command::Version)
        }
        "explain" => {
            let format = match args.get(1).map(String::as_str) {
                None => ExplainFormat::Terminal,
                Some("--markdown") => ExplainFormat::Markdown,
                Some("--mermaid") => ExplainFormat::Mermaid,
                Some("--open") => ExplainFormat::OpenInCmux,
                Some(other) => bail!("unknown option {other:?} for `ahu explain`."),
            };
            if format != ExplainFormat::Terminal {
                expect_no_more(&args[2..])?;
            }
            Ok(Command::Explain { format })
        }
        "init" => {
            expect_no_more(&args[1..])?;
            Ok(Command::Init)
        }
        "agents" => {
            expect_no_more(&args[1..])?;
            Ok(Command::Agents)
        }
        "tasks" => {
            expect_no_more(&args[1..])?;
            Ok(Command::Tasks)
        }
        "doctor" => {
            expect_no_more(&args[1..])?;
            Ok(Command::Doctor)
        }
        "codex" => {
            expect_no_more(&args[1..])?;
            Ok(Command::Codex)
        }
        "task" | "diff" => {
            let task_id = args
                .get(1)
                .filter(|id| !id.is_empty() && !id.starts_with('-'))
                .cloned()
                .ok_or_else(|| {
                    crate::util::Error::new(format!("`ahu {first}` needs a task id."))
                })?;
            let output_json = first == "task"
                && args.get(2).map(String::as_str) == Some("--output")
                && args.get(3).map(String::as_str) == Some("json");
            expect_no_more(&args[if output_json { 4 } else { 2 }..])?;
            if first == "task" {
                Ok(Command::Task {
                    task_id,
                    output_json,
                })
            } else {
                Ok(Command::Diff { task_id })
            }
        }
        "focus" => {
            let task_id = args
                .get(1)
                .cloned()
                .ok_or_else(|| crate::util::Error::new("`ahu focus` needs a task id."))?;
            expect_no_more(&args[2..])?;
            Ok(Command::Focus { task_id })
        }
        "inventory" => Ok(Command::Inventory {
            agent: optional_agent(&args[1..])?,
        }),
        "hygiene" => Ok(Command::Hygiene {
            agent: optional_agent(&args[1..])?,
        }),
        "launch" => parse_launch(&args[1..], stdin_available),
        "onboard" => parse_onboard(&args[1..]),
        "run-task" => parse_run_task(&args[1..]),
        "--no-focus" => {
            expect_no_more(&args[1..])?;
            Ok(Command::Interactive { focus: false })
        }
        other => bail!("unknown command {other:?}.\n\nRun 'ahu help' for usage."),
    }
}

fn expect_no_more(rest: &[String]) -> Result<()> {
    if let Some(extra) = rest.first() {
        bail!("unexpected argument {extra:?}.\n\nRun 'ahu help' for usage.");
    }
    Ok(())
}

fn optional_agent(rest: &[String]) -> Result<Option<String>> {
    let Some(first) = rest.first() else {
        return Ok(None);
    };
    expect_no_more(&rest[1..])?;
    let name = first.strip_prefix('@').unwrap_or(first);
    if name.is_empty() {
        bail!("`@` on its own is not an agent name.");
    }
    Ok(Some(name.to_string()))
}

fn parse_onboard(rest: &[String]) -> Result<Command> {
    let mut register = None;
    let mut remove = None;
    let mut model = None;
    let mut version = "0.1.0".to_string();
    let mut index = 0;
    while index < rest.len() {
        match rest[index].as_str() {
            "--register" => {
                register = Some(value_for("--register", rest, &mut index)?);
            }
            "--remove" => {
                remove = Some(value_for("--remove", rest, &mut index)?);
            }
            "--model" => {
                model = Some(value_for("--model", rest, &mut index)?);
            }
            "--agent-version" => {
                version = value_for("--agent-version", rest, &mut index)?;
            }
            other => bail!("unknown option {other:?} for `ahu onboard`."),
        }
        index += 1;
    }
    if register.is_some() && remove.is_some() {
        bail!("`--register` and `--remove` cannot be combined.");
    }
    Ok(Command::Onboard {
        register,
        remove,
        model,
        version,
    })
}

fn parse_run_task(rest: &[String]) -> Result<Command> {
    let mut task_dir = None;
    let mut index = 0;
    while index < rest.len() {
        match rest[index].as_str() {
            "--task-dir" => {
                task_dir = Some(PathBuf::from(value_for("--task-dir", rest, &mut index)?));
            }
            other => bail!("unknown option {other:?} for `ahu run-task`."),
        }
        index += 1;
    }
    let task_dir = task_dir
        .ok_or_else(|| crate::util::Error::new("`ahu run-task` needs --task-dir <path>."))?;
    Ok(Command::RunTask { task_dir })
}

fn value_for(flag: &str, rest: &[String], index: &mut usize) -> Result<String> {
    *index += 1;
    rest.get(*index)
        .cloned()
        .ok_or_else(|| crate::util::Error::new(format!("{flag} needs a value.")))
}

fn parse_launch(rest: &[String], stdin_available: bool) -> Result<Command> {
    let name = rest
        .first()
        .ok_or_else(|| crate::util::Error::new("ahu launch needs @agent --prompt-file <path>."))?;
    let agent = name.strip_prefix('@').unwrap_or(name);
    if agent.is_empty()
        || !agent
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        || agent.starts_with('-')
    {
        bail!("invalid agent name {agent:?}.");
    }
    let mut prompt_file = None;
    let mut prompt_inline = None;
    let mut output_json = false;
    let mut dry_run = false;
    let mut allow_widened_approvals = false;
    let mut index = 1;
    while index < rest.len() {
        match rest[index].as_str() {
            "--prompt-file" if prompt_file.is_none() => {
                prompt_file = Some(PathBuf::from(value_for("--prompt-file", rest, &mut index)?));
            }
            "--prompt" if prompt_inline.is_none() => {
                prompt_inline = Some(value_for("--prompt", rest, &mut index)?);
            }
            "--output" if !output_json => {
                let value = value_for("--output", rest, &mut index)?;
                if value != "json" {
                    bail!("unsupported --output {value:?}; expected json.");
                }
                output_json = true;
            }
            "--dry-run" if !dry_run => dry_run = true,
            "--allow-widened-approvals" if !allow_widened_approvals => {
                allow_widened_approvals = true;
            }
            other => bail!("unknown or repeated option {other:?} for ahu launch."),
        }
        index += 1;
    }
    let prompt = match (prompt_file, prompt_inline) {
        (Some(_), Some(_)) => {
            bail!("--prompt and --prompt-file conflict; supply exactly one prompt source.")
        }
        (Some(path), None) => PromptSource::File(path),
        (None, Some(text)) => PromptSource::Inline(text),
        (None, None) if stdin_available => PromptSource::Stdin,
        (None, None) => bail!("ahu launch needs --prompt, --prompt-file, or piped stdin."),
    };
    if output_json && !dry_run {
        bail!("--output json requires --dry-run.");
    }
    Ok(Command::Launch {
        agent: agent.to_string(),
        prompt,
        output_json,
        dry_run,
        allow_widened_approvals,
    })
}

/// Remove global color options while leaving command option values intact.
/// Keeping value-taking options together prevents an inline prompt that happens
/// to say `--color=always` from being interpreted as application configuration.
pub fn extract_color(
    args: Vec<String>,
) -> Result<(Vec<String>, Option<crate::style::ColorChoice>)> {
    use crate::style::ColorChoice;
    let mut remaining = Vec::new();
    let mut choice = None;
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        let value = if arg == "--color" {
            Some(args.next().ok_or_else(|| {
                crate::util::Error::new("--color needs auto, always, or never.")
                    .with_kind(crate::util::ErrorKind::Usage)
            })?)
        } else {
            arg.strip_prefix("--color=").map(str::to_string)
        };
        if let Some(value) = value {
            if choice.is_some() {
                bail!(kind: crate::util::ErrorKind::Usage, "--color may only be supplied once.");
            }
            choice = Some(match value.as_str() {
                "auto" => ColorChoice::Auto,
                "always" => ColorChoice::Always,
                "never" => ColorChoice::Never,
                _ => {
                    bail!(kind: crate::util::ErrorKind::Usage, "invalid --color {value:?}; expected auto, always, or never.")
                }
            });
        } else {
            let takes_value = matches!(
                arg.as_str(),
                "--prompt"
                    | "--prompt-file"
                    | "--output"
                    | "--register"
                    | "--remove"
                    | "--model"
                    | "--agent-version"
                    | "--task-dir"
            );
            remaining.push(arg);
            if takes_value && let Some(value) = args.next() {
                remaining.push(value);
            }
        }
    }
    Ok((remaining, choice))
}

#[cfg(test)]
mod color_tests {
    use super::*;

    #[test]
    fn color_is_global_and_command_values_stay_literal() {
        for flag in ["--color=auto", "--color=always", "--color=never"] {
            let (args, choice) = extract_color(vec!["agents".into(), flag.into()]).unwrap();
            assert_eq!(args, ["agents"]);
            assert!(choice.is_some());
        }
        let (args, choice) = extract_color(vec![
            "--color".into(),
            "never".into(),
            "launch".into(),
            "@fixture".into(),
            "--prompt".into(),
            "--color=always".into(),
        ])
        .unwrap();
        assert_eq!(choice, Some(crate::style::ColorChoice::Never));
        assert_eq!(args, ["launch", "@fixture", "--prompt", "--color=always"]);
        for args in [
            vec!["--color"],
            vec!["--color="],
            vec!["--color=invalid"],
            vec!["--color=always", "--color=never"],
        ] {
            assert!(extract_color(args.into_iter().map(str::to_string).collect()).is_err());
        }
    }
    #[test]
    fn color_tokens_used_as_prompt_and_path_values_are_not_flags() {
        for option in ["--prompt", "--prompt-file"] {
            for value in ["--color", "--color=always"] {
                let (args, choice) = extract_color(vec![
                    "launch".into(),
                    "@fixture".into(),
                    option.into(),
                    value.into(),
                    "--color=never".into(),
                ])
                .unwrap();
                assert_eq!(choice, Some(crate::style::ColorChoice::Never));
                assert_eq!(args, ["launch", "@fixture", option, value]);
            }
        }
    }
}
