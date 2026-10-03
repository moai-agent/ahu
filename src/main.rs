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
        Command::Help { topic } => {
            println!("{}", cli::help_for(topic.as_deref())?);
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
        // The records are outside the checkout by policy, but the repository
        // still has to be located: locating it is what makes that check real.
        Command::EvalReport {
            records,
            output_json,
        } => {
            let repo = commands::repo_from_cwd()?;
            commands::with_stdio_output(output_json, |console| {
                commands::eval_report(console, &repo, &records, output_json)
            })
        }
        Command::EvalRun {
            case,
            suite,
            agents,
            evaluator,
            evaluator_repo,
            decision_evaluator,
            skill_selection,
            records,
            runs,
            timeout_seconds,
            allow_widened_approvals,
            output_json,
        } => {
            let repo = commands::repo_from_cwd()?;
            let request = ahu::eval::RunRequest {
                case: case.as_deref(),
                suite: suite.as_deref(),
                agents: &agents,
                evaluator: evaluator.as_deref(),
                evaluator_repo: evaluator_repo.as_deref(),
                decision_evaluator,
                skill_selection,
                records: &records,
                runs,
                timeout_seconds,
                allow_widened_approvals,
                json_output: output_json,
            };
            commands::with_stdio_output(output_json, |console| {
                ahu::eval::run(console, &repo, &request)
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
        Command::Auth {
            action,
            harness,
            profile,
            replace,
        } => {
            let repo = commands::repo_from_cwd()?;
            let message = match action.as_str() {
                "profiles" => ahu::auth_binding::list_profiles(&repo)?,
                "bind" => match profile.as_deref() {
                    Some(profile) => {
                        ahu::auth_binding::bind_profile(&repo, &harness, profile, replace)?
                    }
                    None => ahu::auth_binding::bind(&repo, &harness, replace)?,
                },
                "status" => match profile.as_deref() {
                    Some(profile) => ahu::auth_binding::status_profile(&repo, &harness, profile)?,
                    None => ahu::auth_binding::status(&repo, &harness)?,
                },
                "select" => ahu::auth_binding::select_profile(
                    &repo,
                    profile.as_deref().expect("validated by parser"),
                )?,
                _ => unreachable!("auth action is validated by the parser"),
            };
            println!("{}", ahu::util::display_safe_block(&message));
            Ok(0)
        }
        Command::Telemetry {
            action,
            record_key,
            task_ref,
            output_json,
        } => {
            let repo = commands::repo_from_cwd()?;
            match action.as_str() {
                "link" => {
                    ahu::telemetry::private_store::link(
                        &repo,
                        &record_key,
                        task_ref.as_deref().expect("validated by parser"),
                    )?;
                    println!(
                        "Linked task to private record key {}.",
                        ahu::util::display_safe(&record_key)
                    );
                    Ok(0)
                }
                "unlink" => {
                    if ahu::telemetry::private_store::unlink(
                        &repo,
                        &record_key,
                        task_ref.as_deref(),
                    )? {
                        println!(
                            "Removed private telemetry association for {}.",
                            ahu::util::display_safe(&record_key)
                        );
                        Ok(0)
                    } else {
                        println!("No matching private telemetry association.");
                        Ok(1)
                    }
                }
                "report" => {
                    let report = ahu::telemetry::private_store::report(&repo, &record_key)?;
                    if output_json {
                        println!("{}", serde_json::to_string_pretty(&report)?);
                    } else {
                        println!(
                            "Private measurement report: {}",
                            ahu::util::display_safe(&record_key)
                        );
                        println!("  linked tasks  {}", report["linked_tasks"]);
                        println!("  missing tasks {}", report["missing_tasks"]);
                        println!(
                            "  no headless results {} task(s)",
                            report["tasks_without_headless_attempt_results"]
                        );
                        println!(
                            "  attempts      {} complete, {} with opt-in metrics",
                            report["completed_attempts"], report["attempts_with_opt_in_metrics"]
                        );
                        for group in report["groups"].as_array().into_iter().flatten() {
                            print_private_report_group(group);
                        }
                    }
                    Ok(0)
                }
                _ => unreachable!("telemetry action is validated by parser"),
            }
        }
        // `doctor` reports on a missing repository rather than failing on one.
        Command::Doctor { verbose } => {
            let repo = commands::repo_from_cwd();
            commands::with_stdio(|console| commands::doctor_with_verbosity(console, &repo, verbose))
        }
        other => {
            let repo = commands::repo_from_cwd()?;
            commands::with_stdio(|console| match other {
                Command::Interactive { focus, agent } => {
                    commands::interactive(console, &repo, focus, agent.as_deref())
                }
                Command::Setup => ahu::setup::run(console, &repo),
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
                Command::Lock { update } => commands::lock_cmd(console, &repo, update),
                Command::Tasks { limit } => commands::tasks_with_limit(console, &repo, limit),
                Command::Task {
                    task_id,
                    output_json,
                } => commands::task_cmd(console, &repo, &task_id, output_json),
                Command::Focus { task_id } => commands::focus(console, &repo, &task_id),
                Command::Remove { task_id } => commands::remove_cmd(console, &repo, &task_id),
                Command::Message { task_id, text } => {
                    commands::message_cmd(console, &repo, &task_id, &text)
                }
                Command::Help { .. }
                | Command::HeadlessLaunch { .. }
                | Command::BatchControl { .. }
                | Command::BatchSupervisor { .. }
                | Command::TasksJson
                | Command::Version
                | Command::Explain { .. }
                | Command::Doctor { .. }
                | Command::CmuxStatus { .. }
                | Command::CmuxInstall { .. }
                | Command::McpServe
                | Command::Auth { .. }
                | Command::Telemetry { .. }
                | Command::Claude
                | Command::Codex
                | Command::OpenCode
                | Command::Antigravity
                | Command::Launch { .. }
                | Command::KnowledgeLint { .. }
                | Command::EvalReport { .. }
                | Command::EvalRun { .. }
                | Command::RunTask { .. } => unreachable!("handled above"),
            })
        }
    }
}

fn print_private_report_group(group: &serde_json::Value) {
    let safe =
        |value: &serde_json::Value| ahu::util::display_safe(value.as_str().unwrap_or("unknown"));
    let number = |value: &serde_json::Value| value.as_u64().unwrap_or(0);
    let attempts = number(&group["attempts"]);
    println!(
        "\n  {} · {} · {} · {} ({} attempt(s))",
        safe(&group["agent"]),
        safe(&group["group"]["harness"]),
        safe(&group["group"]["model"]),
        safe(&group["group"]["outcome"]),
        attempts
    );
    if let Some(mean) = group["elapsed_ms"]["mean_observed_ms"].as_f64() {
        println!(
            "    time   {mean:.0} ms mean ({}/{attempts} observed)",
            number(&group["elapsed_ms"]["observed_attempts"])
        );
    } else {
        println!("    time   unavailable (0/{attempts} observed)");
    }
    for (key, label) in [
        ("ahu.tokens.input", "input"),
        ("ahu.tokens.output", "output"),
        ("ahu.tokens.total", "total"),
    ] {
        let value = &group["values"][key];
        let observed = number(&value["observed_attempts"]);
        match value["maximum_observed"].as_u64() {
            Some(maximum) => {
                println!("    {label:<6} max {maximum} ({observed}/{attempts} observed)")
            }
            None => println!("    {label:<6} unavailable (0/{attempts} observed)"),
        }
    }
    let costs = &group["reported_cost"];
    let means = costs["mean_observed_usd_by_source"]
        .as_object()
        .cloned()
        .unwrap_or_default();
    let observations = costs["observed_attempts_by_source"].as_object();
    if means.is_empty() {
        println!(
            "    cost   unavailable ({} unavailable attempt(s)); no estimate inferred",
            number(&costs["unavailable_attempts"])
        );
    } else {
        for (source, mean) in means {
            let count = observations
                .and_then(|values| values.get(&source))
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0);
            println!(
                "    cost   ${:.6} mean ({count}/{attempts}; {})",
                mean.as_f64().unwrap_or_default(),
                safe(&serde_json::Value::String(source))
            );
        }
    }
}
