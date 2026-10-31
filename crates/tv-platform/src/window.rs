//! The window and its message loop. The app implements [`Handler`] and asks for
//! things through [`Commands`], which run after the handler returns. That way,
//! nothing that pumps messages, like a modal dialog, happens while the app's state
//! is borrowed, so re-entrant window messages can't conflict with it.

use crate::{input, shell, Waker};
use accesskit::{ActionHandler, ActionRequest, ActivationHandler, TreeUpdate};
use accesskit_windows::Adapter;
use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use tv_ui::{Color, Event, MouseButton, Point};
use windows::core::{w, Error, Result, HSTRING};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{BeginPaint, EndPaint, InvalidateRect, PAINTSTRUCT};
use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{AdjustWindowRectExForDpi, GetDpiForWindow};
use windows::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture};
use windows::Win32::UI::Shell::{DragAcceptFiles, HDROP};
use windows::Win32::UI::WindowsAndMessaging::{
    ChangeWindowMessageFilterEx, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW,
    GetClientRect, GetMessageW, GetWindowLongPtrW, LoadCursorW, PostMessageW, PostQuitMessage,
    RegisterClassW, SetWindowLongPtrW, SetWindowPos, SetWindowTextW, ShowWindow, TranslateMessage,
    CREATESTRUCTW, CW_USEDEFAULT, GWLP_USERDATA, IDC_ARROW, MSG, MSGFLT_ALLOW, SWP_NOACTIVATE,
    SWP_NOMOVE, SWP_NOZORDER, SW_SHOW, WM_APP, WM_CLOSE, WM_COPYDATA, WM_DESTROY, WM_DPICHANGED,
    WM_DROPFILES, WM_ERASEBKGND, WM_GETOBJECT, WM_KEYDOWN, WM_KILLFOCUS, WM_LBUTTONDOWN,
    WM_LBUTTONUP, WM_MBUTTONDOWN, WM_MBUTTONUP, WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_NCCREATE, WM_PAINT,
    WM_RBUTTONDOWN, WM_RBUTTONUP, WM_SETFOCUS, WM_SETTINGCHANGE, WM_SIZE, WM_SYSKEYDOWN, WNDCLASSW,
    WS_EX_ACCEPTFILES, WS_OVERLAPPEDWINDOW,
};

const WM_WAKE: u32 = WM_APP + 1;
/// The message UIPI uses to ask a window whether a file drop is allowed.
const WM_COPYGLOBALDATA: u32 = 0x0049;

pub struct WindowOptions {
    pub title: String,
    pub width: f32,
    pub height: f32,
}

/// What an app gets when its window is created.
pub struct WindowContext {
    pub hwnd: isize,
    pub width: u32,
    pub height: u32,
    pub dpi: f32,
    pub system_is_dark: bool,
    pub waker: Waker,
}

pub enum Command {
    Redraw,
    OpenFileDialog,
    SetTitle(String),
    SetClipboard(String),
    SetCaptionColors {
        background: Color,
        text: Color,
        dark: bool,
    },
    Close,
}

#[derive(Default)]
pub struct Commands {
    list: Vec<Command>,
    accessibility_changed: bool,
}

impl Commands {
    pub fn push(&mut self, command: Command) {
        self.list.push(command);
    }

    pub fn redraw(&mut self) {
        self.push(Command::Redraw);
    }

    /// Tells screen readers that what's on screen changed.
    pub fn accessibility_changed(&mut self) {
        self.accessibility_changed = true;
    }
}

pub trait Handler {
    fn event(&mut self, event: Event, commands: &mut Commands);
    /// Draws the window. Takes no commands, because drawing changes no state.
    fn paint(&mut self, commands: &mut Commands);
    /// The window's size in physical pixels, or its DPI, changed.
    fn resized(&mut self, width: u32, height: u32, dpi: f32, commands: &mut Commands);
    /// Another thread called [`Waker::wake`].
    fn woken(&mut self, commands: &mut Commands);
    fn files_chosen(&mut self, paths: Vec<PathBuf>, commands: &mut Commands);
    fn system_theme_changed(&mut self, dark: bool, commands: &mut Commands);
    fn accessibility_tree(&mut self) -> TreeUpdate;
    fn accessibility_action(&mut self, request: ActionRequest, commands: &mut Commands);
}

