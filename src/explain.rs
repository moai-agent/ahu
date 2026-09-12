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
use crate::harness::RELIABILITY_WARNING;
use crate::hooks::{NON_PROJECT_HOOK_DETAIL, NON_PROJECT_HOOK_WARNING};
use crate::util::{Error, Result};

/// The launch pipeline, as a Mermaid flowchart.
pub const MERMAID_PIPELINE: &str = r#"flowchart TD
    A["ahu (in a Git repo, inside cmux)"] --> B{".agents/ahu/config.toml?"}
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
    J --> K["Submission preview<br/>identity, Git effects, hooks, warnings"]
    K -- "no" --> L["Nothing created"]
    K -- "yes" --> M["git worktree add<br/>branch ahu/&lt;agent&gt;/&lt;task-id&gt;"]

    M --> N["Materialize parent agent config<br/>at native paths"]
    N --> O["Write task record + prompt.txt"]
    O --> P["cmux: find-or-create repository group"]
    P --> Q["cmux: child workspace in that group"]
    Q --> R["Shell runs:<br/>ahu run-task --task-dir '...'"]
    R --> S["Re-derive argv, compare to record"]
    S --> T["exec claude --model &lt;id&gt; --agent &lt;name&gt; -- &lt;prompt&gt;"]
"#;

