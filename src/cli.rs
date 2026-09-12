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
and submit it. Pasting never submits by itself.

Commands:
  help                  Print this help message
  explain               Architecture overview and Mermaid diagrams
  init                  Record this project's agreed harness and model order
  launch @name --prompt-file <path> [--dry-run]
                        Assign work in a separate cmux session (no confirmation)
  agents                List the agents registered for this repository
  onboard               Preview native agent definitions that could be registered
  inventory [@agent]    Show everything that can influence an agent's context
  hygiene [@agent]      Run the context hygiene review now
  tasks                 List tasks launched from this repository
  focus <task-id>       Bring a task's cmux session to the front
  doctor                Check repository, configuration, harness, and cmux
  run-task              Internal: run a prepared task (used by cmux)

Options:
  -h, --help            Print this help message
  -V, --version         Print the version

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
  --dry-run             Show the preview and create nothing
  --allow-widened-approvals
                        Required to launch an agent whose manifest declares
                        permissions = auto or accept-edits. `ahu launch` reads no
                        confirmation, so widening is opt-in on the command line

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
        prompt_file: PathBuf,
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
    Focus {
        task_id: String,
    },
    Doctor,
    RunTask {
        task_dir: PathBuf,
    },
}

/// Parse `args`, which excludes the executable name.
pub fn parse<I, S>(args: I) -> Result<Command>
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    let args: Vec<String> = args.into_iter().map(Into::into).collect();
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
        "launch" => parse_launch(&args[1..]),
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

fn parse_launch(rest: &[String]) -> Result<Command> {
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
    let mut dry_run = false;
    let mut allow_widened_approvals = false;
    let mut index = 1;
    while index < rest.len() {
        match rest[index].as_str() {
            "--prompt-file" if prompt_file.is_none() => {
                prompt_file = Some(PathBuf::from(value_for("--prompt-file", rest, &mut index)?));
            }
            "--dry-run" if !dry_run => dry_run = true,
            "--allow-widened-approvals" if !allow_widened_approvals => {
                allow_widened_approvals = true;
            }
            other => bail!("unknown or repeated option {other:?} for ahu launch."),
        }
        index += 1;
    }
    let prompt_file = prompt_file
        .ok_or_else(|| crate::util::Error::new("ahu launch needs --prompt-file <path>."))?;
    Ok(Command::Launch {
        agent: agent.to_string(),
        prompt_file,
        dry_run,
        allow_widened_approvals,
    })
}
