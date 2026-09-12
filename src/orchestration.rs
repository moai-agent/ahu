//! Launch guidance shared by all harnesses. It never substitutes an agent's definition.
use crate::harness::LaunchCommand;
use crate::util::{Error, Result};

pub const INSTRUCTIONS: &str = r#"ahu delegation contract (v1)
You are running an ahu-assigned task in a cmux workspace.
Delegation is scoped to the entrypoint: outside ahu, a request for sub-agents or
fan out belongs to that harness's own natively supported sub-agent features and
stays inside that harness. Inside THIS ahu task, "sub-agents" and "fan out" mean
ONLY registered ahu agents, each in its own cmux workspace with its configured
harness and model. Use ahu for ALL delegated work, including parallel reviews or
named specialists. Native harness sub-agents are not valid ahu child agents.
Run "$AHU_BIN" agents to discover the registered agents. Their manifests specify
which harness and exact model must execute each assignment.
For each assignment, write its complete task to a UTF-8 file, then run:
  "$AHU_BIN" launch @name --prompt-file /absolute/path/to/task.txt
This command launches immediately, without confirmation, in a separate cmux
workspace under the repository group. Use --dry-run to inspect a launch first.
Run from your task worktree so the child's base is your HEAD and its configuration
comes from this checkout. Children get fresh worktrees; uncommitted source edits
are NOT copied. For a review of uncommitted work, explicitly include the absolute
source checkout path and revision/diff scope in the assignment. Tell reviewers
where to write their findings so you can read and compare their actual reports.
Record each returned task id, worktree, and cmux workspace. Use "$AHU_BIN" tasks
and cmux read-screen --workspace <workspace-id> --scrollback to inspect progress.
A session launch or process exit does not prove completion; read the findings.
Never read an agent definition and impersonate it using this harness's built-in
Agent, Task, team, spawn_agent, or similar delegation tools. Never launch a
replacement harness yourself. Never replace a configured harness or model with
the coordinator's harness/model, even if a launch fails. Report the blocker.
If an agent is unregistered, unavailable, or cannot launch with its configured
identity, stop that assignment and report the reason. Do not silently fall back.
These instructions apply recursively to every child launched through ahu.
"#;

/// Keep Claude's task prompt literal and append a separate system instruction.
/// Other adapters receive explicit launcher guidance before the unchanged task text.
pub fn configure(mut command: LaunchCommand) -> Result<LaunchCommand> {
    let index = command
        .prompt_arg
        .ok_or_else(|| Error::new("harness has no task prompt slot"))?;
    if command.program == "claude" {
        let separator = index
            .checked_sub(1)
            .filter(|i| command.args[*i] == "--")
            .ok_or_else(|| Error::new("Claude command is missing its option separator"))?;
        command.args.splice(
            separator..separator,
            [
                "--append-system-prompt".to_string(),
                INSTRUCTIONS.to_string(),
                "--disallowedTools".to_string(),
                "Agent,Task,TeamCreate".to_string(),
            ],
        );
        command.prompt_arg = Some(index + 4);
    } else {
        command.args[index] = format!("{INSTRUCTIONS}\nAssigned task:\n{}", command.args[index]);
    }
    Ok(command)
}
