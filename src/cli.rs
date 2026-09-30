//! Argument parsing and help text.

use std::path::PathBuf;

use crate::bail;
use crate::util::Result;

pub const HELP_ALL: &str = "ahu - The moai-agent command-line interface

Launch repository-defined agents in fresh Git worktrees and organise their
interactive sessions in cmux. Use --headless for unattended execution with
primary-owned coordination and optional detached supervision.

Usage: ahu [--repo <path>] [COMMAND]

Start work directly with a registered agent using `ahu @agent [prompt]`.
Use `ahu @agent` to open the interactive launcher with that agent selected.

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
  help [COMMAND|all]    Print command help or the full option reference
  explain               Architecture overview and Mermaid diagrams
  setup                 Configure ahu, harness MCP access, skills, and dev agents
  @agent [prompt]       Assign work to an agent; quote multi-word prompts.
                        Also accepts --prompt or --prompt-file.
  agents                List the agents registered for this repository
  onboard               Preview native agent definitions that could be registered
  lock [--update]       Check committed agent context, or refresh ahu.lock before committing it
  knowledge lint [--output json]
                        Check the OKF bundles named in [knowledge] with okf.
                        Reads only; nothing is fetched, indexed, or rewritten
  eval run (--case <path> | --suite <path>) --agent @name [--agent @other]
           --records <path> [options]
                        Run candidates (and optionally a blind agent evaluator)
                        over an external evaluation case or suite
  eval report --records <path> [--output json]
                        Compare local evaluation runs from an external JSONL
                        record file. Reads only, and only outside this checkout
  tasks [--limit N|--all]
                        List recent tasks or the full history
  task <task-id> [--output json]
                        Inspect a task's recorded session state and locations
  wait <task-id> [--output json]    Wait for a headless attempt to stop
  result <task-id> [--output json]  Read durable process and harness outcomes
  cleanup <task-id>                After known termination, remove recognized old captures
                                  and bounded requests; retain results and native sessions,
                                  branches and worktrees
  cancel <task-id>                Request cancellation of the task and ahu descendants;
                                  confirmed interactive cancellation closes its cmux workspace,
                                  while the worktree, branch and record are kept
  resume <task-id> --prompt-file PATH [--output json]
                                  Resume a root task from the host using its recorded native session;
                                  child/worker resume unsupported: submit a new registered assignment
  focus <task-id>       Bring a task's cmux session to the front
  remove <task-id>      Remove a terminal task's record, worktree, and branch
  message <task-id> <text>
                        Append an operator message; subsequent flags are literal text
  cmux status [--output json]
                        Inspect native integration evidence and headless isolation
  cmux install --harness ID [--dry-run]
                        Preview or explicitly delegate a native cmux installation
  mcp serve              Serve repository-scoped ahu tools and typed decisions over stdio MCP
  doctor [--verbose]    Check repository, configuration, harness, and cmux
  agy                   Open the Antigravity CLI here on this project's
                        top-ranked Antigravity model, in YOLO mode
                        (--dangerously-skip-permissions)
  claude                Open Claude here on this project's top-ranked
                        Claude Code model, with permission checks bypassed
                        (--dangerously-skip-permissions)
  codex                 Open Codex here on this project's top-ranked Codex
                        model, with approval prompts and the sandbox bypassed
                        (--dangerously-bypass-approvals-and-sandbox)
  opencode              Open OpenCode here on this project's top-ranked OpenCode
                        model, auto-approving every permission it does not deny
                        (--auto; ahu passes no --pure)
  run-task              Internal: run a prepared task (used by cmux)
  supervise --task-dir PATH
                        Internal: supervise a detached headless task

Task references:
  Use ahu:task:<id> to identify a task explicitly. Bare IDs and unique ID
  prefixes remain accepted. Exact @name handles select tasks in this repository.
  In `ahu @name`, @name selects a registered agent instead.

Options:
  help [COMMAND]        Print focused help for a command; use `help all` for this reference
  -h, --help            Print this help message
  -V, --version         Print the version
  --repo <path>         Select a repository checkout before the command.
                        Also accepts --repo=<path>; paths in the command are
                        relative to this checkout. No environment override.
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

eval report options:
  --records <path>      JSONL run records written with schema version 2.
                        Required, and refused when it resolves inside this
                        repository: run evidence stays in a user-owned directory
  --output json         Emit a versioned JSON comparison on stdout, the readable
                        report on stderr. Rows are grouped by case and corpus
                        version, stage, agent/version, evaluator/version, model,
                        harness/version, every input fingerprint, and skill
                        digest. Binary answer and tool-expectation pass rates
                        carry 95% Wilson intervals. Only schema version 2
                        records are accepted. Token,
                        timing, and decision-call figures are reported as
                        coverage counts, so a missing observation is not a zero.
                        `eval report` reads records; it runs no candidate and no
                        evaluator

eval run options:
  --skill-selection <none|lexical|decision>
                        Optional prelaunch skill advice (default none). Decision
                        sends candidate-visible task and committed skill descriptions
                        to the configured decision provider. Does not remove skills.
  --case <path>         OKF Markdown case with YAML front matter; expected
                        answer values, rubric, and tool expectations stay hidden
                        from the candidate. Schema 2 prompts tool-neutrally;
                        candidates receive a tool-neutral prompt
  --suite <path>        OKF Markdown suite (type ahu:eval-suite) naming cases by
                        relative path with fixed weights. An alternative to
                        --case. The whole case x agent x run matrix is validated
                        and capped before any model launches
  --agent @name         Registered candidate agent (required, repeatable). The
                        order given is the order the trials run in
  --decision-evaluator  Send case evidence, candidate output and rubric to the
                        configured typed decision provider for grading. Opt-in;
                        conflicts with --evaluator and --evaluator-repo
  --evaluator @name     Optional separate registered evaluator agent
  --evaluator-repo <path>
                        Run the evaluator from a separately prepared checkout,
                        recorded as blinding `isolated`. Without it the
                        evaluator is blinded at the prompt level only
                        (`prompt_only`), which is not environment isolation
  --records <path>      External JSONL destination; prompts and artifacts are
                        written to a private sibling run directory
  --runs <count>        Repetitions from 1 to 100 (default 1)
  --timeout <seconds>   Per-agent headless timeout from 1 to 86400 (default 1800)
  --allow-widened-approvals
                        Explicitly authorize a candidate or evaluator manifest
                        that widens harness approvals
  --output json         Emit a versioned summary to stdout

tasks options:
  --limit <count>       Show the most recent 1 to 500 tasks (default 20)
  --all                 Show the full task history
  --output json         Emit the complete task list as JSON; cannot be combined with --limit/--all

doctor options:
  --verbose             Show component-level cmux, hook, drift, and context-lock details

agent assignment options:
  --no-focus            Do not switch to the new session after launching