/// Everything the window procedure needs. It's kept behind a raw pointer in the
/// window's user data, so each part has its own `RefCell`.
struct State {
    hwnd: Cell<HWND>,
    handler: RefCell<Option<Box<dyn Handler>>>,
    adapter: RefCell<Option<Adapter>>,
    actions: Receiver<ActionRequest>,
    wake_pending: Arc<AtomicBool>,
    scale: Cell<f32>,
}

/// Runs on UI Automation's threads, so it only queues requests for the UI thread.
struct ActionQueue {
    sender: Sender<ActionRequest>,
    waker: Waker,
}

impl ActionHandler for ActionQueue {
    fn do_action(&mut self, request: ActionRequest) {
        let _ = self.sender.send(request);
        self.waker.wake();
    }
}

/// A tree built ahead of time, so AccessKit's response holds no borrow of the
/// app's state while Windows calls back into this window.
struct InitialTree(Option<TreeUpdate>);

impl ActivationHandler for InitialTree {
    fn request_initial_tree(&mut self) -> Option<TreeUpdate> {
        self.0.take()
    }
}

pub(crate) fn post_wake(hwnd: isize) {
    // SAFETY: posting to a stale handle fails harmlessly, and the failure means
    // the window is gone, so nothing needs waking.
    let _ = unsafe { PostMessageW(Some(HWND(hwnd as *mut _)), WM_WAKE, WPARAM(0), LPARAM(0)) };
}

fn dpi_of(hwnd: HWND) -> f32 {
    // SAFETY: the handle is this window's, and the call only reads it.
    (unsafe { GetDpiForWindow(hwnd) }) as f32
}

fn client_pixels(hwnd: HWND) -> (u32, u32) {
    let mut client = RECT::default();
    // SAFETY: the handle is this window's, and client is a local.
    let _ = unsafe { GetClientRect(hwnd, &mut client) };
    (client.right.max(0) as u32, client.bottom.max(0) as u32)
}

fn low_word(value: isize) -> i16 {
    (value & 0xFFFF) as u16 as i16
}

fn high_word(value: isize) -> i16 {
    ((value >> 16) & 0xFFFF) as u16 as i16
}

impl State {
    fn point_from(&self, lparam: LPARAM) -> Point {
        let scale = self.scale.get();
        Point::new(
            f32::from(low_word(lparam.0)) / scale,
            f32::from(high_word(lparam.0)) / scale,
        )
    }

    /// Calls the handler and carries out what it asks for. If the handler is
    /// already running, which can only be a re-entrant message, it's skipped.
    fn with_handler(&self, call: impl FnOnce(&mut dyn Handler, &mut Commands)) {
        let mut commands = Commands::default();
        {
            let Ok(mut slot) = self.handler.try_borrow_mut() else {
                return;
            };
            let Some(handler) = slot.as_mut() else {
                return;
            };
            call(handler.as_mut(), &mut commands);
        }
        self.apply(commands);
    }

    fn update_accessibility(&self) {
        let queued = {
            let Ok(mut slot) = self.handler.try_borrow_mut() else {
                return;
            };
            let (Some(handler), Ok(mut adapter)) = (slot.as_mut(), self.adapter.try_borrow_mut())
            else {
                return;
            };
            adapter
                .as_mut()
                .and_then(|adapter| adapter.update_if_active(|| handler.accessibility_tree()))
        };
        // Raising events can send messages back to this window, so nothing is
        // borrowed by this point.
        if let Some(queued) = queued {
            queued.raise();
        }
    }

