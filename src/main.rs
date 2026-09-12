use std::process::ExitCode;

use ahu::cli::{self, Command, ExplainFormat};
use ahu::commands;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(args) {
        Ok(code) => ExitCode::from(u8::try_from(code).unwrap_or(1)),
        Err(error) => {
            // Error text carries repository-controlled values: file names,
            // symlink targets, configuration values. Escape sequences in them
            // must not reach the terminal raw just because this is the error
            // path rather than a renderer.
            eprintln!("ahu: {}", ahu::util::display_safe_block(&error.to_string()));
            ExitCode::from(2)
        }
    }
}

fn run(args: Vec<String>) -> ahu::util::Result<i32> {
    let command = cli::parse(args)?;
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
                        eprintln!("ahu: could not open it in cmux: {e}");
                        eprintln!("The document is still at the path above.");
                        Ok(1)
                    }
                }
            }
        },
        // `run-task` is started by cmux inside the task worktree and works from
        // the task record alone, so it does not need repository discovery.
        Command::RunTask { task_dir } => commands::run_task(&task_dir),
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
                Command::Focus { task_id } => commands::focus(console, &repo, &task_id),
                Command::Help
                | Command::Version
                | Command::Explain { .. }
                | Command::Doctor
                | Command::RunTask { .. } => unreachable!("handled above"),
            })
        }
    }
}