agent assignment options:
  --name <name>        Reserve an immutable @name for this task in the repository.
                        By default, generate a short name from the displayed title.
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
                        permissions = auto or accept-edits. `ahu @agent` reads no
                        confirmation, so widening is opt-in on the command line

Exit codes:
  0 success; 1 cancelled; 2 usage error; 3 unknown agent;
  4 missing prerequisite; 5 run failure.

run-task options:
  --task-dir <path>     Directory holding the prepared task record";

pub const HELP: &str = "ahu — launch and coordinate repository agents

Usage: ahu [--repo PATH] [COMMAND]

Start the launcher with `ahu`, or run a registered agent with
`ahu @agent 'task prompt'`.

Commands:
  setup                 Configure this project for ahu
  agents                List registered agents
  onboard               Preview native agent definitions
  doctor [--verbose]    Check project and harness readiness
  lock [--update]       Check or refresh committed agent context
  tasks [--limit N]     List recent tasks; use --all for the full list
  task ID               Inspect a task
  wait|result|cancel ID Control or inspect a task
  resume ID --prompt-file PATH
  cleanup|remove ID     Clean captures or remove a completed task
  focus|message ID ...  Focus a task or send it a message
  eval run|report       Run and compare local agent evaluations
  knowledge lint        Check configured OKF bundles
  cmux status|install   Inspect or install native cmux integration
  mcp serve             Serve repository tools over stdio MCP
  explain               Show the architecture overview
  @agent [PROMPT]       Assign work to a registered agent
  agy|claude|codex|opencode
                        Open a coordinating harness session

Use `ahu help COMMAND` or `ahu COMMAND --help` for focused help.
Use `ahu help all` for the full command and option reference.

Global options: --repo PATH, --color auto|always|never, --help, --version
Docs: docs/reference.md";

/// Help for a command or a command group. Detailed flag sections are sliced
/// from the complete help text so there is one source of truth.
pub fn help_for(topic: Option<&str>) -> Result<String> {
    let Some(topic) = topic else {
        return Ok(HELP.to_string());
    };
    let topic = topic.trim().trim_start_matches("ahu ");
    if matches!(topic, "--help" | "-h") {
        return Ok(HELP.to_string());
    }
    if matches!(topic, "all" | "--all") {
        return Ok(HELP_ALL.to_string());
    }
    let help = match topic {
        "setup" => "Usage: ahu setup\n\nDetect installed harnesses, select a model for each ahu dev agent, install user-facing skills, configure project MCP access, and refresh ahu.lock. Existing project files are preserved; review and commit setup output before launching.\n".to_string(),
        "tasks" => "Usage: ahu tasks [--limit N | --all] [--output json]\n\nLists recent tasks (default limit: 20). Use --all to show the full history. --output json emits the complete task list for scripting.\n".to_string(),
        "eval run" => help_section("eval run options:", "agent assignment options:"),
        "eval report" => help_section("eval report options:", "eval run options:"),
        "@agent" => help_section("agent assignment options:", "Exit codes:"),
        "explain" => help_section("explain options:", "onboard options:"),
        "onboard" => help_section("onboard options:", "knowledge lint options:"),
        "knowledge lint" => help_section("knowledge lint options:", "eval report options:"),
        "doctor" => "Usage: ahu doctor\n\nSummarize project, context-lock, skills, harness, telemetry, and cmux readiness. Use --verbose for component-level diagnostics.\n".to_string(),
        "lock" => "Usage: ahu lock [--update]\n\nChecks that recognized agent context matches committed ahu.lock. --update refreshes the lock for review and commit.\n".to_string(),
        "cmux status" => "Usage: ahu cmux status [--output json]\n\nInspect native integration evidence and headless isolation.\n".to_string(),
        "cmux install" => "Usage: ahu cmux install --harness ID [--dry-run]\n\nPreview or delegate a native cmux installation.\n".to_string(),
        "mcp serve" => "Usage: ahu mcp serve\n\nServe repository-scoped agent/task inspection and optional typed decisions over stdio MCP.\n".to_string(),
        "task" => "Usage: ahu task ID [--output json]\n\nInspect a task's state, branch, worktree, and launch evidence.\n".to_string(),
        "wait" => "Usage: ahu wait TASK [--output json]\n\nWait for a headless task to reach a terminal state.\n".to_string(),
        "result" => "Usage: ahu result TASK [--output json]\n\nRead the durable process and harness outcomes for a headless task.\n".to_string(),
        "cancel" => "Usage: ahu cancel TASK\n\nRequest cancellation of the task and its ahu descendants.\n".to_string(),
        "resume" => "Usage: ahu resume TASK --prompt-file PATH [--output json]\n\nResume a root headless task using its recorded native session.\n".to_string(),
        "cleanup" => "Usage: ahu cleanup TASK\n\nRemove recognized captures and bounded requests after termination is known.\n".to_string(),
        "remove" => "Usage: ahu remove TASK\n\nRemove a completed task's record, worktree, and branch when safe.\n".to_string(),
        "focus" => "Usage: ahu focus TASK\n\nBring an interactive task's cmux session to the front.\n".to_string(),
        "message" => "Usage: ahu message TASK TEXT\n\nAppend an operator message for the task. The remaining arguments are literal text.\n".to_string(),
        "run-task" => "Usage: ahu run-task --task-dir PATH\n\nInternal worker command started by ahu's task supervisor.\n".to_string(),
        "supervise" => "Usage: ahu supervise --task-dir PATH\n\nInternal supervisor command for detached headless tasks.\n".to_string(),
        "agents" | "eval" | "knowledge" | "cmux" | "mcp" | "agy" | "claude" | "codex" | "opencode" | "help" => return Ok(format!("{}\n\nRun `ahu help all` for detailed options.\n", help_line_for(topic).unwrap_or("Unknown command"))),
        other => return Err(crate::util::Error::new(format!("unknown help topic {other:?}; run `ahu help` for commands."))),
    };
    Ok(help)
}

fn help_section(start: &str, end: &str) -> String {
    let lines: Vec<_> = HELP_ALL.lines().collect();
    let Some(first) = lines.iter().position(|line| line.starts_with(start)) else {
        return String::new();
    };
    let last = lines[first + 1..]
        .iter()
        .position(|line| line.starts_with(end))
        .map(|offset| first + 1 + offset)
        .unwrap_or(lines.len());
    lines[first..last].join("\n") + "\n"
}