    fn apply(&self, commands: Commands) {
        let Commands {
            list,
            accessibility_changed,
        } = commands;
        for command in list {
            match command {
                Command::Redraw => {
                    // SAFETY: the handle is this window's.
                    let _ = unsafe { InvalidateRect(Some(self.hwnd.get()), None, false) };
                }
                Command::OpenFileDialog => {
                    if let Some(path) = shell::choose_file(self.hwnd.get()) {
                        self.with_handler(|handler, commands| {
                            handler.files_chosen(vec![path], commands);
                        });
                    }
                }
                Command::SetTitle(title) => {
                    // SAFETY: the handle is this window's and the string outlives the call.
                    let _ = unsafe { SetWindowTextW(self.hwnd.get(), &HSTRING::from(title)) };
                }
                Command::SetClipboard(text) => shell::set_clipboard_text(self.hwnd.get(), &text),
                Command::SetCaptionColors {
                    background,
                    text,
                    dark,
                } => shell::set_caption_colors(self.hwnd.get(), background, text, dark),
                Command::Close => {
                    // SAFETY: the handle is this window's.
                    let _ = unsafe { DestroyWindow(self.hwnd.get()) };
                    return;
                }
            }
        }
        if accessibility_changed {
            self.update_accessibility();
        }
    }

    fn set_window_focus(&self, focused: bool) {
        let queued = self.adapter.try_borrow_mut().ok().and_then(|mut adapter| {
            adapter
                .as_mut()
                .and_then(|adapter| adapter.update_window_focus_state(focused))
        });
        if let Some(queued) = queued {
            queued.raise();
        }
    }

    /// Answers a screen reader's request for this window's accessibility object.
    /// Returns `None` to leave the message to Windows.
    fn get_object(&self, wparam: WPARAM, lparam: LPARAM) -> Option<LRESULT> {
        let tree = {
            let mut slot = self.handler.try_borrow_mut().ok()?;
            slot.as_mut()?.accessibility_tree()
        };
        let response = {
            let mut adapter = self.adapter.try_borrow_mut().ok()?;
            adapter
                .as_mut()?
                .handle_wm_getobject(wparam, lparam, &mut InitialTree(Some(tree)))
        };
        // Answering can send nested messages, so nothing is borrowed by this point.
        response.map(Into::into)
    }

    fn resized(&self) {
        let (width, height) = client_pixels(self.hwnd.get());
        let dpi = dpi_of(self.hwnd.get());
        self.scale.set(dpi / 96.0);
        self.with_handler(|handler, commands| {
            handler.resized(width, height, dpi, commands);
            commands.redraw();
        });
    }

    fn mouse_down(&self, lparam: LPARAM, button: MouseButton) {
        let point = self.point_from(lparam);
        let modifiers = input::modifiers();
        // SAFETY: the handle is this window's. Capture keeps drags alive outside it.
        unsafe { SetCapture(self.hwnd.get()) };
        self.with_handler(|handler, commands| {
            handler.event(Event::MouseDown(point, button, modifiers), commands);
        });
    }

