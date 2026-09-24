use std::io::IsTerminal;
use std::process::ExitCode;

use ahu::cli::{self, Command, ExplainFormat};
use ahu::commands;
use ahu::style::{self, Role};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = run(args);
    ahu::telemetry::shutdown();
    match result {
        Ok(code) => ExitCode::from(u8::try_from(code).unwrap_or(5)),
        Err(error) => {
            // Error text carries repository-controlled values: file names,
            // symlink targets, configuration values. Escape sequences in them
            // must not reach the terminal raw just because this is the error
            // path rather than a renderer.
            eprintln!(
                "{} {}",
                style::stdout().paint(Role::Error, "ahu:"),
                ahu::util::display_safe_block(&error.to_string())
            );
            ExitCode::from(error.kind() as u8)
        }
    }
}

fn run(args: Vec<String>) -> ahu::util::Result<i32> {
    let (args, repository) = cli::extract_repository(args)?;
    let (args, color) =
        cli::extract_color(args).map_err(|error| error.with_kind(ahu::util::ErrorKind::Usage))?;
    style::configure(color);
    let command = cli::parse_with_stdin(args, !std::io::stdin().is_terminal())?;
    if let Some(repository) = repository {
        std::env::set_current_dir(&repository).map_err(|error| {
            ahu::util::Error::new(format!(
                "cannot select repository {}: {error}",
                repository.display()
            ))
        })?;
    }
    match command {
        Command::Help => {
            println!("{}", cli::HELP);
            Ok(0)
        }
        Command::Version => {
            println!("ahu {}", env!("CARGO_PKG_VERSION"));
            Ok(0)
        }
        // `explain` is documentation. It reads no repository and no configuration.
        Command::Explain { format } => match format {
            ExplainFormat::Terminal => {
                print!("{}", ahu::explain::overview());
                Ok(0)
            }
            ExplainFormat::Markdown => {
                print!("{}", ahu::explain::markdown());
                Ok(0)
            }
            ExplainFormat::Mermaid => {
                print!("{}", ahu::explain::mermaid_only());
                Ok(0)
            }
            ExplainFormat::OpenInCmux => {
                // The document is written either way, so a machine without cmux
                // still ends up with something it can open.
                let path = ahu::explain::write_document()?;
                println!("Wrote {}", path.display());
                match ahu::explain::open_in_cmux(&path, true) {
                    Ok(surface) => {
                        println!("Opened in cmux's Markdown viewer (surface {surface}).");
                        println!("Diagrams render there; the file also renders on GitHub.");
                        Ok(0)
                    }
                    Err(e) => {
                        eprintln!(
                            "{}",
                            style::stdout()
                                .paint(Role::Hint, "The document is still at the path above.")
                        );
                        Err(e)
                    }
                }
            }
        },
        // `run-task` is started by cmux inside the task worktree and works from
        // the task record alone, so it does not need repository discovery.
        Command::RunTask { task_dir } => commands::run_task(&task_dir),
        Command::BatchSupervisor { task_dir } => ahu::headless::supervise(&task_dir),
        Command::BatchControl {
            action,
            task_id,
            prompt,
            json,
        } => {
            let repo = commands::repo_from_cwd()?;
            if action == "cancel" {
                commands::cancel_cmd(&repo, &task_id, json)
            } else {
                ahu::headless::control(&repo, &action, &task_id, prompt.as_deref(), json)
            }
        }
        Command::TasksJson => {
            let repo = commands::repo_from_cwd()?;
            let listing = ahu::task::list(&repo)?;
            let workspaces = commands::liveness_workspaces(&listing.records);
            println!(
                "{}",
                serde_json::to_string(&serde_json::json!({
                    "schema_version": 1,
                    "tasks": listing.records.iter().map(|(dir,r)| commands::task_summary(dir,r,workspaces.as_ref())).collect::<ahu::util::Result<Vec<_>>>()?,
                    "unreadable": listing.unreadable.iter().map(|row| serde_json::json!({"task_id":row.task_id,"reason":row.reason})).collect::<Vec<_>>(),
                    "notes":listing.notes,
                }))?
            );
            Ok(0)
        }
        Command::HeadlessLaunch { launch, options } => {
            let Command::Launch {
                agent,
                prompt,
                display,
                output_json,
                dry_run,
                allow_widened_approvals,
            } = *launch
            else {
                unreachable!()
            };
            let repo = commands::repo_from_cwd()?;
            let prompt = prompt.read(&mut std::io::stdin(), std::io::stdin().is_terminal())?;
            commands::with_stdio_output(output_json, |console| {
                ahu::headless::launch(
                    console,
                    &repo,
                    &agent,
                    &prompt,
                    &display,
                    output_json,
                    dry_run,
                    allow_widened_approvals,
                    options,
                )
            })
        }
        Command::Claude => {
            let repo = commands::repo_from_cwd()?;
            commands::claude(&repo)
        }
        Command::Codex => {
            let repo = commands::repo_from_cwd()?;
            commands::codex(&repo)
        }
        Command::OpenCode => {
            let repo = commands::repo_from_cwd()?;
            commands::opencode(&repo)
        }
        Command::Antigravity => {
            let repo = commands::repo_from_cwd()?;
            commands::antigravity(&repo)
        }
        Command::Launch {
            agent,
            prompt,
            output_json,
            dry_run,
            allow_widened_approvals,
            display,
        } => {
            let repo = commands::repo_from_cwd()?;
            let prompt = prompt.read(&mut std::io::stdin(), std::io::stdin().is_terminal())?;
            commands::with_stdio_output(output_json, |console| {
                commands::launch_cmd(
                    console,
                    &repo,
                    &agent,
                    &prompt,
                    output_json,
                    dry_run,
                    allow_widened_approvals,
                    &display,
                )
            })
        }
        // The JSON report is the contract on stdout, so the readable findings
        // go to stderr when it is asked for, exactly as `launch --output json`.
        Command::KnowledgeLint { output_json } => {
            let repo = commands::repo_from_cwd()?;
            commands::with_stdio_output(output_json, |console| {
                commands::knowledge_lint(console, &repo, output_json)
            })
        }
        Command::CmuxStatus { output_json } => {
            let cwd = std::env::current_dir()?;
            let root = commands::repo_from_cwd().map(|r| r.root).unwrap_or(cwd);
            ahu::cmux::integration::status_command(&root, output_json)
        }
        Command::CmuxInstall { harness, dry_run } => {
            let cwd = std::env::current_dir()?;
            let root = commands::repo_from_cwd().map(|r| r.root).unwrap_or(cwd);
            ahu::cmux::integration::install_command(&root, &harness, dry_run)
        }
        Command::McpServe => {
            let repo = commands::repo_from_cwd()?;
            ahu::mcp::serve(&repo)
        }
        Command::McpSetup => {
            let repo = commands::repo_from_cwd()?;
            ahu::mcp::setup(&repo)
        }
        // `doctor` reports on a missing repository rather than failing on one.
        Command::Doctor => {
            let repo = commands::repo_from_cwd();
            commands::with_stdio(|console| commands::doctor(console, &repo))
        }
        other => {
            let repo = commands::repo_from_cwd()?;
            commands::with_stdio(|console| match other {
                Command::Interactive { focus } => commands::interactive(console, &repo, focus),
                Command::Init => commands::init(console, &repo),
                Command::Agents => commands::agents(console, &repo),
                Command::Onboard {
                    register,
                    remove,
                    model,
                    version,
                } => commands::onboard_cmd(
                    console,
                    &repo,
                    register.as_deref(),
                    remove.as_deref(),
                    model.as_deref(),
                    &version,
                ),
                Command::Inventory { agent } => {
                    commands::inventory_cmd(console, &repo, agent.as_deref())
                }
                Command::Hygiene { agent } => {
                    commands::hygiene_cmd(console, &repo, agent.as_deref())
                }
                Command::Tasks => commands::tasks(console, &repo),
                Command::Task {
                    task_id,
                    output_json,
                } => commands::task_cmd(console, &repo, &task_id, output_json),
                Command::Diff { task_id } => commands::diff_cmd(console, &repo, &task_id),
                Command::Focus { task_id } => commands::focus(console, &repo, &task_id),
                Command::Remove { task_id } => commands::remove_cmd(console, &repo, &task_id),
                Command::Message { task_id, text } => {
                    commands::message_cmd(console, &repo, &task_id, &text)
                }
                Command::Help
                | Command::HeadlessLaunch { .. }
                | Command::BatchControl { .. }
                | Command::BatchSupervisor { .. }
                | Command::TasksJson
                | Command::Version
                | Command::Explain { .. }
                | Command::Doctor
                | Command::CmuxStatus { .. }
                | Command::CmuxInstall { .. }
                | Command::McpServe
                | Command::McpSetup
                | Command::Claude
                | Command::Codex
                | Command::OpenCode
                | Command::Antigravity
                | Command::Launch { .. }
                | Command::KnowledgeLint { .. }
                | Command::RunTask { .. } => unreachable!("handled above"),
            })
        }
    }
}
