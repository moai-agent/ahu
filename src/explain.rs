//! `ahu explain` — the architecture overview built into the CLI.
//!
//! The overview exists so the model ahu works to is available where people
//! actually are. It is written once as a small document tree and rendered two
//! ways, so the terminal text and the Markdown can never drift apart.
//!
//! The Markdown form carries the diagrams as ```` ```mermaid ```` fences, which
//! render unchanged on GitHub and in cmux's own Markdown viewer.

use std::path::{Path, PathBuf};

use crate::catalog;
use crate::hooks::{NON_PROJECT_HOOK_DETAIL, NON_PROJECT_HOOK_WARNING};
use crate::util::{Error, Result};

/// Interactive composition and the two execution backends, as a Mermaid flowchart.
pub const MERMAID_PIPELINE: &str = r#"flowchart TD
    A["ahu (in a Git repo)"] --> B{".agents/ahu/config.toml?"}
    B -- no --> C["First-run setup:<br/>project-agreed harness order,<br/>model order, catalog pin"]
    C --> D
    B -- yes --> D["Launcher"]

    D --> E{"@agent typed?"}
    E -- "@chris" --> F["Named identity<br/>manifest pins harness + model"]
    E -- blank --> G["Automatic selection<br/>walk project rankings"]
    F --> H["Show resolved harness + model<br/>BEFORE the prompt is typed"]
    G --> H

    H --> I["Prompt composer<br/>paste never submits"]
    I --> J["Plan: snapshot config, detect drift,<br/>read hooks, build argv"]
    X["Scripted launch<br/>prompt file, inline or stdin"] --> J
    J --> K["Submission preview<br/>identity, Git effects, hooks, warnings"]
    K -- "no" --> L["Nothing created"]
    K -- "yes" --> M["git worktree add<br/>branch ahu/&lt;agent&gt;/&lt;task-id&gt;"]

    M --> N["Materialize parent agent config<br/>at native paths"]
    N --> O["Write task record + prompt.txt"]
    O --> U{"Execution backend"}
    U -- headless --> V["Supervisor: batch argv,<br/>primary-owned coordination"]
    U -- interactive --> P["cmux: find-or-create repository group"]
    P --> Q["cmux: child workspace in that group"]
    Q --> R["Shell runs:<br/>ahu run-task --task-dir '...'"]
    R --> S["Re-derive argv, compare to record"]
    S --> T["supervise configured harness<br/>model flags + one composed prompt argument"]
"#;

/// How a prompt reaches the harness without ever being shell input, and what
/// else travels in the same argv element.
pub const MERMAID_PROMPT: &str = r#"flowchart LR
    P["Pasted prompt<br/>$(...), backticks, newlines"] --> F["prompt.txt<br/>mode 0600"]
    F -.->|"read at start"| R["ahu run-task"]
    D["ahu delegation contract"] --> N["Compose, fenced with<br/>a per-launch nonce"]
    A["Agent instructions<br/>manifest body or source.path"] --> N
    R --> N
    N -->|"one argv element"| C["configured harness<br/>contract + instructions + prompt"]

    S["cmux startup command<br/>(shell-interpreted)"] --> R
    Q["Only ahu's own exe path<br/>+ task dir, single-quoted"] --> S

    style F fill:#e8f4ea,stroke:#3a7d44
    style Q fill:#e8f4ea,stroke:#3a7d44
    style N fill:#fdf1e7,stroke:#b5651d
"#;

/// What travels into a task worktree and what stays behind.
pub const MERMAID_INHERITANCE: &str = r#"flowchart TB
    subgraph parent["Invoking checkout"]
        PC[".agents/ .claude/ .codex/<br/>CLAUDE.md AGENTS.md .mcp.json<br/>committed, uncommitted, gitignored"]
        PS["Unrelated dirty source files"]
        PU["~/.claude/settings.json<br/>managed settings, plugins"]
    end

    subgraph task["Task worktree (base commit + config)"]
        TC["Same files, same native paths<br/>local deletions honoured<br/>executable bits preserved"]
    end

    PC -->|"copied"| TC
    PS -.->|"NOT copied"| TC
    PU -.->|"never copied; still applies<br/>from its native location"| TC

    style PS stroke-dasharray: 4 4
    style PU stroke-dasharray: 4 4