/// How a prompt reaches the harness without ever being shell input.
pub const MERMAID_PROMPT: &str = r#"flowchart LR
    P["Pasted prompt<br/>$(...), backticks, newlines"] --> F["prompt.txt<br/>mode 0600"]
    F -.->|"read at start"| R["ahu run-task"]
    R -->|"one argv element"| C["claude ... -- &lt;prompt&gt;"]

    S["cmux startup command<br/>(shell-interpreted)"] --> R
    Q["Only ahu's own exe path<br/>+ task dir, single-quoted"] --> S

    style F fill:#e8f4ea,stroke:#3a7d44
    style Q fill:#e8f4ea,stroke:#3a7d44
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
        I["Agent identity<br/>name, version, harness, model,<br/>system prompt source"]
        R["Repository instructions<br/>CLAUDE.md, AGENTS.md"]
        K["Skills"]
        M["MCP config"]
        H["Hooks<br/>project / local / user / managed"]
        T["Task prompt"]
    end

    subgraph unseen["ahu cannot read these"]
        B["Harness built-in system prompt"]
        W["cmux wrapper-injected hooks"]
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
                    "ahu is cross-harness configuration management for agent sessions, built for",
                    "cmux. It launches agents that a repository defines into isolated Git worktrees",
                    "and organises their sessions as rows under a repository group in cmux.",
                ]),
                para(&[
                    "ahu is not an agent harness. It does not host a model, run an agent loop, own a",
                    "conversation, or provide tools. Claude Code, Codex, and the Antigravity CLI do",
                    "that. ahu decides which of them runs, with which model and instructions, in",
                    "which worktree, and then reports honestly on everything that can influence the",
                    "session it started.",
                ]),
                para(&[
                    "An agent's harness, model, and instructions live in the repository, so changing",
                    "how it behaves is a reviewable change like any other. A launch uses exactly that",
                    "configuration or it fails.",
                ]),
            ],
        },
        Section {
            title: "What ahu needs",
            blocks: vec![
                bullets(&[
                    "**cmux**, required: every task session is a cmux workspace.",
                    "**A supported harness**, installed and authenticated by you.",
                    "**Git**: every task gets its own branch and worktree.",
                ]),
                para(&[
                    "ahu creates the repository group and the per-task workspace through cmux, and",
                    "0.1.1 has no mode that runs without it. The harness is what actually runs the",
                    "agent; ahu never installs, configures, or authenticates one, and it does not",
                    "ship one.",
                ]),
                para(&[
                    "ahu holds no credentials and speaks to no model provider. Your harness's own",
                    "authentication, permissions, and approval boundaries are used unchanged.",
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
                    "Only `.agents/ahu/agents/<name>.toml` makes an agent launchable. Definitions found",
                    "anywhere else are onboarding candidates, never implicit registrations: a skill is",
                    "not an agent, and AGENTS.md is not an agent registry. A manifest names an explicit",
                    "harness, an exact model identifier (never an alias), and a semantic version, and",
                    "points at a native definition in place. If that native file also declares a model,",
                    "the two must agree — ahu will not rewrite either file or pick one silently.",
                ]),
                para(&[
                    "Without an @agent, ahu walks the project's agreed harness order, then that",
                    "harness's agreed model order, and freezes the result for the task. Local",
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
            ],
        },
        Section {
            title: "Worktree inheritance",
            blocks: vec![
                Block::Mermaid(MERMAID_INHERITANCE),
                para(&[
                    "Every task gets a unique id, a fresh branch `ahu/<agent>/<task-id>`, and a fresh",
                    "worktree in an ahu-managed directory outside your source tree. The worktree starts",
                    "at the base commit, then receives the invoking checkout's complete agent",
                    "configuration as it stands at submission — including uncommitted and Git-ignored",
                    "files, with local deletions honoured. Unrelated dirty source files stay behind.",
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
                    "Hooks are shell commands the harness runs on its own lifecycle events. They are the",
                    "most behaviour-determining thing in a repository: one can block a tool call, and",
                    "another can put arbitrary text into the model's context. They are also often stored",
                    "in Git-ignored directories and never reviewed.",
                ]),
                para(&[
                    "ahu reports them and never writes them. Hooks declared in the repository travel",
                    "into the task worktree with their executable bit intact and run there, which the",
                    "launch preview states explicitly. Hooks configured outside the repository get the",
                    "consistency warning below. Adding or editing hooks on your behalf would be exactly",
                    "the invisible behaviour modification ahu exists to prevent.",
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
            ])],
        },
        Section {
            title: "State and records",
            blocks: vec![para(&[
                "Task records, worktrees, the repository-to-cmux-group mapping, and hygiene",
                "timestamps are local operational state under `$AHU_STATE_DIR` (default",
                "`~/.local/state/ahu`). Policy never lives there. Each record freezes the launched",
                "identity, digests, base commit, branch, worktree, and cmux ids, so editing an agent",
                "later changes the next launch while a running task keeps what it started with.",
            ])],
        },
        Section {
            title: "What ahu will not do",
            blocks: vec![bullets(&[
                "substitute a different harness or model, for any reason",
                "stage, commit, push, stash, reset, clean, or switch branches in your checkout",
                "add, edit, or remove hooks, skills, memories, or instruction files",
                "widen permissions or bypass the harness's own approval boundaries",
                "install, configure, or authenticate a harness on your behalf",
                "delete a worktree, branch, or task record that may hold your work",
                "call an inventory complete, or a behaviour change harmless",
            ])],
        },
        Section {
            title: "Standing warnings",
            blocks: vec![
                Block::Warning {
                    headline: RELIABILITY_WARNING.to_string(),
                    detail: vec![
                        "Claude Code pins the model with --model at launch, but an interactive session"
                            .to_string(),
                        "can change it with /model and ahu has no supported control that prevents"
                            .to_string(),
                        "that. ahu still always requests the configured identity.".to_string(),
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
                    "Compatibility catalog {} ships with this build and is pinned by project",
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
                                    "future work".to_string()
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
        "Fixed identity. A named agent's harness, model, and system prompt are used",
        "as configured. No fallback model, no availability-based substitution, no",
        "task-driven prompt rewriting. Invalid configuration fails before a task starts.",
    ],
    &[
        "One project policy. Rankings, cadence, and the pinned catalog are the same",
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
/// It goes in ahu's own state directory rather than the repository: it is
/// generated documentation about the tool, not part of anyone's project, and ahu
/// does not write into a user's checkout.
pub fn document_path() -> Result<PathBuf> {
    Ok(crate::state::root()?.join("docs/architecture.md"))
}

/// Write the Markdown document to ahu's state directory.
pub fn write_document() -> Result<PathBuf> {
    let path = document_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, markdown())
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