    fn mouse_up(&self, lparam: LPARAM, button: MouseButton) {
        let point = self.point_from(lparam);
        // SAFETY: takes no arguments, and only releases this thread's capture.
        let _ = unsafe { ReleaseCapture() };
        self.with_handler(|handler, commands| {
            handler.event(Event::MouseUp(point, button), commands);
        });
    }
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if message == WM_NCCREATE {
        // SAFETY: for this message, lparam is the CREATESTRUCTW holding the State
        // pointer `run` passed in, and `run` keeps that State alive for as long
        // as this window exists.
        unsafe {
            let create = &*(lparam.0 as *const CREATESTRUCTW);
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, create.lpCreateParams as isize);
        }
    }
    // SAFETY: the user data is either zero or the live State pointer set above.
    let state = unsafe { (GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const State).as_ref() };
    let Some(state) = state else {
        // SAFETY: forwards this message unchanged.
        return unsafe { DefWindowProcW(hwnd, message, wparam, lparam) };
    };
    if message == WM_NCCREATE {
        state.hwnd.set(hwnd);
    }

    match message {
        WM_PAINT => {
            let mut paint = PAINTSTRUCT::default();
            // SAFETY: BeginPaint and EndPaint are paired, with a local struct.
            unsafe {
                BeginPaint(hwnd, &mut paint);
                let _ = EndPaint(hwnd, &paint);
            }
            state.with_handler(|handler, commands| handler.paint(commands));
            return LRESULT(0);
        }
        // The renderer covers every pixel, so erasing would only cause flicker.
        WM_ERASEBKGND => return LRESULT(1),
        WM_SIZE => {
            state.resized();
            return LRESULT(0);
        }
        WM_DPICHANGED => {
            // SAFETY: for this message, lparam points to Windows' suggested RECT.
            unsafe {
                let suggested = &*(lparam.0 as *const RECT);
                let _ = SetWindowPos(
                    hwnd,
                    None,
                    suggested.left,
                    suggested.top,
                    suggested.right - suggested.left,
                    suggested.bottom - suggested.top,
                    SWP_NOZORDER | SWP_NOACTIVATE,
                );
            }
            state.resized();
            return LRESULT(0);
        }
        WM_KEYDOWN | WM_SYSKEYDOWN => {
            if let Some(key) = input::key_from_virtual(wparam.0 as u16) {
                let modifiers = input::modifiers();
                state.with_handler(|handler, commands| {
                    handler.event(Event::KeyDown(key, modifiers), commands);
                });
                if message == WM_KEYDOWN {
                    return LRESULT(0);
                }
            }
        }
        WM_LBUTTONDOWN => {
            state.mouse_down(lparam, MouseButton::Left);
            return LRESULT(0);
        }
        WM_RBUTTONDOWN => {
            state.mouse_down(lparam, MouseButton::Right);
            return LRESULT(0);
        }
        WM_MBUTTONDOWN => {
            state.mouse_down(lparam, MouseButton::Middle);
            return LRESULT(0);
        }
        WM_LBUTTONUP => {
            state.mouse_up(lparam, MouseButton::Left);
            return LRESULT(0);
        }
        WM_RBUTTONUP => {
            state.mouse_up(lparam, MouseButton::Right);
            return LRESULT(0);
        }
        WM_MBUTTONUP => {
            state.mouse_up(lparam, MouseButton::Middle);
            return LRESULT(0);
        }
        WM_MOUSEMOVE => {
            let point = state.point_from(lparam);
            state.with_handler(|handler, commands| {
                handler.event(Event::MouseMove(point), commands);
            });
            return LRESULT(0);
        }
        WM_MOUSEWHEEL => {
            let rows = input::wheel_rows(high_word(wparam.0 as isize));
            state.with_handler(|handler, commands| handler.event(Event::Wheel(rows), commands));
            return LRESULT(0);
        }
        WM_DROPFILES => {
            let paths = shell::dropped_files(HDROP(wparam.0 as *mut _));
            state.with_handler(|handler, commands| handler.files_chosen(paths, commands));
            return LRESULT(0);
        }
        WM_SETTINGCHANGE => {
            let dark = shell::system_is_dark();
            state.with_handler(|handler, commands| handler.system_theme_changed(dark, commands));
        }
        WM_SETFOCUS => state.set_window_focus(true),
        WM_KILLFOCUS => state.set_window_focus(false),
        WM_GETOBJECT => {
            if let Some(result) = state.get_object(wparam, lparam) {
                return result;
            }
        }
        WM_WAKE => {
            state.wake_pending.store(false, Ordering::Release);
            while let Ok(request) = state.actions.try_recv() {
                state.with_handler(|handler, commands| {
                    handler.accessibility_action(request, commands);
                });
            }
            state.with_handler(|handler, commands| handler.woken(commands));
            return LRESULT(0);
        }
        WM_CLOSE => {
            // SAFETY: the handle is this window's.
            let _ = unsafe { DestroyWindow(hwnd) };
            return LRESULT(0);
        }
        WM_DESTROY => {
            // SAFETY: takes an exit code, and only posts WM_QUIT to this thread.
            unsafe { PostQuitMessage(0) };
            return LRESULT(0);
        }
        _ => {}
    }
    // SAFETY: forwards this message unchanged.
    unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
}

/// An elevated app doesn't get file drops from a normal Explorer window unless it
/// allows these messages. Admin tools often run elevated, so this matters here.
fn allow_drops_when_elevated(hwnd: HWND) {
    for message in [WM_DROPFILES, WM_COPYDATA, WM_COPYGLOBALDATA] {
        // SAFETY: the handle is this window's, and the extra info is optional.
        let _ = unsafe { ChangeWindowMessageFilterEx(hwnd, message, MSGFLT_ALLOW, None) };
    }
}