"#;

/// Where context comes from, and how much of it ahu can actually see.
pub const MERMAID_CONTEXT: &str = r#"flowchart LR
    subgraph seen["ahu can read these"]
        I["Agent identity<br/>name, version, harness, model,<br/>instructions source"]
        AD["ahu-delivered prompt text<br/>contract + agent instructions<br/>not enforced by any harness"]
        R["Repository instructions<br/>CLAUDE.md, AGENTS.md"]
        K["Skills"]
        M["MCP config"]
        H["Hooks<br/>project / local / user / managed"]
        T["Task prompt"]
    end

    subgraph unseen["ahu cannot establish these completely"]
        B["Harness built-in system prompt"]
        W["Effective wrapper hook injection"]
        PL["Plugin-contributed hooks"]
        L["Which sources actually loaded"]
        X["Retrieval, compaction, caches"]
    end

    seen --> S["Session"]
    unseen --> S

    style unseen fill:#fdf1e7,stroke:#b5651d
"#;

/// One piece of the overview. Rendered differently for a terminal and for
/// Markdown, but authored only once.
pub enum Block {
    /// Pre-wrapped prose lines.
    Para(Vec<String>),
    Bullets(Vec<String>),
    /// Each entry is one numbered item, already wrapped across lines.
    Numbered(Vec<Vec<String>>),
    Mermaid(&'static str),
    Warning {
        headline: String,
        detail: Vec<String>,
    },
    /// A small fixed-column table: harness, status, verification.
    Rows {
        headers: [&'static str; 3],
        rows: Vec<[String; 3]>,
    },
}

pub struct Section {
    pub title: &'static str,
    pub blocks: Vec<Block>,
}

fn para(lines: &[&str]) -> Block {
    Block::Para(lines.iter().map(|l| l.to_string()).collect())
}

fn bullets(lines: &[&str]) -> Block {
    Block::Bullets(lines.iter().map(|l| l.to_string()).collect())
}

/// The overview, as a document.
pub fn document() -> Vec<Section> {
    vec![
        Section {
            title: "What ahu is",
            blocks: vec![
                para(&[
                    "ahu is cross-harness configuration management for agent sessions. It launches",
                    "agents that a repository defines into separate Git worktrees",
                    "and runs interactive sessions in cmux or unattended headless attempts.",
                ]),
                para(&[
                    "ahu is not an agent harness. It does not host a model, run an agent loop, own a",
                    "conversation, or provide tools. Claude Code, Codex, the Antigravity CLI and OpenCode do",
                    "that. ahu decides which of them runs, with which model and instructions, in",
                    "which worktree, and reports visible context sources and coverage gaps.",
                ]),
                para(&[
                    "An agent's harness, model, and instructions live in the repository, so changing",
                    "how it behaves is a reviewable change like any other. A launch uses exactly that",
                    "configuration or it fails.",
                ]),
            ],
        },
        Section {
            title: "Delegation follows the entrypoint",
            blocks: vec![para(&[
                "Outside ahu, sub-agents and fan out use the current harness's native",
                "sub-agent features. Inside ahu, registered assignments use registered ahu",
                "agents, each using its configured harness and model in a separate worktree.",
                "Headless children inherit that backend; interactive children use cmux.",
                "Use ahu launch @name --prompt-file /path/to/task.txt.",
                "Headless native helpers follow the frozen policy described below. Every",
                "launch supplies its backend delegation contract as prompt text on every",
                "harness, inside a fence tagged with a per-launch nonce,",
                "ahead of the agent's instructions and then the task prompt. ahu uses no",
                "system-prompt or agent-selection flag anywhere, so none of it is enforced;",
                "the task prompt that follows can contradict it, and ahu cannot prevent a",
                "harness or its shell tools from starting other processes.",
            ])],
        },
        Section {
            title: "What ahu needs",
            blocks: vec![
                bullets(&[
                    "**cmux**, required for interactive task sessions; headless execution needs no cmux.",
                    "**A supported harness**, installed and signed in by you: Claude Code, Codex, the Antigravity CLI, or OpenCode.",
                    "**Git**: every task gets its own branch and worktree.",
                ]),
                para(&[
                    "ahu creates the repository group and the per-task workspace through cmux, and",
                    "interactive sessions require that connection. Headless tasks and local inspection do",
                    "not. The harness is what actually runs the",
                    "agent; ahu never installs, configures, or authenticates one, and it does not",
                    "ship one. Sign-in is the harness's own, and ahu holds no API key for any of",
                    "them.",
                    "Git and default cmux lookup skip empty and relative PATH entries and use",
                    "canonical executables outside Git working trees. Candidate ancestry is",
                    "inspected without running candidate Git. Explicit AHU_CMUX_BIN retains",
                    "user-selected command semantics; these checks are not OS isolation.",
                ]),
                para(&[
                    "ahu does not use an agent-selection or system-prompt flag. A native lookup",
                    "by name does not bind the selection to the file ahu read and digested.",
                    "Instructions travel in the prompt on all three harnesses, attributed to",
                    "their source file and delivered-text digest; delivery is not enforcement.",
                ]),
                para(&[
                    "ahu holds no credentials and speaks to no model provider. Sign-in and the",
                    "approval boundary are the harness's own. Headless adapters pass explicit batch",
                    "controls; manifest-requested widening remains gated. ahu cannot assert the",
                    "effective boundary; harness settings also apply. The",
                    "launch preview reports what ahu read in them rather than asserting a result.",
                ]),
            ],
        },
        Section {
            title: "Four rules everything else follows from",
            blocks: vec![Block::Numbered(
                FOUR_RULES
                    .iter()
                    .map(|rule| rule.iter().map(|l| l.to_string()).collect())
                    .collect(),
            )],
        },
        Section {
            title: "Launch pipeline",
            blocks: vec![Block::Mermaid(MERMAID_PIPELINE)],
        },
        Section {
            title: "Identity and selection",
            blocks: vec![
                para(&[
                    "Only `.agents/ahu/agents/<name>.md` makes an agent launchable. Definitions found",
                    "anywhere else are onboarding candidates, never implicit registrations: a skill is",
                    "not an agent, and AGENTS.md is not an agent registry. A manifest names an explicit",
                    "harness, an exact model identifier (never an alias), and a semantic version, and",
                    "either carries its instructions in its body or references a native definition in",
                    "place. A nonempty parsed frontmatter model other than inherit must match; ahu",
                    "does not rewrite either file.",
                ]),
                para(&[
                    "At launch ahu reads that file, records two digests of it — one over the whole",
                    "file, one over exactly the instruction text it will deliver — and puts the",
                    "instruction text into the harness's prompt, ahead of the task prompt. It passes",
                    "no agent-selection flag on any harness. The harness and the model are fixed by",
                    "real flags; the identity is not, and ahu reports that as a gap rather than a",
                    "control.",
                ]),
                para(&[
                    "Without an @agent, ahu walks the project's harness order, takes the first",
                    "with an available adapter and nonempty project model ranking, and selects",
                    "that ranking's first model. There is no catalog fallback or named identity. Local",
                    "prerequisites are checked after the pair is resolved, so a missing installation is",
                    "a diagnostic for that machine, never a different selection for that user.",
                ]),
            ],
        },
        Section {
            title: "Prompt transport",
            blocks: vec![
                Block::Mermaid(MERMAID_PROMPT),
                para(&[
                    "The cmux startup command is shell-interpreted, so it contains only ahu's own",
                    "executable path and task directory, both single-quoted. The prompt is written to a",
                    "file and handed to the harness as one argument vector element. Shell syntax inside",
                    "a prompt is therefore delivered literally and never evaluated.",
                ]),
                para(&[
                    "That one element holds everything ahu supplies, in a fixed order on every",
                    "harness: the delegation contract, available metadata and state, selected agent",
                    "instructions, then the request. Layout 3 fences each section with XML-shaped",
                    "tags carrying the launch nonce in both opening and closing names. Bodies retain",
                    "their exact bytes; this is raw text framing, not an escaped XML document.",
                    "Metadata and state hold frozen execution facts and native-session references,",
                    "without importing native history. Fence collisions refuse delivery; fences do",
                    "not enforce authority. Older records without a layout version replay layout 1",
                    "exactly; layout 2 keeps its frozen bytes. Unknown layouts refuse.",
                ]),
            ],
        },
        Section {
            title: "Worktree inheritance",
            blocks: vec![
                Block::Mermaid(MERMAID_INHERITANCE),
                para(&[
                    "Every task gets a unique id, a fresh branch `ahu/<agent>/<task-id>`, and a fresh",
                    "worktree under .worktrees/ in the primary checkout. The worktree starts",
                    "at the invoking checkout's HEAD, then receives its recognized agent",
                    "configuration as it stands at submission — including uncommitted and Git-ignored",
                    "files, with local deletions honoured. Scan skips and depth limits bound coverage;",
                    "committed files under skipped paths still arrive through Git. Configuration",
                    "symlinks are not followed. Unrelated dirty source files stay behind.",
                    "Nothing is staged, committed, stashed, or reset in your checkout, ever.",
                ]),
            ],
        },
        Section {
            title: "Context sources",
            blocks: vec![
                Block::Mermaid(MERMAID_CONTEXT),
                para(&[
                    "`ahu inventory` marks each source loaded, available, disabled, opaque, or absent,",
                    "and ends with what ahu cannot see. `available` means the harness can discover a",
                    "source, not that its contents reached the model. The inventory is never labelled",
                    "complete.",
                ]),
            ],
        },
        Section {
            title: "Hooks",
            blocks: vec![
                para(&[
                    "Hooks run on harness lifecycle events and can affect tool calls or context.",
                    "ahu inventories Claude Code hook settings. Codex, Antigravity and OpenCode",
                    "launches report unknown hook coverage; a missing scan is not evidence of no",
                    "hooks. An OpenCode launch does name the plugin modules a repository declares.",
                ]),
                para(&[
                    "Ordinary launch and inspection do not write hooks. Recognized configuration",
                    "travels into the task worktree with executable bits preserved; the harness",
                    "decides whether and when hooks run. Local settings can travel without being",
                    "shared policy. User and managed settings remain at their native locations.",
                    "Hooks outside project policy raise the consistency warning below.",
                    "ahu cmux status, doctor and launch disclosures share native integration evidence.",
                    "Registration, activation, isolation and live conformance are separate facts.",
                    "Explicit ahu cmux install --harness ID delegates fixed native installer argv;",
                    "--dry-run previews it. Native confirmations, scope and exit status are preserved.",
                    "The installer mapping is reviewed only for cmux 0.64.22 build ddd4a01bc.",
                    "Claude uses cmux Settings > Automation, with no native installer operation.",
                    "Reviewed OpenCode Feed lacks disable/surface guards and refuses headless use.",
                    "Unknown components and builds stay unknown. Static inspection is not live testing.",
                ]),
            ],
        },
        Section {
            title: "Versions and drift",
            blocks: vec![para(&[
                "Every named agent carries a semantic version. ahu does not automate releases, but it",
                "will not let a version label quietly cover changed inputs: if `chris@1.2.0` launches",
                "with different instructions, repository configuration, policy, or hooks than the",
                "last `chris@1.2.0` launch, that drift is reported as a pending behaviour change for",
                "the next version bump. It is not classified as safe.",
                "",
                "Drift names which digest moved. The instructions digest covers exactly the text ahu",
                "delivered; the file digest covers the whole source file including any frontmatter.",
                "An edit that changes only frontmatter moves the second and not the first, and drift",
                "says so rather than reporting one number that could mean either.",
            ])],
        },
        Section {
            title: "Headless attempts and results",
            blocks: vec![para(&[
                "launch --headless uses batch execution without cmux or a PTY. Add --background",
                "to return after supervisor startup; otherwise execution stays in the foreground.",
                "The preview names the admitted CLI profile, exact command, timeout and gaps.",
                "Known cmux wrappers and unsupported versions fail without a fallback.",
                "Minimal coordination belongs to the primary checkout's owner-only .ahu/state/.",
                "Native event streams, stderr, final text and helper summaries are not copied.",
                "New results use schema 2, with bounded outcome metadata and native references.",
                "Unverified native locations remain unknown; references do not import history.",
                "Native harness session stores keep their own external homes and retention.",
                "tasks and task show backend, attempt, ownership and blockers; wait follows an attempt, result",
                "reads its outcome and known native session with provenance and artifact locations.",
                "Human inspection bounds and escapes metadata; process completion is not acceptance.",
                "resume explicitly continues its recorded native session, and",
                "cancel requests termination of the task and its recorded ahu descendants; an",
                "interactive (cmux) task is stopped by its run-task parent. Confirmed cancellation",
                "closes its workspace; an unconfirmed request leaves it open. Work and records stay.",
                "Earlier attempt artifacts survive resume. Configuration or executable drift",
                "refuses resume. A child cannot resume after its owning parent attempt terminates;",
                "child/worker resume is also unsupported while its parent is live.",
                "Legacy schema-1 resume and dispatch need the original runner or a new task.",
                "Submit a new registered assignment with the prior result",
                "and explicit source/revision scope; dirty changes and native sessions do not",
                "transfer automatically. Child resume does not use the launch broker.",
                "Supervisor loss is interrupted, with no automatic replay.",
                "Process success and harness success are evidence, not orchestrator acceptance.",
                "Reports and same-user editable records remain untrusted; provider-managed child",
                "cleanup and child usage accounting can be unknown. Review actual work and tests.",
                "Host submission grants registered children with --allow-child @name or",
                "--allow-child-widened @name. Grants freeze identity and policy; descendants",
                "cannot expand them. The supervisor broker dispatches the real registered child",
                "outside the worker sandbox using the child's own harness and approval mapping.",
                "Codex workspace-write gets a narrow request-directory write root; read-only",
                "Codex broker transport is refused. Dead or stale parent attempts cannot admit",
                "children. Failed or unjoined current-attempt children block parent success.",
                "Limits: depth eight, 128 assignments per root grant, 16 active/interrupted",
                "assignments per repository runtime store. No global token cap is promised.",
                "Native helpers default to disabled. Bounded supports Claude Code 2.1.270 only:",
                "the entire owner and helpers have read-only model tools, with no shell or edits.",
                "The owner cannot shell-launch ahu children. A shell-capable coordinator instead",
                "grants a separate registered bounded reviewer, which uses helpers internally.",
                "Bounded uses one concurrent helper, depth one, the owner's exact model and a",
                "USD 5 budget per attempt. Roles are requested; total helper count is not capped.",
                "MCP and slash commands are excluded; settings and deny rules remain discoverable.",
                "The model tool ceiling does not prove hooks cannot write or spawn processes.",
                "Known successful helper joins are required. Provider-side cleanup stays unknown.",
                "Codex 0.154.0/0.155.1, Antigravity 1.2.2 and OpenCode 1.18.29/1.18.30/1.18.31 admit ordinary",
                "headless execution, but refuse bounded helpers. Claude 2.1.269 also admits only",
                "the disabled profile.",
                "Same-user code is not isolated from broker state. Hooks and arbitrary shell",
                "commands require a vetted environment; ahu's backend itself does not use cmux.",
                "Explicit cleanup removes recognized old captures and bounded requests, retaining",
                "structured results, frozen inputs, native stores, branches and worktrees.",
            ])],
        },
        Section {
            title: "State and records",
            blocks: vec![para(&[
                "Task worktrees are siblings under `.worktrees/` in the primary checkout.",
                "An interactive task's record and prompt live in `.ahu/state/` in its worktree, chosen",
                "by ahu at launch and passed to the session, so removing that worktree removes",
                "them with it. `ahu tasks`, `task`, `diff` and `focus` find them by looking through",
                "`.worktrees/`, from the primary checkout or from any sibling. The launch lock,",
                "cmux group mapping, headless coordination and task index belong to the primary;",
                "hygiene timestamps stay in the checkout they were recorded from. `.ahu/` ignores",
                "itself in Git. Nested sessions discover state from their working checkout;",
                "coordination stays in the primary checkout. Legacy lookup reads the primary and",
                "invoking plain-checkout stores; managed worktree stores always enforce ownership.",
                "Use ahu --repo PATH before the command to select a checkout explicitly.",
                "AHU_REPO_ROOT, AHU_STATE_DIR, AHU_RUNTIME_DIR and AHU_TASK_INDEX_DIR are not selectors.",
                "Old external stores are read in place; legacy-lookup.json names additional roots.",
                "There is no migration or global task scope across unrelated repositories.",
                "Task commands accept ahu:task:<id>, bare IDs and unique prefixes consistently.",
                "Message arguments after the task reference are literal inbox payload.",
                "The run-task owner supervises the child and lends/restores terminal foreground.",
                "Cancellation uses live ownership, preserves work, and reports unconfirmed outcomes.",
                "Terminal outcomes persist even if foreground restoration fails.",
                "Existing state symlinks are refused; path checks and mutable record",
                "digests do not protect against concurrent hostile host processes. Policy never lives",
                "there. Each record freezes the launched",
                "identity, digests, base commit, branch, worktree, and cmux ids, so editing an agent",
                "later changes the next launch while a running task keeps what it started with.",
                "Misplaced records in managed stores are reported as notes, not accepted or migrated.",
                "After legacy lookup, recordless worktrees are reported as incomplete. Failed launch",
                "rollback does not force-remove worktrees holding changes; it reports retained paths",
                "and validates state paths before attempting cleanup. Listing never deletes them.",
            ])],
        },
        Section {
            title: "What ahu will not do",
            blocks: vec![bullets(&[
                "substitute a different harness or model, for any reason",
                "stage, commit, push, stash, reset, clean, or switch branches in your checkout",
                "silently add, edit, or remove hooks, or reorganise native skills, settings, or histories",
                "widen permissions unless a manifest asks for it, which the preview states in full before anything starts",
                "claim to know the effective approval boundary: the harness's own settings decide it, and ahu only reports what it read and which flags it passed",
                "install, configure, or authenticate a harness on your behalf",
                "delete a worktree, branch, or task record that may hold your work",
                "call an inventory complete, or a behaviour change harmless",
            ])],
        },
        Section {
            title: "Standing warnings",
            blocks: vec![
                para(&[
                    "ahu selects the configured harness and model at launch. During the session,",
                    "the harness manages model changes and its other native behavior. These are",
                    "normal harness capabilities, not launch problems.",
                ]),
                Block::Warning {
                    headline:
                        "ahu-supplied instructions are not enforced by the harness.".to_string(),
                    detail: vec![
                        "ahu delivers its delegation contract and the agent's instructions as"
                            .to_string(),
                        "prompt text, identically on every harness. It uses no agent-selection or"
                            .to_string(),
                        "system-prompt flag anywhere. A native agent lookup by name does not bind"
                            .to_string(),
                        "to the source file ahu read. The task prompt that follows can"
                            .to_string(),
                        "contradict any of it, and the model may follow the task prompt instead."
                            .to_string(),
                        "What ahu does pin with real flags is the harness, the exact model, and"
                            .to_string(),
                        "the permission flags a manifest asks for.".to_string(),
                    ],
                },
                Block::Warning {
                    headline: NON_PROJECT_HOOK_WARNING.to_string(),
                    detail: NON_PROJECT_HOOK_DETAIL
                        .iter()
                        .map(|l| l.to_string())
                        .collect(),
                },
            ],
        },
        Section {
            title: "Compatibility",
            blocks: vec![
                Block::Para(vec![format!(
                    "Compatibility catalog {} ships with this build and is fixed by project",
                    catalog::CATALOG_VERSION
                ),
                "configuration, so installing a newer ahu cannot silently change which model your".to_string(),
                "project selects.".to_string()]),
                Block::Rows {
                    headers: ["harness", "ahu adapter", "verified against"],
                    rows: catalog::HARNESSES
                        .iter()
                        .map(|harness| {
                            [
                                harness.id.to_string(),
                                if harness.adapter_available {
                                    "available".to_string()
                                } else {
                                    "unavailable".to_string()
                                },
                                if harness.verified_versions.is_empty() {
                                    "not verified".to_string()
                                } else {
                                    harness.verified_versions.to_string()
                                },
                            ]
                        })
                        .collect(),
                },
            ],
        },
    ]
}

/// The load-bearing principles, one entry per rule, already wrapped.
const FOUR_RULES: &[&[&str]] = &[
    &[
        "Fixed identity. A named agent's harness, model, and instructions are used",
        "as configured. No fallback model, no availability-based substitution, no",
        "task-driven prompt rewriting. Invalid configuration fails before a task starts.",
        "The harness and model are fixed by flags; the instructions are delivered as",
        "prompt text, which ahu reports as a gap rather than as enforcement.",
    ],
    &[
        "One project policy. Rankings, cadence, and the fixed catalog revision are the same",
        "for every ahu user in the project. There are no personal profiles and no",
        "command-line switches that change them. A machine that cannot meet the policy",
        "reports a diagnostic; it does not get different behaviour.",
    ],
    &[
        "Harness-native conventions win. Skills, memory, settings, and agent",
        "definitions stay where the harness wants them. ahu references them in place",
        "and never reorganises or rewrites them.",
    ],
    &[
        "Say what you cannot do. Where ahu cannot see a context source or cannot",
        "enforce an identity, it reports that rather than implying a guarantee.",
    ],
];

/// Every diagram in the document, in order.
fn diagrams() -> Vec<&'static str> {
    vec![
        MERMAID_PIPELINE,
        MERMAID_PROMPT,
        MERMAID_INHERITANCE,
        MERMAID_CONTEXT,
    ]
}

/// Just the diagrams, as a Markdown document of fenced blocks.
pub fn mermaid_only() -> String {
    let mut out = String::from("# ahu architecture\n");
    for section in document() {
        for block in &section.blocks {
            if let Block::Mermaid(source) = block {
                out.push_str(&format!(
                    "\n## {}\n\n```mermaid\n{source}```\n",
                    section.title
                ));
            }
        }
    }
    debug_assert_eq!(out.matches("```mermaid").count(), diagrams().len());
    out
}

/// The overview as plain text for a terminal.
pub fn overview() -> String {
    let mut out = format!(
        "ahu {} — architecture overview\n{}\n",
        env!("CARGO_PKG_VERSION"),
        "=".repeat(46)
    );
    for section in document() {
        out.push_str(&format!(
            "\n{}\n{}\n",
            section.title,
            "-".repeat(section.title.len())
        ));
        for block in &section.blocks {
            out.push_str(&render_terminal_block(block));
        }
        trim_blank_run(&mut out);
    }
    out.push_str(
        "\nRun `ahu explain --markdown` for the same document as Markdown, `--mermaid` for the\n\
         diagrams alone, or `--open` to render it in cmux's Markdown viewer.\n\
         Run `ahu help` for commands, and `ahu doctor` for this machine.\n",
    );
    out
}

fn render_terminal_block(block: &Block) -> String {
    let mut out = String::new();
    match block {
        Block::Para(lines) => {
            for line in lines {
                out.push_str(&format!("{}\n", strip_emphasis(line)));
            }
            out.push('\n');
        }
        Block::Bullets(items) => {
            for item in items {
                out.push_str(&format!("  - {}\n", strip_emphasis(item)));
            }
            out.push('\n');
        }
        Block::Numbered(items) => {
            for (index, lines) in items.iter().enumerate() {
                for (offset, line) in lines.iter().enumerate() {
                    if offset == 0 {
                        out.push_str(&format!("{}. {}\n", index + 1, strip_emphasis(line)));
                    } else {
                        out.push_str(&format!("   {}\n", strip_emphasis(line)));
                    }
                }
            }
            out.push('\n');
        }
        Block::Mermaid(source) => {
            out.push_str(&format!("```mermaid\n{source}```\n\n"));
        }
        Block::Warning { headline, detail } => {
            out.push_str(&format!("  !! {headline}\n"));
            for line in detail {
                out.push_str(&format!("     {line}\n"));
            }
        }
        Block::Rows { rows, .. } => {
            for row in rows {
                out.push_str(&format!("  {:<14} {:<12} {}\n", row[0], row[1], row[2]));
            }
            out.push('\n');
        }
    }
    out
}

/// The overview as a Markdown document.
pub fn markdown() -> String {
    let mut out = format!(
        "# ahu {} — architecture overview\n",
        env!("CARGO_PKG_VERSION")
    );
    for section in document() {
        out.push_str(&format!("\n## {}\n\n", section.title));
        for block in &section.blocks {
            out.push_str(&render_markdown_block(block));
        }
        trim_blank_run(&mut out);
    }
    out.push_str(
        "\n---\n\nGenerated by `ahu explain --markdown`. Run `ahu help` for commands, and\n\
         `ahu doctor` for this machine.\n",
    );
    out
}

fn render_markdown_block(block: &Block) -> String {
    let mut out = String::new();
    match block {
        Block::Para(lines) => {
            out.push_str(&lines.join("\n"));
            out.push_str("\n\n");
        }
        Block::Bullets(items) => {
            for item in items {
                out.push_str(&format!("- {item}\n"));
            }
            out.push('\n');
        }
        Block::Numbered(items) => {
            for (index, lines) in items.iter().enumerate() {
                out.push_str(&format!("{}. {}\n", index + 1, lines[0]));
                for line in &lines[1..] {
                    out.push_str(&format!("   {line}\n"));
                }
            }
            out.push('\n');
        }
        Block::Mermaid(source) => {
            out.push_str(&format!("```mermaid\n{source}```\n\n"));
        }
        Block::Warning { headline, detail } => {
            out.push_str(&format!("> **{headline}**\n>\n"));
            for line in detail {
                out.push_str(&format!("> {line}\n"));
            }
            out.push('\n');
        }
        Block::Rows { headers, rows } => {
            out.push_str(&format!(
                "| {} | {} | {} |\n",
                headers[0], headers[1], headers[2]
            ));
            out.push_str("| --- | --- | --- |\n");
            for row in rows {
                out.push_str(&format!("| {} | {} | {} |\n", row[0], row[1], row[2]));
            }
            out.push('\n');
        }
    }
    out
}

/// Leave exactly one newline at the end of the buffer.
///
/// Blocks end with their own blank line and every section heading starts with
/// one, so without this the two stack into a double blank before each heading.
fn trim_blank_run(out: &mut String) {
    while out.ends_with("\n\n") {
        out.pop();
    }
}

/// Markdown bold survives into a Markdown render but is noise in a terminal.
fn strip_emphasis(line: &str) -> String {
    line.replace("**", "")
}

/// Where `ahu explain --open` writes the document.
///
/// The generated document goes in the invoking checkout's ignored state store,
/// not in tracked project documentation.
pub fn document_path() -> Result<PathBuf> {
    Ok(crate::state::root()?.join("docs/architecture.md"))
}

/// Write the Markdown document to ahu's state directory.
pub fn write_document() -> Result<PathBuf> {
    let path = document_path()?;
    crate::state::write_private_file(&path, markdown().as_bytes())
        .map_err(|e| Error::new(format!("cannot write {}: {e}", path.display())))?;
    Ok(path)
}

/// Open the written document in cmux's Markdown viewer.
///
/// cmux's viewer renders ```` ```mermaid ```` fences as diagrams, so this is the
/// same document as `--markdown` with the pictures drawn. Verified against cmux
/// 0.64.22, whose bundled markdown viewer ships and wires up Mermaid.
pub fn open_in_cmux(path: &Path, focus: bool) -> Result<String> {
    let client = crate::cmux::Cmux::discover()?;
    client.open_markdown(path, focus)
}
