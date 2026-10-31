//! The Windows layer: the window, its message loop, input, DPI, clipboard, file
//! dialogs, UI Automation and console handling. Almost all of the app's Windows
//! API calls and `unsafe` code live here, with the rest in `tv-render`.

mod console;
mod input;
pub mod metrics;
mod shell;
mod window;

pub use console::{prepare_console_for_gui, ConsoleOutcome};
pub use shell::show_error;
pub use window::{run, Command, Commands, Handler, WindowContext, WindowOptions};

/// Lets another thread wake the window's message loop. Wakes coalesce, so
/// calling this often is cheap and can't flood the message queue.
#[derive(Clone)]
pub struct Waker {
    hwnd: isize,
    pending: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl Waker {
    pub fn wake(&self) {
        use std::sync::atomic::Ordering;
        if !self.pending.swap(true, Ordering::AcqRel) {
            window::post_wake(self.hwnd);
        }
    }
}
