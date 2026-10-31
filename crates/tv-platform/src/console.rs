//! Lets one EXE serve as both a command-line tool and a GUI. It's built as a
//! console program, so shells wait for its CLI commands. On Windows 11 24H2 and
//! later, its manifest stops Windows allocating a console when it's launched from
//! Explorer. Older Windows ignores that setting and opens one, so it's closed here.

use std::os::windows::process::CommandExt;
use std::process::Command;
use windows::Win32::System::Console::{FreeConsole, GetConsoleProcessList};

const DETACHED_PROCESS: u32 = 0x0000_0008;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConsoleOutcome {
    /// There's no console now, so carry on and start the GUI.
    Continue,
    /// A detached copy was started so the shell isn't left waiting. Exit now.
    Relaunched,
}

fn processes_on_console() -> u32 {
    let mut list = [0u32; 4];
    // SAFETY: the buffer is a local, and its length is passed with it.
    unsafe { GetConsoleProcessList(&mut list) }
}

/// Call before opening the GUI.
pub fn prepare_console_for_gui() -> ConsoleOutcome {
    match processes_on_console() {
        0 => ConsoleOutcome::Continue,
        1 => {
            // This process is the console's only user, so the console was opened
            // just for it. That's older Windows launching from Explorer.
            // SAFETY: takes no arguments and only detaches this process.
            let _ = unsafe { FreeConsole() };
            ConsoleOutcome::Continue
        }
        _ => relaunch_detached(),
    }
}

/// Started from a shell, so relaunch detached rather than make the shell wait
/// until the window closes. If that fails, run here and let the shell wait.
fn relaunch_detached() -> ConsoleOutcome {
    let Ok(exe) = std::env::current_exe() else {
        return ConsoleOutcome::Continue;
    };
    let started = Command::new(exe)
        .args(std::env::args_os().skip(1))
        .creation_flags(DETACHED_PROCESS)
        .spawn();
    match started {
        Ok(_) => ConsoleOutcome::Relaunched,
        Err(_) => ConsoleOutcome::Continue,
    }
}
