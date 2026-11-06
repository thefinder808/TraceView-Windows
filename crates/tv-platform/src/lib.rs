//! The Windows layer: the window, its message loop, input, DPI, clipboard, file
//! dialogs, UI Automation and console handling. Almost all of the app's Windows
//! API calls and `unsafe` code live here, with the rest in `tv-render`.

mod console;
mod file_types;
mod input;
pub mod metrics;
mod shell;
mod user_path;
mod window;

pub use console::{prepare_console_for_gui, ConsoleOutcome};
pub use file_types::{registered_file_types, FILE_TYPES};
pub use shell::{clipboard_text, show_error, show_folder};
pub use user_path::UserPath;
pub use window::{run, Command, Commands, Cursor, Handler, WindowContext, WindowOptions};

/// Lets another thread wake the window's message loop. Wakes coalesce, so
/// calling this often is cheap and can't flood the message queue.
#[derive(Clone)]
pub struct Waker {
    hwnd: isize,
    pending: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl Waker {
    /// A waker with no window, for code that runs without one, such as tests.
    /// Waking it does nothing.
    pub fn detached() -> Self {
        Self {
            hwnd: 0,
            pending: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }

    pub fn wake(&self) {
        if self.hwnd == 0 {
            return;
        }
        use std::sync::atomic::Ordering;
        if !self.pending.swap(true, Ordering::AcqRel) {
            window::post_wake(self.hwnd);
        }
    }
}