fn help_line_for(topic: &str) -> Option<&'static str> {
    Some(match topic {
        "agents" => "ahu agents — list registered agents",
        "setup" => "ahu setup — configure this project",
        "tasks" => "ahu tasks — list recent tasks",
        "doctor" => "ahu doctor — check project readiness",
        "lock" => "ahu lock — check committed agent context",
        "eval" => "ahu eval — run or compare evaluations",
        "knowledge" => "ahu knowledge lint — validate configured knowledge bundles",
        "cmux" => "ahu cmux — inspect or install cmux integration",
        "mcp" => "ahu mcp serve — start the MCP server",
        "explain" => "ahu explain — show the architecture overview",
        "onboard" => "ahu onboard — preview native agent definitions",
        "agy" => "ahu agy — open Antigravity",
        "claude" => "ahu claude — open Claude Code",
        "codex" => "ahu codex — open Codex",
        "opencode" => "ahu opencode — open OpenCode",
        "@agent" => "ahu @agent — assign work to a registered agent",
        "help" => "ahu help — show command help",
        "run-task" => "ahu run-task — internal task worker",
        "supervise" => "ahu supervise — internal task supervisor",
        _ => return None,
    })
}

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
    Help {
        topic: Option<String>,
    },
    Version,
    Explain {
        format: ExplainFormat,
    },
    Interactive {
        focus: bool,
        agent: Option<String>,
    },
    Setup,
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
    Lock {
        update: bool,
    },
    KnowledgeLint {
        output_json: bool,
    },
    EvalReport {
        records: PathBuf,
        output_json: bool,
    },
    EvalRun {
        /// A single case. Exactly one of `case` and `suite` is set.
        case: Option<PathBuf>,
        suite: Option<PathBuf>,
        /// Candidate agents in the order named, which is the execution order.
        agents: Vec<String>,
        evaluator: Option<String>,
        /// A separately prepared checkout the evaluator runs from.
        evaluator_repo: Option<PathBuf>,
        decision_evaluator: bool,
        skill_selection: crate::skill_selection::Mode,
        records: PathBuf,
        runs: u32,
        timeout_seconds: u64,
        allow_widened_approvals: bool,
        output_json: bool,
    },
    Tasks {
        limit: Option<usize>,
    },
    Task {
        task_id: String,
        output_json: bool,
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
    CmuxStatus {
        output_json: bool,
    },
    CmuxInstall {
        harness: String,
        dry_run: bool,
    },
    McpServe,
    Doctor {
        verbose: bool,
    },
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
                        "ahu @agent needs --prompt or --prompt-file when stdin is a terminal.",
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
        return Ok(Command::Interactive {
            focus: true,
            agent: None,
        });
    };
    if let Some(help) = help_request(&args)? {
        return Ok(help);
    }
    match first {
        "help" | "-h" | "--help" => {
            unreachable!("help requests are handled before command parsing")
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
        "setup" => {
            expect_no_more(&args[1..])?;
            Ok(Command::Setup)
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
        "tasks" => {
            let mut limit = Some(20usize);
            let mut saw_limit = false;
            let mut output_json = false;
            let mut index = 1;
            while index < args.len() {
                match args[index].as_str() {
                    "--all" if limit == Some(20) && !saw_limit => limit = None,
                    "--limit" if !saw_limit && limit.is_some() => {
                        let value = value_for("--limit", &args, &mut index)?;
                        let parsed = value
                            .parse::<usize>()
                            .ok()
                            .filter(|n| (1..=500).contains(n));
                        limit = Some(parsed.ok_or_else(|| {
                            crate::util::Error::new("--limit must be between 1 and 500")
                        })?);
                        saw_limit = true;
                    }
                    "--output" if !output_json => {
                        if value_for("--output", &args, &mut index)? != "json" {
                            bail!("expected json");
                        }
                        output_json = true;
                    }
                    other => {
                        bail!("unexpected option {other:?} for `ahu tasks`; use `ahu help tasks`.")
                    }
                }
                index += 1;
            }
            if output_json {
                if saw_limit || limit.is_none() {
                    bail!(
                        "`ahu tasks --output json` returns the complete list; remove --all or --limit."
                    );
                }
                Ok(Command::TasksJson)
            } else {
                Ok(Command::Tasks { limit })
            }
        }
        "cmux" => parse_cmux(&args[1..]),
        "mcp" => match args.get(1).map(String::as_str) {
            Some("serve") if args.len() == 2 => Ok(Command::McpServe),
            _ => bail!("expected ahu mcp serve"),
        },
        "doctor" => match &args[1..] {
            [] => Ok(Command::Doctor { verbose: false }),
            [flag] if flag == "--verbose" => Ok(Command::Doctor { verbose: true }),
            _ => bail!("expected ahu doctor [--verbose]"),
        },
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
        "task" => {
            let task_id = args
                .get(1)
                .filter(|id| !id.is_empty() && !id.starts_with('-'))
                .cloned()
                .ok_or_else(|| crate::util::Error::new("`ahu task` needs a task id."))?;
            let output_json = args.get(2).map(String::as_str) == Some("--output")
                && args.get(3).map(String::as_str) == Some("json");
            expect_no_more(&args[if output_json { 4 } else { 2 }..])?;
            Ok(Command::Task {
                task_id,
                output_json,
            })
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
        "lock" => match args.get(1).map(String::as_str) {
            None => Ok(Command::Lock { update: false }),
            Some("--update") if args.len() == 2 => Ok(Command::Lock { update: true }),
            Some(other) => bail!("unknown option {other:?} for `ahu lock`."),
        },
        "knowledge" => parse_knowledge(&args[1..]),
        "eval" => parse_eval(&args[1..]),
        direct if direct.starts_with('@') => {
            if args.len() == 1 && !stdin_available {
                let agent = direct.strip_prefix('@').unwrap_or_default();
                validate_agent_name(agent)?;
                Ok(Command::Interactive {
                    focus: true,
                    agent: Some(agent.to_string()),
                })
            } else {
                parse_launch_backend(&args, stdin_available)
            }
        }
        "onboard" => parse_onboard(&args[1..]),
        "run-task" => parse_run_task(&args[1..]),
        "--no-focus" => {
            expect_no_more(&args[1..])?;
            Ok(Command::Interactive {
                focus: false,
                agent: None,
            })
        }
        "launch" => bail!(
            "`ahu launch` was removed. Use `ahu @agent [prompt]`; see `ahu help @agent` for options."
        ),
        other => bail!("unknown command {other:?}.\n\nRun 'ahu help' for usage."),
    }
}

fn help_request(args: &[String]) -> Result<Option<Command>> {
    if args.first().is_some_and(|arg| arg == "help") {
        if args.len() > 3 {
            bail!("usage: ahu help [COMMAND]");
        }
        return Ok(Some(Command::Help {
            topic: (args.len() > 1).then(|| args[1..].join(" ")),
        }));
    }
    if args
        .first()
        .is_some_and(|arg| matches!(arg.as_str(), "-h" | "--help"))
    {
        if args.len() == 1 {
            return Ok(Some(Command::Help { topic: None }));
        }
        bail!("use `ahu help COMMAND` for command help");
    }
    if args
        .last()
        .is_none_or(|last| last != "--help" && last != "-h")
        || args.first().is_some_and(|first| first == "message")
    {
        return Ok(None);
    }
    let before_help = &args[..args.len() - 1];
    if before_help.last().is_some_and(|flag| {
        matches!(
            flag.as_str(),
            "--prompt"
                | "--prompt-file"
                | "--records"
                | "--case"
                | "--suite"
                | "--agent"
                | "--evaluator"
                | "--evaluator-repo"
                | "--runs"
                | "--timeout"
                | "--name"
                | "--title"
                | "--summary"
                | "--model"
                | "--task-dir"
                | "--harness"
                | "--register"
                | "--remove"
                | "--agent-version"
                | "--output"
                | "--allow-child"
                | "--allow-child-widened"
                | "--native-helpers"
        )
    }) {
        return Ok(None);
    }
    let Some(first) = before_help.first() else {
        return Ok(Some(Command::Help { topic: None }));
    };
    let count = if matches!(first.as_str(), "eval" | "knowledge" | "cmux" | "mcp") {
        before_help.len().min(2)
    } else {
        1
    };
    Ok(Some(Command::Help {
        topic: Some(before_help[..count].join(" ")),
    }))
}

fn expect_no_more(rest: &[String]) -> Result<()> {
    if let Some(extra) = rest.first() {
        bail!("unexpected argument {extra:?}.\n\nRun 'ahu help' for usage.");
    }
    Ok(())
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

/// Parse eval orchestration and reporting commands.
fn parse_eval(rest: &[String]) -> Result<Command> {
    match rest.first().map(String::as_str) {
        None => bail!("`ahu eval` needs a subcommand: `run` or `report`."),
        Some("run") => return parse_eval_run(&rest[1..]),
        Some("report") => {}
        Some(other) => {
            bail!("unknown subcommand {other:?} for `ahu eval`; expected `run` or `report`.")
        }
    }
    let mut records = None;
    let mut output_json = false;
    let mut index = 1;
    while index < rest.len() {
        match rest[index].as_str() {
            "--records" if records.is_none() => {
                records = Some(PathBuf::from(value_for("--records", rest, &mut index)?));
            }
            "--output" if !output_json => {
                let value = value_for("--output", rest, &mut index)?;
                if value != "json" {
                    bail!("unsupported --output {value:?}; expected json.");
                }
                output_json = true;
            }
            other => bail!("unknown or repeated option {other:?} for `ahu eval report`."),
        }
        index += 1;
    }
    let records = records.ok_or_else(|| {
        crate::util::Error::new(
            "`ahu eval report` needs --records <path> naming a JSONL run record file outside this repository.",
        )
    })?;
    Ok(Command::EvalReport {
        records,
        output_json,
    })
}

fn parse_eval_run(rest: &[String]) -> Result<Command> {
    let mut case = None;
    let mut suite = None;
    let mut agents: Vec<String> = Vec::new();
    let mut evaluator = None;
    let mut evaluator_repo = None;
    let mut decision_evaluator = false;
    let mut skill_selection = None;
    let mut records = None;
    let mut runs = 1u32;
    let mut timeout_seconds = 1800u64;
    let mut saw_runs = false;
    let mut saw_timeout = false;
    let mut allow_widened_approvals = false;
    let mut output_json = false;
    let mut index = 0;
    while index < rest.len() {
        match rest[index].as_str() {
            "--case" if case.is_none() => {
                case = Some(PathBuf::from(value_for("--case", rest, &mut index)?))
            }
            "--suite" if suite.is_none() => {
                suite = Some(PathBuf::from(value_for("--suite", rest, &mut index)?))
            }
            // Repeatable: each occurrence adds a candidate, and the order is
            // kept because it is the order the trials run in.
            "--agent" => agents.push(value_for("--agent", rest, &mut index)?),
            "--decision-evaluator" if !decision_evaluator => decision_evaluator = true,
            "--evaluator" if evaluator.is_none() => {
                evaluator = Some(value_for("--evaluator", rest, &mut index)?)
            }
            "--evaluator-repo" if evaluator_repo.is_none() => {
                evaluator_repo = Some(PathBuf::from(value_for(
                    "--evaluator-repo",
                    rest,
                    &mut index,
                )?))
            }
            "--skill-selection" if skill_selection.is_none() => {
                skill_selection = Some(crate::skill_selection::Mode::parse(&value_for(
                    "--skill-selection",
                    rest,
                    &mut index,
                )?)?);
            }
            "--records" if records.is_none() => {
                records = Some(PathBuf::from(value_for("--records", rest, &mut index)?))
            }
            "--runs" if !saw_runs => {
                saw_runs = true;
                runs = value_for("--runs", rest, &mut index)?
                    .parse()
                    .map_err(|_| {
                        crate::util::Error::new("--runs must be an integer from 1 to 100")
                    })?;
                if !(1..=100).contains(&runs) {
                    bail!("--runs must be an integer from 1 to 100");
                }
            }
            "--timeout" if !saw_timeout => {
                saw_timeout = true;
                timeout_seconds =
                    value_for("--timeout", rest, &mut index)?
                        .parse()
                        .map_err(|_| {
                            crate::util::Error::new(
                                "--timeout must be an integer from 1 to 86400 seconds",
                            )
                        })?;
                if !(1..=86_400).contains(&timeout_seconds) {
                    bail!("--timeout must be an integer from 1 to 86400 seconds");
                }
            }
            "--allow-widened-approvals" if !allow_widened_approvals => {
                allow_widened_approvals = true
            }
            "--output" if !output_json => {
                let value = value_for("--output", rest, &mut index)?;
                if value != "json" {
                    bail!("unsupported --output {value:?}; expected json.");
                }
                output_json = true;
            }
            other => bail!("unknown or repeated option {other:?} for `ahu eval run`."),
        }
        index += 1;
    }
    match (&case, &suite) {
        (None, None) => bail!("`ahu eval run` needs --case <path> or --suite <path>."),
        (Some(_), Some(_)) => {
            bail!("--case and --suite are alternatives for `ahu eval run`; pass one.")
        }
        _ => {}
    }
    if agents.is_empty() {
        bail!("`ahu eval run` needs --agent @name; repeat it to compare candidates.");
    }
    if agents.len() > 16 {
        bail!("`ahu eval run` accepts at most 16 --agent candidates.");
    }
    for agent in &agents {
        if !agent.starts_with('@') || agent.len() < 2 {
            bail!("--agent must name a registered agent as @name");
        }
    }
    // Two spellings of the same agent are a duplicate, and the matrix would run
    // it twice and report it as two candidates.
    let mut seen = std::collections::BTreeSet::new();
    for agent in &agents {
        if !seen.insert(agent.trim_start_matches('@')) {
            bail!("--agent {agent} is named more than once.");
        }
    }
    if let Some(evaluator) = &evaluator
        && (!evaluator.starts_with('@') || evaluator.len() < 2)
    {
        bail!("--evaluator must name a registered agent as @name");
    }
    if decision_evaluator && (evaluator.is_some() || evaluator_repo.is_some()) {
        bail!("--decision-evaluator conflicts with --evaluator and --evaluator-repo");
    }
    if evaluator_repo.is_some() && evaluator.is_none() {
        bail!("--evaluator-repo needs --evaluator @name: it names where that evaluator runs from.");
    }
    let records = records.ok_or_else(|| {
        crate::util::Error::new("`ahu eval run` needs --records <external-jsonl-path>.")
    })?;
    Ok(Command::EvalRun {
        case,
        suite,
        agents,
        evaluator,
        evaluator_repo,
        decision_evaluator,
        skill_selection: skill_selection.unwrap_or_default(),
        records,
        runs,
        timeout_seconds,
        allow_widened_approvals,
        output_json,
    })
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
                    "--prompt" | "--prompt-file" | "--title" | "--summary" | "--name" | "--output"
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
        .ok_or_else(|| crate::util::Error::new("ahu @agent needs --prompt-file <path>."))?;
    let agent = name.strip_prefix('@').unwrap_or(name);
    validate_agent_name(agent)?;
    let mut display = crate::launch::DisplayMetadata::default();
    let mut prompt_file = None;
    let mut prompt_inline = None;
    let mut output_json = false;
    let mut dry_run = false;
    let mut allow_widened_approvals = false;
    let mut positional_prompt = Vec::new();
    let mut index = 1;
    while index < rest.len() {
        match rest[index].as_str() {
            "--name" if display.name.is_none() => {
                display.name = Some(crate::task_handles::name(&value_for(
                    "--name", rest, &mut index,
                )?)?);
            }
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
            value if !value.starts_with('-') => positional_prompt.push(value.to_string()),
            other => bail!("unknown or repeated option {other:?} for ahu @agent."),
        }
        index += 1;
    }
    if !positional_prompt.is_empty() && (prompt_file.is_some() || prompt_inline.is_some()) {
        bail!("a positional prompt cannot be combined with --prompt or --prompt-file.");
    }
    let prompt_inline = prompt_inline
        .or_else(|| (!positional_prompt.is_empty()).then(|| positional_prompt.join(" ")));
    let prompt = match (prompt_file, prompt_inline) {
        (Some(_), Some(_)) => {
            bail!("--prompt and --prompt-file conflict; supply exactly one prompt source.")
        }
        (Some(path), None) => PromptSource::File(path),
        (None, Some(text)) => PromptSource::Inline(text),
        (None, None) if stdin_available => PromptSource::Stdin,
        (None, None) => bail!("ahu @agent needs --prompt, --prompt-file, or piped stdin."),
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

fn validate_agent_name(agent: &str) -> Result<()> {
    if agent.is_empty()
        || !agent
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        || agent.starts_with('-')
    {
        bail!("invalid agent name {agent:?}.");
    }
    Ok(())
}

/// Read the repository selector only before the command, so literal prompts,
/// messages, and native arguments can never change the lookup scope.
pub fn extract_repository(args: Vec<String>) -> Result<(Vec<String>, Option<PathBuf>)> {
    let mut remaining = Vec::new();
    let mut repository = None;
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        let value = if arg == "--repo" {
            Some(args.next().ok_or_else(|| {
                crate::util::Error::new("--repo needs a checkout path.")
                    .with_kind(crate::util::ErrorKind::Usage)
            })?)
        } else {
            arg.strip_prefix("--repo=").map(str::to_string)
        };
        if let Some(value) = value {
            if value.is_empty() || value.starts_with("--") {
                bail!(kind: crate::util::ErrorKind::Usage, "--repo needs a checkout path; prefix a path beginning with '--' with './'.");
            }
            if repository.replace(PathBuf::from(value)).is_some() {
                bail!(kind: crate::util::ErrorKind::Usage, "--repo may only be supplied once.");
            }
        } else if arg == "--color" {
            remaining.push(arg);
            if let Some(value) = args.next() {
                remaining.push(value);
            }
        } else if arg.starts_with("--color=") {
            remaining.push(arg);
        } else {
            remaining.push(arg);
            remaining.extend(args);
            break;
        }
    }
    Ok((remaining, repository))
}

/// Remove global color options while leaving command option values and inbox
/// messages intact. A literal payload never changes application configuration.
pub fn extract_color(
    args: Vec<String>,
) -> Result<(Vec<String>, Option<crate::style::ColorChoice>)> {
    use crate::style::ColorChoice;
    let mut remaining = Vec::new();
    let mut choice = None;
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        if remaining.first().map(String::as_str) == Some("message") && remaining.len() >= 2 {
            remaining.push(arg);
            remaining.extend(args);
            break;
        }
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
                    | "--limit"
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

fn parse_cmux(args: &[String]) -> Result<Command> {
    match args.first().map(String::as_str) {
        Some("status") => {
            let output_json = match &args[1..] {
                [] => false,
                [flag, value] if flag == "--output" && value == "json" => true,
                _ => bail!("expected ahu cmux status [--output json]"),
            };
            Ok(Command::CmuxStatus { output_json })
        }
        Some("install") => {
            let mut harness = None;
            let mut dry_run = false;
            let mut index = 1;
            while index < args.len() {
                match args[index].as_str() {
                    "--harness" if harness.is_none() => {
                        harness = Some(value_for("--harness", args, &mut index)?.to_string());
                    }
                    "--dry-run" if !dry_run => dry_run = true,
                    _ => bail!("expected ahu cmux install --harness ID [--dry-run]"),
                }
                index += 1;
            }
            let harness =
                harness.ok_or_else(|| crate::util::Error::new("--harness ID required"))?;
            if !["claude-code", "codex", "opencode", "antigravity"].contains(&harness.as_str()) {
                bail!("unsupported harness; use claude-code, codex, opencode, or antigravity");
            }
            Ok(Command::CmuxInstall { harness, dry_run })
        }
        _ => bail!(
            "expected ahu cmux status [--output json] or ahu cmux install --harness ID [--dry-run]"
        ),
    }
}

#[cfg(test)]
mod parser_tests {
    use super::*;

    fn assert_usage(args: &[&str], diagnostic: &str) {
        let error = parse(args.iter().copied()).unwrap_err();
        assert_eq!(error.kind(), crate::util::ErrorKind::Usage, "{args:?}");
        assert!(error.to_string().contains(diagnostic), "{args:?}: {error}");
    }

    #[test]
    fn task_listing_json_conflicts_and_limit_boundaries() {
        assert_eq!(
            parse(["tasks", "--output", "json"]).unwrap(),
            Command::TasksJson
        );
        for limit in [1, 500] {
            assert_eq!(
                parse(["tasks", "--limit", &limit.to_string()]).unwrap(),
                Command::Tasks { limit: Some(limit) }
            );
        }
        for selection in [vec!["--all"], vec!["--limit", "20"]] {
            for json_first in [false, true] {
                let mut args = vec!["tasks"];
                if json_first {
                    args.extend(["--output", "json"]);
                }
                args.extend(selection.iter().copied());
                if !json_first {
                    args.extend(["--output", "json"]);
                }
                assert_usage(&args, "returns the complete list");
            }
        }
        assert_usage(&["tasks", "--output", "yaml"], "expected json");
        assert_usage(&["tasks", "--limit", "NaN"], "between 1 and 500");
        for options in [
            vec!["--all", "--all"],
            vec!["--limit", "20", "--limit", "21"],
            vec!["--limit", "20", "--all"],
            vec!["--output", "json", "--output", "json"],
        ] {
            let mut args = vec!["tasks"];
            args.extend(options);
            assert_usage(&args, "unexpected option");
        }
    }

    #[test]
    fn help_flags_preserve_option_values_and_message_payloads() {
        assert_usage(&["help", "eval", "run", "extra"], "usage: ahu help");
        for flag in ["--help", "-h"] {
            assert_usage(&[flag, "tasks"], "use `ahu help COMMAND`");
            assert_eq!(
                parse(["eval", "run", flag]).unwrap(),
                Command::Help {
                    topic: Some("eval run".into()),
                }
            );
            assert_eq!(
                parse(["message", "fixture", flag]).unwrap(),
                Command::Message {
                    task_id: "fixture".into(),
                    text: flag.into(),
                }
            );
            assert_eq!(
                parse(["onboard", "--model", flag]).unwrap(),
                Command::Onboard {
                    register: None,
                    remove: None,
                    model: Some(flag.into()),
                    version: "0.1.0".into(),
                }
            );
        }
    }

    #[test]
    fn knowledge_lint_refuses_missing_repeated_and_unknown_options() {
        for output_json in [false, true] {
            let mut args = vec!["knowledge", "lint"];
            if output_json {
                args.extend(["--output", "json"]);
            }
            assert_eq!(parse(args).unwrap(), Command::KnowledgeLint { output_json });
        }
        for (args, diagnostic) in [
            (vec!["knowledge"], "needs a subcommand"),
            (vec!["knowledge", "fetch"], "unknown subcommand"),
            (
                vec!["knowledge", "lint", "--output"],
                "--output needs a value",
            ),
            (
                vec!["knowledge", "lint", "--output", "text"],
                "unsupported --output",
            ),
            (
                vec!["knowledge", "lint", "--output", "json", "--output", "json"],
                "unknown or repeated option",
            ),
            (
                vec!["knowledge", "lint", "--unknown"],
                "unknown or repeated option",
            ),
        ] {
            assert_usage(&args, diagnostic);
        }
    }

    #[test]
    fn batch_control_requires_task_and_scopes_resume_options() {
        for action in ["wait", "result", "cancel", "cleanup", "resume"] {
            for json in [false, true] {
                let mut args = vec![action, "ahu:task:fixture"];
                if json {
                    args.extend(["--output", "json"]);
                }
                let prompt = if action == "resume" {
                    args.extend(["--prompt-file", "next task.txt"]);
                    Some(PathBuf::from("next task.txt"))
                } else {
                    None
                };
                assert_eq!(
                    parse(args).unwrap(),
                    Command::BatchControl {
                        action: action.into(),
                        task_id: "ahu:task:fixture".into(),
                        prompt,
                        json,
                    }
                );
            }
            assert_usage(&[action], "task id required");
            assert_usage(&[action, "--output", "json"], "task id required");
            assert_usage(&[action, "fixture", "--output", "yaml"], "expected json");
            assert_usage(&[action, "fixture", "--output"], "--output needs a value");
            assert_usage(
                &[action, "fixture", "--output", "json", "--output", "json"],
                "unexpected option",
            );
            if action != "resume" {
                assert_usage(
                    &[action, "fixture", "--prompt-file", "task.txt"],
                    "unexpected option",
                );
            }
        }
        assert_usage(&["resume", "fixture"], "resume requires --prompt-file");
        assert_usage(
            &["resume", "fixture", "--prompt-file"],
            "--prompt-file needs a value",
        );
        assert_usage(
            &[
                "resume",
                "fixture",
                "--prompt-file",
                "a",
                "--prompt-file",
                "b",
            ],
            "unexpected option",
        );
    }

    #[test]
    fn onboard_parses_registration_removal_and_preview() {
        assert_eq!(
            parse(["onboard"]).unwrap(),
            Command::Onboard {
                register: None,
                remove: None,
                model: None,
                version: "0.1.0".into(),
            }
        );
        assert_eq!(
            parse([
                "onboard",
                "--register",
                "fixture",
                "--model",
                "fixture-model",
                "--agent-version",
                "1.2.3",
            ])
            .unwrap(),
            Command::Onboard {
                register: Some("fixture".into()),
                remove: None,
                model: Some("fixture-model".into()),
                version: "1.2.3".into(),
            }
        );
        assert_eq!(
            parse(["onboard", "--remove", "fixture"]).unwrap(),
            Command::Onboard {
                register: None,
                remove: Some("fixture".into()),
                model: None,
                version: "0.1.0".into(),
            }
        );
        for flag in ["--register", "--remove", "--model", "--agent-version"] {
            assert_usage(&["onboard", flag], &format!("{flag} needs a value"));
        }
        assert_usage(&["onboard", "--unknown"], "unknown option");
        assert_usage(
            &["onboard", "--register", "a", "--remove", "b"],
            "cannot be combined",
        );
    }

    #[test]
    fn internal_workers_require_task_directory() {
        for command in ["run-task", "supervise"] {
            let task_dir = PathBuf::from("task directory");
            let expected = if command == "run-task" {
                Command::RunTask { task_dir }
            } else {
                Command::BatchSupervisor { task_dir }
            };
            assert_eq!(
                parse([command, "--task-dir", "task directory"]).unwrap(),
                expected
            );
            assert_usage(&[command], "--task-dir");
            assert_usage(&[command, "--task-dir"], "--task-dir");
        }
        assert_usage(&["run-task", "--unknown"], "unknown option");
        for args in [
            vec!["supervise", "directory"],
            vec!["supervise", "--unknown", "directory"],
            vec!["supervise", "--task-dir", "directory", "extra"],
        ] {
            assert_usage(&args, "supervise requires --task-dir PATH");
        }
    }

    #[test]
    fn cmux_install_validates_harness_and_singleton_options() {
        for harness in ["claude-code", "codex", "opencode", "antigravity"] {
            for dry_run in [false, true] {
                let mut args = vec!["cmux", "install"];
                if dry_run {
                    args.push("--dry-run");
                }
                args.extend(["--harness", harness]);
                assert_eq!(
                    parse(args).unwrap(),
                    Command::CmuxInstall {
                        harness: harness.into(),
                        dry_run,
                    }
                );
            }
        }
        assert_usage(&["cmux", "install"], "--harness ID required");
        assert_usage(&["cmux", "install", "--harness"], "--harness needs a value");
        assert_usage(
            &["cmux", "install", "--harness", "unknown"],
            "unsupported harness",
        );
        for args in [
            vec![
                "cmux",
                "install",
                "--harness",
                "codex",
                "--harness",
                "opencode",
            ],
            vec![
                "cmux",
                "install",
                "--harness",
                "codex",
                "--dry-run",
                "--dry-run",
            ],
            vec!["cmux", "install", "--unknown"],
            vec!["cmux", "status", "--output", "yaml"],
            vec!["cmux"],
            vec!["cmux", "unknown"],
        ] {
            assert_usage(&args, "expected ahu cmux");
        }
    }

    #[test]
    fn eval_run_checks_numeric_limits_and_candidate_identity() {
        let base = [
            "eval",
            "run",
            "--case",
            "case.md",
            "--records",
            "runs.jsonl",
        ];
        for (flag, values, diagnostic) in [
            (
                "--runs",
                vec!["0", "101", "no", "4294967296"],
                "--runs must be an integer",
            ),
            (
                "--timeout",
                vec!["0", "86401", "no", "18446744073709551616"],
                "--timeout must be an integer",
            ),
        ] {
            for value in values {
                let mut args = base.to_vec();
                args.extend(["--agent", "@fixture", flag, value]);
                assert_usage(&args, diagnostic);
            }
        }
        for (extra, diagnostic) in [
            (vec![], "needs --agent"),
            (vec!["--agent", "@"], "--agent must name"),
            (
                vec!["--agent", "@fixture", "--agent", "@fixture"],
                "named more than once",
            ),
            (
                vec!["--agent", "@fixture", "--agent", "@@fixture"],
                "named more than once",
            ),
            (
                vec!["--agent", "@fixture", "--evaluator", "judge"],
                "--evaluator must name",
            ),
            (
                vec!["--agent", "@fixture", "--evaluator", "@"],
                "--evaluator must name",
            ),
            (
                vec!["--agent", "@fixture", "--output", "yaml"],
                "unsupported --output",
            ),
        ] {
            let mut args = base.to_vec();
            args.extend(extra);
            assert_usage(&args, diagnostic);
        }
        assert_usage(
            &["eval", "run", "--case", "case.md", "--agent", "@fixture"],
            "needs --records",
        );

        // The maximum candidate count is accepted, with input order intact.
        let agents: Vec<String> = (0..16).map(|n| format!("@fixture{n}")).collect();
        let mut args: Vec<String> = base.iter().map(|s| s.to_string()).collect();
        for agent in &agents {
            args.extend(["--agent".into(), agent.clone()]);
        }
        args.extend(
            [
                "--runs",
                "100",
                "--timeout",
                "86400",
                "--allow-widened-approvals",
            ]
            .map(str::to_string),
        );
        assert_eq!(
            parse(args.clone()).unwrap(),
            Command::EvalRun {
                case: Some("case.md".into()),
                suite: None,
                agents,
                evaluator: None,
                evaluator_repo: None,
                decision_evaluator: false,
                skill_selection: crate::skill_selection::Mode::None,
                records: "runs.jsonl".into(),
                runs: 100,
                timeout_seconds: 86400,
                allow_widened_approvals: true,
                output_json: false,
            }
        );
        args.extend(["--agent".into(), "@overflow".into()]);
        assert_usage(
            &args.iter().map(String::as_str).collect::<Vec<_>>(),
            "at most 16",
        );
    }

    #[test]
    fn eval_skill_selection_is_explicit_bounded_and_documented() {
        for mode in ["none", "lexical", "decision"] {
            let parsed = parse(
                [
                    "eval",
                    "run",
                    "--case",
                    "case.md",
                    "--agent",
                    "@fixture",
                    "--records",
                    "/tmp/records.jsonl",
                    "--skill-selection",
                    mode,
                ]
                .map(str::to_owned),
            )
            .unwrap();
            assert!(
                matches!(parsed, Command::EvalRun { skill_selection, .. } if skill_selection.as_str() == mode)
            );
        }
        assert_usage(
            &[
                "eval",
                "run",
                "--case",
                "case.md",
                "--agent",
                "@fixture",
                "--records",
                "/tmp/records.jsonl",
                "--skill-selection",
                "automatic",
            ],
            "skill selection mode",
        );
        assert_usage(
            &[
                "eval",
                "run",
                "--case",
                "case.md",
                "--agent",
                "@fixture",
                "--records",
                "/tmp/records.jsonl",
                "--skill-selection",
                "none",
                "--skill-selection",
                "decision",
            ],
            "unknown or repeated",
        );
        assert!(
            help_for(Some("eval run"))
                .unwrap()
                .contains("--skill-selection")
        );
    }

    #[test]
    fn headless_launch_keeps_options_separate_from_literal_prompt_values() {
        for policy in ["disabled", "bounded"] {
            let Command::HeadlessLaunch { launch, options } = parse([
                "@fixture",
                "--headless",
                "--background",
                "--timeout",
                "1",
                "--native-helpers",
                policy,
                "--allow-child",
                "@reader",
                "--allow-child",
                "checker",
                "--allow-child-widened",
                "@writer",
                "--prompt",
                "--background",
                "--output",
                "json",
            ])
            .unwrap() else {
                panic!("expected headless launch")
            };
            assert_eq!(
                options,
                crate::headless::Options {
                    background: true,
                    timeout_seconds: 1,
                    native_helpers: policy.into(),
                    native_helpers_explicit: true,
                    child_agents: vec!["reader".into(), "checker".into()],
                    child_widened: vec!["writer".into()],
                }
            );
            assert_eq!(
                *launch,
                Command::Launch {
                    agent: "fixture".into(),
                    prompt: PromptSource::Inline("--background".into()),
                    display: crate::launch::DisplayMetadata::default(),
                    output_json: true,
                    dry_run: false,
                    allow_widened_approvals: false,
                }
            );
        }
    }

    #[test]
    fn headless_launch_rejects_invalid_and_repeated_backend_options() {
        for (extra, diagnostic) in [
            (vec!["--headless", "--headless"], "repeated batch option"),
            (
                vec!["--background", "--background"],
                "repeated batch option",
            ),
            (
                vec!["--timeout", "1", "--timeout", "2"],
                "repeated batch option",
            ),
            (
                vec![
                    "--native-helpers",
                    "disabled",
                    "--native-helpers",
                    "bounded",
                ],
                "repeated batch option",
            ),
            (vec!["--timeout", "zero"], "--timeout needs seconds"),
            (vec!["--timeout", "0"], "timeout must be positive"),
            (
                vec!["--native-helpers", "unlimited"],
                "native helpers must be disabled or bounded",
            ),
            (
                vec!["--allow-child", "../reader"],
                "invalid child agent name",
            ),
            (
                vec!["--allow-child-widened", "@"],
                "invalid child agent name",
            ),
        ] {
            let mut args = vec!["@fixture", "--prompt", "task"];
            args.extend(extra);
            assert_usage(&args, diagnostic);
        }
    }
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
            "@fixture".into(),
            "--prompt".into(),
            "--color=always".into(),
        ])
        .unwrap();
        assert_eq!(choice, Some(crate::style::ColorChoice::Never));
        assert_eq!(args, ["@fixture", "--prompt", "--color=always"]);
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
                    "@fixture".into(),
                    option.into(),
                    value.into(),
                    "--color=never".into(),
                ])
                .unwrap();
                assert_eq!(choice, Some(crate::style::ColorChoice::Never));
                assert_eq!(args, ["@fixture", option, value]);
            }
        }
    }
}