/// Opens the window and runs until it closes. `make_handler` runs once the window
/// exists but before it's shown, so the first thing a person sees is a real frame.
pub fn run(
    options: &WindowOptions,
    make_handler: impl FnOnce(WindowContext) -> std::result::Result<Box<dyn Handler>, String>,
) -> Result<()> {
    // SAFETY: first COM use on this thread. The dialogs need this apartment type.
    unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()? };
    let class_name = w!("TraceViewWindow");
    // SAFETY: registers a class with static strings and a valid window procedure.
    let instance = unsafe {
        let instance = GetModuleHandleW(None)?;
        let class = WNDCLASSW {
            lpfnWndProc: Some(window_proc),
            hInstance: instance.into(),
            lpszClassName: class_name,
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            ..Default::default()
        };
        if RegisterClassW(&class) == 0 {
            return Err(Error::from_thread());
        }
        instance
    };

    let (action_sender, actions) = channel();
    let wake_pending = Arc::new(AtomicBool::new(false));
    // Boxed so its address stays put, because the window keeps a pointer to it.
    // It's dropped only after the message loop ends and the window is gone.
    let state = Box::new(State {
        hwnd: Cell::new(HWND::default()),
        handler: RefCell::new(None),
        adapter: RefCell::new(None),
        actions,
        wake_pending: wake_pending.clone(),
        scale: Cell::new(1.0),
    });
    let state_pointer: *const State = &raw const *state;

    // SAFETY: the class is registered, and the State pointer is valid as above.
    let hwnd = unsafe {
        CreateWindowExW(
            WS_EX_ACCEPTFILES,
            class_name,
            &HSTRING::from(options.title.as_str()),
            WS_OVERLAPPEDWINDOW,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            None,
            None,
            Some(instance.into()),
            Some(state_pointer.cast()),
        )?
    };

    let dpi = dpi_of(hwnd);
    let scale = dpi / 96.0;
    let mut frame = RECT {
        left: 0,
        top: 0,
        right: (options.width * scale) as i32,
        bottom: (options.height * scale) as i32,
    };
    // SAFETY: the handle is live, and the rectangle is a local.
    unsafe {
        let _ = AdjustWindowRectExForDpi(
            &mut frame,
            WS_OVERLAPPEDWINDOW,
            false,
            WS_EX_ACCEPTFILES,
            dpi as u32,
        );
        let _ = SetWindowPos(
            hwnd,
            None,
            0,
            0,
            frame.right - frame.left,
            frame.bottom - frame.top,
            SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
        );
        DragAcceptFiles(hwnd, true);
    }
    allow_drops_when_elevated(hwnd);

    let (width, height) = client_pixels(hwnd);
    let waker = Waker {
        hwnd: hwnd.0 as isize,
        pending: wake_pending,
    };
    let handler = make_handler(WindowContext {
        hwnd: hwnd.0 as isize,
        width,
        height,
        dpi,
        system_is_dark: shell::system_is_dark(),
        waker: waker.clone(),
    })
    .map_err(|message| {
        // SAFETY: the handle is live, and the window isn't shown yet.
        let _ = unsafe { DestroyWindow(hwnd) };
        Error::new(windows::core::HRESULT(-1), message.as_str())
    })?;

    let adapter = Adapter::new(
        hwnd,
        false,
        ActionQueue {
            sender: action_sender,
            waker,
        },
    );
    *state.adapter.borrow_mut() = Some(adapter);
    *state.handler.borrow_mut() = Some(handler);
    state.scale.set(scale);

    state.with_handler(|handler, commands| {
        handler.resized(width, height, dpi, commands);
        handler.paint(commands);
    });
    // SAFETY: the handle is live.
    let _ = unsafe { ShowWindow(hwnd, SW_SHOW) };

    let mut message = MSG::default();
    // SAFETY: the message struct is a local. GetMessageW sleeps until there's
    // work, which is why the app uses no CPU while idle.
    unsafe {
        while GetMessageW(&mut message, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
    }
    drop(state);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn word_extraction_keeps_sign() {
        let lparam = ((-5i16 as u16 as isize) << 16) | (300i16 as u16 as isize);
        assert_eq!(low_word(lparam), 300);
        assert_eq!(high_word(lparam), -5);
    }
}
