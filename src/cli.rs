//! Argument parsing and help text.

use std::path::PathBuf;

use crate::bail;
use crate::util::Result;

pub const HELP: &str = "ahu - The moai-agent command-line interface

Launch repository-defined agents in fresh Git worktrees and organise their
interactive sessions in cmux. Use --headless for unattended execution with
external results and optional detached supervision.

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
manifest asks for.

Commands:
  help                  Print this help message
  explain               Architecture overview and Mermaid diagrams
  init                  Record this project's agreed harness and model order
  launch @name [--prompt <text> | --prompt-file <path>] [--dry-run]
                        Assign work in a separate cmux session. Reads no
                        confirmation, so approval widening needs an explicit flag
                        --title <text> and --summary <text> set plain sidebar text.
                        With --title alone, the description also uses that title.
  agents                List the agents registered for this repository
  onboard               Preview native agent definitions that could be registered
  inventory [@agent]    Inspect visible context sources and coverage gaps
  hygiene [@agent]      Run the context hygiene review now
  knowledge lint [--output json]
                        Check the OKF bundles named in [knowledge] with okf.
                        Reads only; nothing is fetched, indexed, or rewritten
  tasks                 List tasks launched from this repository
  task <task-id> [--output json]
                        Inspect a task's recorded session state and locations
  diff <task-id>         Review tracked changes since launch; list untracked files
  wait <task-id> [--output json]    Wait for a headless attempt to stop
  result <task-id> [--output json]  Read durable process and harness outcomes
  cleanup <task-id>                Remove captured logs after a recorded terminal result;
                                  retain results, native sessions, branches and worktrees
  cancel <task-id>                Request cancellation of the task and ahu descendants;
                                  an interactive task is stopped and its cmux workspace closed,
                                  while the worktree, branch and record are kept
  resume <task-id> --prompt-file PATH [--output json]
                                  Resume a root task from the host using its recorded native session;
                                  child/worker resume unsupported: submit a new registered assignment
  focus <task-id>       Bring a task's cmux session to the front
  remove <task-id>      Remove a terminal task's record, worktree, and branch
  message <task-id> <text>
                        Append an operator message to the task's inbox
  doctor                Check repository, configuration, harness, and cmux
  agy                   Open the Antigravity CLI here using its configured model
                        and permissions (ahu passes no --mode and no
                        --dangerously-skip-permissions)
  claude                Open Claude here with permission checks bypassed
                        (uses Claude's configured model)
  codex                 Open Codex here with approval prompts and sandbox bypassed
                        (uses Codex's configured model)
  opencode              Open OpenCode here using its configured model and
                        permissions (ahu passes no --auto and no --pure)
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

knowledge lint options:
  --output json         Emit a versioned JSON report on stdout, diagnostics on
                        stderr. Bundles come from [knowledge] in the project
                        configuration; knowledge.fail_on_warnings decides whether
                        warnings fail the check. Errors always do.

launcher options:
  --no-focus            Do not switch to the new session after launching

launch options:
  --prompt <text>       Use an inline prompt (conflicts with --prompt-file)
  --prompt-file <path>  Read a UTF-8 prompt file
                        With neither option, read non-terminal stdin to EOF.
                        Explicit sources take precedence over unread stdin.
  --output json        Emit a versioned plan or headless launch/result envelope.
                        JSON goes to stdout, diagnostics to stderr.
  --headless            Run without cmux; descendants inherit this backend
  --allow-child @name   Grant this exact registered child identity (repeatable)
  --allow-child-widened @name
                        Grant a child whose manifest widens approvals. Frozen at
                        host submission; child requests cannot expand the grant
  --background          Detach a headless supervisor after startup acknowledgement
  --timeout <seconds>   Bound a headless attempt (default 1800)
  --native-helpers <policy>
                        disabled (default), or bounded: Claude 2.1.270 ONLY.
                        Bounded confines the entire parent and helpers to read-only
                        model tools. No shell, edits, builds or ahu child launches.
                        Settings-defined hook side effects remain unverified. Budget
                        $5 per attempt; one concurrent helper, depth one, same model.
                        Roles are requested; total helper count is not capped.
                        Agent native_helpers or project [execution].native_helpers
                        supplies the default; this flag overrides it.
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
        display: crate::launch::DisplayMetadata,
        output_json: bool,
        dry_run: bool,
        /// Opt in to launching an agent whose manifest widens the harness's own
        /// approval boundary. Required on this path because it has no
        /// interactive confirmation.
        allow_widened_approvals: bool,
    },
    HeadlessLaunch {
        launch: Box<Command>,
        options: crate::headless::Options,
    },
    BatchControl {
        action: String,
        task_id: String,
        prompt: Option<PathBuf>,
        json: bool,
    },
    BatchSupervisor {
        task_dir: PathBuf,
    },
    TasksJson,
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
    KnowledgeLint {
        output_json: bool,
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
    Remove {
        task_id: String,
    },
    Message {
        task_id: String,
        text: String,
    },
    Doctor,
    Codex,
    Claude,
    OpenCode,
    Antigravity,
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
        "supervise" => {
            if args.len() != 3 || args[1] != "--task-dir" {
                bail!("supervise requires --task-dir PATH");
            }
            Ok(Command::BatchSupervisor {
                task_dir: PathBuf::from(&args[2]),
            })
        }
        "wait" | "result" | "cancel" | "resume" | "cleanup" => {
            let task_id = args
                .get(1)
                .filter(|s| !s.starts_with('-'))
                .cloned()
                .ok_or_else(|| crate::util::Error::new("task id required"))?;
            let mut prompt = None;
            let mut json = false;
            let mut index = 2;
            while index < args.len() {
                match args[index].as_str() {
                    "--output" if !json => {
                        if value_for("--output", &args, &mut index)? != "json" {
                            bail!("expected json");
                        }
                        json = true;
                    }
                    "--prompt-file" if first == "resume" && prompt.is_none() => {
                        prompt = Some(PathBuf::from(value_for(
                            "--prompt-file",
                            &args,
                            &mut index,
                        )?));
                    }
                    other => bail!("unexpected option {other:?}"),
                }
                index += 1;
            }
            if first == "resume" && prompt.is_none() {
                bail!("resume requires --prompt-file PATH");
            }
            Ok(Command::BatchControl {
                action: first.to_string(),
                task_id,
                prompt,
                json,
            })
        }
        "tasks" if args.get(1).map(String::as_str) == Some("--output") => {
            if args.len() != 3 || args[2] != "json" {
                bail!("expected tasks --output json");
            }
            Ok(Command::TasksJson)
        }
        "tasks" => {
            expect_no_more(&args[1..])?;
            Ok(Command::Tasks)
        }
        "doctor" => {
            expect_no_more(&args[1..])?;
            Ok(Command::Doctor)
        }
        "claude" => {
            expect_no_more(&args[1..])?;
            Ok(Command::Claude)
        }
        "codex" => {
            expect_no_more(&args[1..])?;
            Ok(Command::Codex)
        }
        "opencode" => {
            expect_no_more(&args[1..])?;
            Ok(Command::OpenCode)
        }
        "agy" => {
            expect_no_more(&args[1..])?;
            Ok(Command::Antigravity)
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
        "remove" => {
            let task_id = args
                .get(1)
                .filter(|id| !id.is_empty() && !id.starts_with('-'))
                .cloned()
                .ok_or_else(|| crate::util::Error::new("`ahu remove` needs a task id."))?;
            expect_no_more(&args[2..])?;
            Ok(Command::Remove { task_id })
        }
        "message" => {
            let task_id = args
                .get(1)
                .filter(|id| !id.is_empty() && !id.starts_with('-'))
                .cloned()
                .ok_or_else(|| {
                    crate::util::Error::new("`ahu message` needs a task id and a message text.")
                })?;
            let text = args[2..].join(" ");
            Ok(Command::Message { task_id, text })
        }
        "inventory" => Ok(Command::Inventory {
            agent: optional_agent(&args[1..])?,
        }),
        "hygiene" => Ok(Command::Hygiene {
            agent: optional_agent(&args[1..])?,
        }),
        "knowledge" => parse_knowledge(&args[1..]),
        "launch" => parse_launch_backend(&args[1..], stdin_available),
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

/// `knowledge` takes a subcommand so later knowledge operations do not have to
/// change the shape of this one.
fn parse_knowledge(rest: &[String]) -> Result<Command> {
    match rest.first().map(String::as_str) {
        None => bail!("`ahu knowledge` needs a subcommand; the only one is `lint`."),
        Some("lint") => {}
        Some(other) => bail!("unknown subcommand {other:?} for `ahu knowledge`; expected `lint`."),
    }
    let mut output_json = false;
    let mut index = 1;
    while index < rest.len() {
        match rest[index].as_str() {
            "--output" if !output_json => {
                let value = value_for("--output", rest, &mut index)?;
                if value != "json" {
                    bail!("unsupported --output {value:?}; expected json.");
                }
                output_json = true;
            }
            other => bail!("unknown or repeated option {other:?} for `ahu knowledge lint`."),
        }
        index += 1;
    }
    Ok(Command::KnowledgeLint { output_json })
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

fn parse_launch_backend(rest: &[String], stdin_available: bool) -> Result<Command> {
    // Only an in-process broker dispatch implicitly selects the headless
    // profile; the broker always dispatches children with explicit --headless.
    let inherited = std::env::var_os("AHU_BROKER_DISPATCH").is_some();
    let mut options = crate::headless::Options::default();
    let mut headless = inherited;
    let mut filtered = Vec::new();
    let mut requested_dry_run = false;
    let mut batch_flags = std::collections::BTreeSet::new();
    let mut i = 0;
    while i < rest.len() {
        let flag = rest[i].as_str();
        if matches!(
            flag,
            "--headless" | "--background" | "--timeout" | "--native-helpers"
        ) && !batch_flags.insert(flag)
        {
            bail!("repeated batch option {flag}");
        }
        if flag == "--dry-run" {
            requested_dry_run = true;
        }
        match flag {
            "--headless" => headless = true,
            "--background" => options.background = true,
            "--timeout" => {
                options.timeout_seconds = value_for("--timeout", rest, &mut i)?
                    .parse()
                    .map_err(|_| crate::util::Error::new("--timeout needs seconds"))?;
                if options.timeout_seconds == 0 {
                    bail!("timeout must be positive");
                }
            }
            "--allow-child" | "--allow-child-widened" => {
                let name = value_for(flag, rest, &mut i)?;
                let name = name.strip_prefix('@').unwrap_or(&name).to_string();
                if !crate::util::is_safe_name(&name) {
                    bail!("invalid child agent name");
                }
                if flag == "--allow-child" {
                    options.child_agents.push(name);
                } else {
                    options.child_widened.push(name);
                }
            }
            "--native-helpers" => {
                options.native_helpers_explicit = true;
                options.native_helpers = value_for("--native-helpers", rest, &mut i)?;
                if !matches!(options.native_helpers.as_str(), "disabled" | "bounded") {
                    bail!("native helpers must be disabled or bounded");
                }
            }
            value => {
                filtered.push(value.to_string());
                if matches!(
                    value,
                    "--prompt" | "--prompt-file" | "--title" | "--summary" | "--output"
                ) {
                    filtered.push(value_for(value, rest, &mut i)?);
                }
            }
        }
        i += 1;
    }
    if !headless {
        if options != crate::headless::Options::default() {
            bail!("batch options require --headless");
        }
        return parse_launch(&filtered, stdin_available);
    }
    // Let the existing parser enforce prompt exclusivity and all shared flags.
    if !requested_dry_run {
        filtered.push("--dry-run".into());
    }
    let mut launch = parse_launch(&filtered, stdin_available)?;
    if let Command::Launch { dry_run, .. } = &mut launch {
        *dry_run = requested_dry_run;
    }
    Ok(Command::HeadlessLaunch {
        launch: Box::new(launch),
        options,
    })
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
    let mut display = crate::launch::DisplayMetadata::default();
    let mut prompt_file = None;
    let mut prompt_inline = None;
    let mut output_json = false;
    let mut dry_run = false;
    let mut allow_widened_approvals = false;
    let mut index = 1;
    while index < rest.len() {
        match rest[index].as_str() {
            "--title" if display.title.is_none() => {
                display.title = Some(value_for("--title", rest, &mut index)?);
            }
            "--summary" if display.summary.is_none() => {
                display.summary = Some(value_for("--summary", rest, &mut index)?);
            }
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
        display,
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
                    | "--title"
                    | "--summary"
                    | "--prompt-file"
                    | "--output"
                    | "--register"
                    | "--remove"
                    | "--model"
                    | "--agent-version"
                    | "--task-dir"
                    | "--timeout"
                    | "--native-helpers"
                    | "--allow-child"
                    | "--allow-child-widened"
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