#[cfg(test)]
mod focused_help_tests {
    use super::*;

    fn parse_args(args: &[&str]) -> Result<Command> {
        parse(args.iter().copied())
    }

    #[test]
    fn help_is_short_by_default_and_focused_by_command() {
        assert!(help_for(None).unwrap().len() < HELP_ALL.len() / 3);
        assert!(
            help_for(Some("tasks"))
                .unwrap()
                .contains("default limit: 20")
        );
        assert!(help_for(Some("doctor")).unwrap().contains("--verbose"));
        assert!(help_for(Some("all")).unwrap().contains("eval run options:"));
        assert!(
            parse_args(&["setup", "--help"]).unwrap()
                == Command::Help {
                    topic: Some("setup".into())
                }
        );
        assert!(
            parse_args(&["help", "tasks"]).unwrap()
                == Command::Help {
                    topic: Some("tasks".into())
                }
        );
    }

    #[test]
    fn task_limits_and_doctor_verbosity_are_parsed_explicitly() {
        assert_eq!(
            parse_args(&["tasks"]).unwrap(),
            Command::Tasks { limit: Some(20) }
        );
        assert_eq!(
            parse_args(&["tasks", "--all"]).unwrap(),
            Command::Tasks { limit: None }
        );
        assert_eq!(
            parse_args(&["tasks", "--limit", "7"]).unwrap(),
            Command::Tasks { limit: Some(7) }
        );
        assert!(parse_args(&["tasks", "--limit", "0"]).is_err());
        assert!(parse_args(&["tasks", "--limit", "501"]).is_err());
        assert!(parse_args(&["tasks", "--all", "--limit", "4"]).is_err());
        assert_eq!(
            parse_args(&["doctor"]).unwrap(),
            Command::Doctor { verbose: false }
        );
        assert_eq!(
            parse_args(&["doctor", "--verbose"]).unwrap(),
            Command::Doctor { verbose: true }
        );
    }

