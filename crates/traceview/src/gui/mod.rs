//! The viewer. Almost everything lives in `workspace` and the modules under it,
//! which need no window, so most of it can be tested without a display. `app`
//! only connects the workspace to the window and renderer.

mod accessibility;
mod app;
#[cfg(feature = "bench")]
mod bench;
mod details;
mod log_view;
mod lookup;
mod row_cache;
mod search;
mod settings;
#[cfg(test)]
mod snapshots;
mod tab;
mod watch;
mod workspace;

use std::path::PathBuf;
use std::process::ExitCode;
use tv_platform::{prepare_console_for_gui, ConsoleOutcome, WindowOptions};

fn window_options() -> WindowOptions {
    WindowOptions {
        title: "TraceView".to_string(),
        width: 1200.0,
        height: 760.0,
    }
}

pub fn run(path: Option<PathBuf>) -> ExitCode {
    if prepare_console_for_gui() == ConsoleOutcome::Relaunched {
        return ExitCode::SUCCESS;
    }
    let result = tv_platform::run(&window_options(), |context| {
        app::App::create(context, path, None).map(|app| Box::new(app) as _)
    });
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            tv_platform::show_error(&format!("TraceView couldn't start: {error}"));
            ExitCode::FAILURE
        }
    }
}

#[cfg(feature = "bench")]
pub fn run_bench(args: &[String]) -> ExitCode {
    let Some(options) = bench::Options::parse(args) else {
        eprintln!(
            "usage: traceview --gui-bench <report.json> [log] [--frames N] [--launch-only] [--window WxH] [--presentation warp|hardware|software]"
        );
        return ExitCode::from(2);
    };
    if prepare_console_for_gui() == ConsoleOutcome::Relaunched {
        return ExitCode::SUCCESS;
    }
    let window = WindowOptions {
        width: options.window.0,
        height: options.window.1,
        ..window_options()
    };
    let path = options.log.clone();
    let report = options.report.clone();
    let result = tv_platform::run(&window, |context| {
        app::App::create(context, path, Some(options)).map(|app| Box::new(app) as _)
    });
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            let _ = std::fs::write(report.with_extension("err"), error.to_string());
            ExitCode::FAILURE
        }
    }
}
