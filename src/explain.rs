//! `ahu explain` — the architecture overview built into the CLI.
//!
//! This exists so the model ahu works to is available where people actually
//! are, without a browser. The Mermaid source is emitted as a fenced block so
//! `ahu explain --mermaid > docs/architecture.md` produces something that
//! renders on GitHub unchanged.

use crate::catalog;
use crate::harness::RELIABILITY_WARNING;
use crate::hooks::NON_PROJECT_HOOK_WARNING;

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

fn banner() -> String {
    format!(
        "ahu {} — architecture overview\n\
         {}\n",
        env!("CARGO_PKG_VERSION"),
        "=".repeat(46)
    )
}

/// Just the diagrams, as fenced Mermaid blocks.
pub fn mermaid_only() -> String {
    let mut out = String::new();
    out.push_str("# ahu architecture\n\n## Launch pipeline\n\n```mermaid\n");
    out.push_str(MERMAID_PIPELINE);
    out.push_str("```\n\n## Prompt transport\n\n```mermaid\n");
    out.push_str(MERMAID_PROMPT);
    out.push_str("```\n\n## Worktree inheritance\n\n```mermaid\n");
    out.push_str(MERMAID_INHERITANCE);
    out.push_str("```\n\n## Context sources\n\n```mermaid\n");
    out.push_str(MERMAID_CONTEXT);
    out.push_str("```\n");
    out
}