    #[test]
    fn help_topics_and_slices_are_consistent() {
        for topic in ["--help", "-h", "ahu tasks"] {
            assert!(!help_for(Some(topic)).unwrap().is_empty());
        }
        assert_eq!(help_for(Some("all")).unwrap(), HELP_ALL);
        for (topic, expected) in [
            ("setup", "Detect installed harnesses"),
            ("lock", "--update refreshes the lock"),
            ("cmux status", "native integration evidence"),
            ("cmux install", "Preview or delegate"),
            ("mcp serve", "stdio MCP"),
            ("resume", "recorded native session"),
            ("run-task", "Internal worker"),
            ("supervise", "Internal supervisor"),
        ] {
            assert!(help_for(Some(topic)).unwrap().contains(expected), "{topic}");
        }
        for (topic, starts_with) in [
            ("eval run", "eval run options:"),
            ("eval report", "eval report options:"),
            ("@agent", "agent assignment options:"),
            ("explain", "explain options:"),
            ("onboard", "onboard options:"),
            ("knowledge lint", "knowledge lint options:"),
        ] {
            assert!(
                help_for(Some(topic)).unwrap().starts_with(starts_with),
                "{topic}"
            );
        }
        assert!(help_for(Some("agents")).unwrap().contains("ahu agents"));
        assert!(
            help_for(Some("@agent"))
                .unwrap()
                .contains("--allow-widened-approvals")
        );
        assert!(help_for(Some("launch")).is_err());
        assert!(help_for(Some("unknown")).is_err());
        assert!(help_section("absent:", "also absent:").is_empty());
        assert!(help_line_for("unlisted").is_none());
    }
}

