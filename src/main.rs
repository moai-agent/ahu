use std::io::IsTerminal;
use std::process::ExitCode;

use ahu::cli::{self, Command, ExplainFormat};
use ahu::commands;
use ahu::style::{self, Role};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(args) {
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
    let (args, color) =
        cli::extract_color(args).map_err(|error| error.with_kind(ahu::util::ErrorKind::Usage))?;
    style::configure(color);
    let command = cli::parse_with_stdin(args, !std::io::stdin().is_terminal())?;
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
        Command::Codex => {
            let repo = commands::repo_from_cwd()?;
            commands::codex(&repo)
        }
        Command::Launch {
            agent,
            prompt,
            output_json,
            dry_run,
            allow_widened_approvals,
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
                )
            })
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
                Command::Help
                | Command::Version
                | Command::Explain { .. }
                | Command::Doctor
                | Command::Codex
                | Command::Launch { .. }
                | Command::RunTask { .. } => unreachable!("handled above"),
            })
        }
    }
}