/// The full overview: what ahu is, how a launch works, and what it cannot do.
pub fn overview() -> String {
    let mut out = String::new();
    out.push_str(&banner());

    out.push_str(
        "\nWhat ahu is\n-----------\n\
         ahu launches repository-defined agents in isolated Git worktrees and organises\n\
         their sessions in cmux. An agent's harness, model, and instructions live in the\n\
         repository, so changing how it behaves is a reviewable change like any other.\n\
         A launch uses exactly that configuration or it fails.\n",
    );

    out.push_str("\nFour rules everything else follows from\n");
    out.push_str("---------------------------------------\n");
    for (number, lines) in FOUR_RULES.iter().enumerate() {
        for (index, line) in lines.iter().enumerate() {
            if index == 0 {
                out.push_str(&format!("{}. {line}\n", number + 1));
            } else {
                out.push_str(&format!("   {line}\n"));
            }
        }
    }

    out.push_str("\nLaunch pipeline\n---------------\n\n```mermaid\n");
    out.push_str(MERMAID_PIPELINE);
    out.push_str("```\n");

    out.push_str(
        "\nIdentity and selection\n----------------------\n\
         Only `.agents/ahu/agents/<name>.toml` makes an agent launchable. Definitions found\n\
         anywhere else are onboarding candidates, never implicit registrations: a skill is\n\
         not an agent, and AGENTS.md is not an agent registry. A manifest names an explicit\n\
         harness, an exact model identifier (never an alias), and a semantic version, and\n\
         points at a native definition in place. If that native file also declares a model,\n\
         the two must agree — ahu will not rewrite either file or pick one silently.\n\n\
         Without an @agent, ahu walks the project's agreed harness order, then that\n\
         harness's agreed model order, and freezes the result for the task. Local\n\
         prerequisites are checked after the pair is resolved, so a missing installation is\n\
         a diagnostic for that machine, never a different selection for that user.\n",
    );

    out.push_str("\nPrompt transport\n----------------\n\n```mermaid\n");
    out.push_str(MERMAID_PROMPT);
    out.push_str("```\n");
    out.push_str(
        "\nThe cmux startup command is shell-interpreted, so it contains only ahu's own\n\
         executable path and task directory, both single-quoted. The prompt is written to a\n\
         file and handed to the harness as one argument vector element. Shell syntax inside\n\
         a prompt is therefore delivered literally and never evaluated.\n",
    );

    out.push_str("\nWorktree inheritance\n--------------------\n\n```mermaid\n");
    out.push_str(MERMAID_INHERITANCE);
    out.push_str("```\n");
    out.push_str(
        "\nEvery task gets a unique id, a fresh branch `ahu/<agent>/<task-id>`, and a fresh\n\
         worktree in an ahu-managed directory outside your source tree. The worktree starts\n\
         at the base commit, then receives the invoking checkout's complete agent\n\
         configuration as it stands at submission — including uncommitted and Git-ignored\n\
         files, with local deletions honoured. Unrelated dirty source files stay behind.\n\
         Nothing is staged, committed, stashed, or reset in your checkout, ever.\n",
    );

    out.push_str("\nContext sources\n---------------\n\n```mermaid\n");
    out.push_str(MERMAID_CONTEXT);
    out.push_str("```\n");
    out.push_str(
        "\n`ahu inventory` marks each source loaded, available, disabled, opaque, or absent,\n\
         and ends with what ahu cannot see. `available` means the harness can discover a\n\
         source, not that its contents reached the model. The inventory is never labelled\n\
         complete.\n",
    );

    out.push_str(
        "\nHooks\n-----\n\
         Hooks are shell commands the harness runs on its own lifecycle events. They are the\n\
         most behaviour-determining thing in a repository: one can block a tool call, and\n\
         another can put arbitrary text into the model's context. They are also often stored\n\
         in Git-ignored directories and never reviewed.\n\n\
         ahu reports them and never writes them. Hooks declared in the repository travel\n\
         into the task worktree with their executable bit intact and run there, which the\n\
         launch preview states explicitly. Hooks configured outside the repository get the\n\
         consistency warning below, because they change behaviour, do not travel, and can\n\
         differ for every teammate. Adding or editing hooks on your behalf would be exactly\n\
         the invisible behaviour modification ahu exists to prevent.\n",
    );

    out.push_str(
        "\nVersions and drift\n------------------\n\
         Every named agent carries a semantic version. ahu does not automate releases, but it\n\
         will not let a version label quietly cover changed inputs: if `chris@1.2.0` launches\n\
         with different instructions, repository configuration, policy, or hooks than the\n\
         last `chris@1.2.0` launch, that drift is reported as a pending behaviour change for\n\
         the next version bump. It is not classified as safe.\n",
    );

    out.push_str(
        "\nState and records\n-----------------\n\
         Task records, worktrees, the repository-to-cmux-group mapping, and hygiene\n\
         timestamps are local operational state under `$AHU_STATE_DIR` (default\n\
         `~/.local/state/ahu`). Policy never lives there. Each record freezes the launched\n\
         identity, digests, base commit, branch, worktree, and cmux ids, so editing an agent\n\
         later changes the next launch while a running task keeps what it started with.\n",
    );

    out.push_str("\nWhat ahu will not do\n--------------------\n");
    for line in [
        "substitute a different harness or model, for any reason",
        "stage, commit, push, stash, reset, clean, or switch branches in your checkout",
        "add, edit, or remove hooks, skills, memories, or instruction files",
        "widen permissions or bypass the harness's own approval boundaries",
        "delete a worktree, branch, or task record that may hold your work",
        "call an inventory complete, or a behaviour change harmless",
    ] {
        out.push_str(&format!("  - {line}\n"));
    }

    out.push_str("\nStanding warnings\n-----------------\n");
    out.push_str(&format!("  !! {RELIABILITY_WARNING}\n"));
    out.push_str(
        "     Claude Code pins the model with --model at launch, but an interactive session\n\
         \x20    can change it with /model and ahu has no supported control that prevents\n\
         \x20    that. ahu still always requests the configured identity.\n",
    );
    out.push_str(&format!("  !! {NON_PROJECT_HOOK_WARNING}\n"));
    for line in crate::hooks::NON_PROJECT_HOOK_DETAIL {
        out.push_str(&format!("     {line}\n"));
    }

    out.push_str(&format!(
        "\nCompatibility\n-------------\n\
         Compatibility catalog {} ships with this build and is pinned by project\n\
         configuration, so installing a newer ahu cannot silently change which model your\n\
         project selects.\n",
        catalog::CATALOG_VERSION
    ));
    for harness in catalog::HARNESSES {
        out.push_str(&format!(
            "  {:<14} {:<12} {}\n",
            harness.id,
            if harness.adapter_available {
                "adapter"
            } else {
                "future work"
            },
            if harness.verified_versions.is_empty() {
                "not verified".to_string()
            } else {
                format!("verified against {}", harness.verified_versions)
            }
        ));
    }

    out.push_str(
        "\nRun `ahu explain --mermaid` for the diagrams alone, ready to redirect into a\n\
         Markdown file. Run `ahu help` for commands, and `ahu doctor` for this machine.\n",
    );
    out
}