#[cfg(test)]
mod direct_agent_tests {
    use super::*;

    #[test]
    fn direct_agent_syntax_accepts_positional_prompt() {
        for args in [
            vec!["@dev-glm", "fix", "the", "parser"],
            vec!["@dev-glm", "--prompt", "fix the parser"],
        ] {
            let Command::Launch {
                agent,
                prompt: PromptSource::Inline(prompt),
                ..
            } = parse_with_stdin(args, false).unwrap()
            else {
                panic!("expected inline launch");
            };
            assert_eq!(agent, "dev-glm");
            assert_eq!(prompt, "fix the parser");
        }
    }

    #[test]
    fn direct_agent_without_prompt_preselects_interactive_or_reads_piped_stdin() {
        assert_eq!(
            parse_with_stdin(["@dev-glm"], false).unwrap(),
            Command::Interactive {
                focus: true,
                agent: Some("dev-glm".into()),
            }
        );
        assert_eq!(
            parse_with_stdin(["@dev-glm"], true).unwrap(),
            Command::Launch {
                agent: "dev-glm".into(),
                prompt: PromptSource::Stdin,
                display: crate::launch::DisplayMetadata::default(),
                output_json: false,
                dry_run: false,
                allow_widened_approvals: false,
            }
        );
    }

    #[test]
    fn help_documents_lock_and_positional_prompt_conflicts_are_rejected() {
        assert!(
            HELP.lines()
                .any(|line| line.trim_start().starts_with("lock [--update]"))
        );
        assert!(parse(["inventory"]).is_err());
        assert!(parse(["hygiene"]).is_err());
        assert!(parse(["@dev-glm", "positional", "--prompt", "flag"]).is_err());
    }

    /// `ahu diff` was removed; plain git on the task's branch does the same
    /// job. It must now fail exactly like any other word ahu does not know,
    /// not be quietly re-parsed as something else.
    #[test]
    fn diff_is_no_longer_a_command() {
        let unknown = parse(["not-a-command"]).expect_err("parsed");
        for args in [vec!["diff"], vec!["diff", "abc1"]] {
            let error = parse(args.clone()).unwrap_err();
            assert_eq!(error.kind(), unknown.kind(), "{args:?}");
            assert!(error.to_string().contains("unknown command"), "{error}");
        }
    }
}
