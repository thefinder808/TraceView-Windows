//! `traceview`: one EXE with several front ends over the same engine. Subcommands
//! run the headless CLI. Otherwise it opens the viewer.

#![forbid(unsafe_code)]

mod cli;
mod gui;

use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();

    #[cfg(feature = "bench")]
    if args.first().is_some_and(|arg| arg == "--gui-bench") {
        return gui::run_bench(&args[1..]);
    }

    match args.as_slice() {
        [] => gui::run(None),
        [path] if !path.starts_with('-') && !cli::is_command(path) => {
            let path = PathBuf::from(path);
            if path.exists() {
                gui::run(Some(path))
            } else {
                // A typo in a shell should say so, not open a window.
                cli::run(args.into_iter())
            }
        }
        _ => cli::run(args.into_iter()),
    }
}
