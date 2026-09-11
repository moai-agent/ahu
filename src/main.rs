use std::{env, process::ExitCode};

const HELP: &str = "ahu - The moai-agent command-line interface

Usage: ahu [COMMAND]

Commands:
  help  Print this help message

Options:
  -h, --help  Print this help message";

fn main() -> ExitCode {
    let mut args = env::args_os().skip(1);
    let first = args.next();
    let is_help = first
        .as_ref()
        .is_none_or(|arg| arg == "help" || arg == "-h" || arg == "--help");

    if is_help && args.next().is_none() {
        println!("{HELP}");
        ExitCode::SUCCESS
    } else {
        eprintln!("error: unsupported arguments\n\nRun 'ahu help' for usage.");
        ExitCode::from(2)
    }
}
