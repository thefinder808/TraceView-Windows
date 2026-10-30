//! Comparison probe: draws the same grid as the egui app with Direct2D and
//! DirectWrite, using the same engine and scroll script.
//!
//! Usage: d2d-probe [log] --bench out.json [--software] [--offscreen] [--light] [--launch-only]
//!                  [--frames N] [--window WxH] [--flip hardware|warp] [--scroll-hints]
//!                  [--stage NAME] [--paced] [--capture]

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::PathBuf;
use std::sync::atomic::AtomicU64;
use std::time::Instant;
use tv_engine::{Level, LogFile};
use windows::core::{w, Interface, Result};
use windows::Win32::Foundation::{FILETIME, HMODULE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Direct2D::Common::{
    D2D1_ALPHA_MODE_IGNORE, D2D1_COLOR_F, D2D1_PIXEL_FORMAT, D2D_RECT_F, D2D_SIZE_U,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1CreateFactory, ID2D1Factory, ID2D1Factory1, ID2D1RenderTarget, ID2D1SolidColorBrush,
    D2D1_BITMAP_OPTIONS_CANNOT_DRAW, D2D1_BITMAP_OPTIONS_TARGET, D2D1_BITMAP_PROPERTIES1,
    D2D1_DEVICE_CONTEXT_OPTIONS_NONE, D2D1_DRAW_TEXT_OPTIONS_CLIP,
    D2D1_FACTORY_TYPE_SINGLE_THREADED, D2D1_HWND_RENDER_TARGET_PROPERTIES,
    D2D1_PRESENT_OPTIONS_IMMEDIATELY, D2D1_RENDER_TARGET_PROPERTIES,
    D2D1_RENDER_TARGET_TYPE_DEFAULT, D2D1_RENDER_TARGET_TYPE_SOFTWARE,
    D2D1_TEXT_ANTIALIAS_MODE_CLEARTYPE,
};
use windows::Win32::Graphics::Direct3D::{D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_WARP};
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_SDK_VERSION,
};
use windows::Win32::Graphics::DirectWrite::{
    DWriteCreateFactory, IDWriteFactory, IDWriteTextFormat, DWRITE_FACTORY_TYPE_SHARED,
    DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT_NORMAL,
    DWRITE_MEASURING_MODE_GDI_CLASSIC, DWRITE_TEXT_ALIGNMENT_TRAILING,
    DWRITE_WORD_WRAPPING_NO_WRAP,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_ALPHA_MODE_IGNORE, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC,
};
use windows::Win32::Graphics::Dxgi::{
    IDXGIDevice, IDXGIFactory2, IDXGISurface, IDXGISwapChain1, DXGI_PRESENT,
    DXGI_PRESENT_PARAMETERS, DXGI_SWAP_CHAIN_DESC1, DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL,
    DXGI_USAGE_RENDER_TARGET_OUTPUT,
};
use windows::Win32::Graphics::Gdi::{
    BitBlt, ClientToScreen, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject,
    GetDC, GetDIBits, ReleaseDC, SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB,
    DIB_RGB_COLORS, SRCCOPY,
};
use windows::Win32::Graphics::Imaging::{
    CLSID_WICImagingFactory, GUID_WICPixelFormat32bppBGR, IWICBitmap, IWICImagingFactory,
    WICBitmapCacheOnLoad,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::ProcessStatus::{K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
use windows::Win32::System::SystemInformation::GetSystemTimeAsFileTime;
use windows::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};
use windows::Win32::UI::HiDpi::{
    SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AdjustWindowRect, CreateWindowExW, DefWindowProcW, DispatchMessageW, GetClientRect,
    LoadCursorW, PeekMessageW, PostQuitMessage, RegisterClassW, ShowWindow, TranslateMessage,
    CW_USEDEFAULT, IDC_ARROW, MSG, PM_REMOVE, SW_SHOW, WINDOW_EX_STYLE, WM_DESTROY, WM_QUIT,
    WNDCLASSW, WS_EX_TOPMOST, WS_OVERLAPPEDWINDOW,
};

const FONT_SIZE: f32 = 13.0;
const ROW_HEIGHT: f32 = 18.0;
const TOP_BAR: f32 = 26.0;
const STATUS_BAR: f32 = 22.0;
const SCROLLBAR: f32 = 14.0;

struct Palette {
    background: D2D1_COLOR_F,
    text: D2D1_COLOR_F,
    muted: D2D1_COLOR_F,
    header: D2D1_COLOR_F,
    warning: D2D1_COLOR_F,
    error: D2D1_COLOR_F,
    critical: D2D1_COLOR_F,
    scrollbar: D2D1_COLOR_F,
    thumb: D2D1_COLOR_F,
}

const fn rgb(value: u32) -> D2D1_COLOR_F {
    D2D1_COLOR_F {
        r: ((value >> 16) & 0xFF) as f32 / 255.0,
        g: ((value >> 8) & 0xFF) as f32 / 255.0,
        b: (value & 0xFF) as f32 / 255.0,
        a: 1.0,
    }
}

const CONSOLE: Palette = Palette {
    background: rgb(0x121418),
    text: rgb(0xDCDFE4),
    muted: rgb(0x8A909A),
    header: rgb(0x20232A),
    warning: rgb(0x4A3C10),
    error: rgb(0x551E22),
    critical: rgb(0x7A141C),
    scrollbar: rgb(0x1A1C21),
    thumb: rgb(0x555B66),
};

const LIGHT: Palette = Palette {
    background: rgb(0xFFFFFF),
    text: rgb(0x000000),
    muted: rgb(0x606060),
    header: rgb(0xECECEC),
    warning: rgb(0xFFF28C),
    error: rgb(0xFF9C9C),
    critical: rgb(0xFF7070),
    scrollbar: rgb(0xF2F2F2),
    thumb: rgb(0xB0B0B0),
};

struct Options {
    path: Option<PathBuf>,
    bench: PathBuf,
    software: bool,
    light: bool,
    launch_only: bool,
    window: (i32, i32),
    frames: usize,
    offscreen: bool,
    flip: Option<Device>,
    scroll_hints: bool,
    stage: Option<String>,
    paced: bool,
    capture: bool,
}

#[derive(Clone, Copy, PartialEq)]
enum Device {
    Hardware,
    Warp,
}

fn parse_args() -> Option<Options> {
    let mut options = Options {
        path: None,
        bench: PathBuf::new(),
        software: false,
        light: false,
        launch_only: false,
        window: (1400, 850),
        frames: 600,
        offscreen: false,
        flip: None,
        scroll_hints: false,
        stage: None,
        paced: false,
        capture: false,
    };
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--bench" => options.bench = PathBuf::from(args.next()?),
            "--software" => options.software = true,
            "--offscreen" => options.offscreen = true,
            "--flip" => {
                options.flip = match args.next()?.as_str() {
                    "hardware" => Some(Device::Hardware),
                    "warp" => Some(Device::Warp),
                    _ => return None,
                }
            }
            "--scroll-hints" => options.scroll_hints = true,
            "--stage" => options.stage = Some(args.next()?),
            "--paced" => options.paced = true,
            "--capture" => options.capture = true,
            "--light" => options.light = true,
            "--launch-only" => options.launch_only = true,
            "--frames" => options.frames = args.next()?.parse().ok()?,
            "--window" => {
                let size = args.next()?;
                let (width, height) = size.split_once('x')?;
                options.window = (width.parse().ok()?, height.parse().ok()?);
            }
            path => options.path = Some(PathBuf::from(path)),
        }
    }
    (!options.bench.as_os_str().is_empty()).then_some(options)
}

fn ms_since_process_start() -> f64 {
    let ticks =
        |time: FILETIME| (u64::from(time.dwHighDateTime) << 32) | u64::from(time.dwLowDateTime);
    let (mut created, mut exited, mut kernel, mut user) = Default::default();
    // SAFETY: the pseudo-handle is always valid and each pointer is a live local.
    let now = unsafe {
        let _ = GetProcessTimes(
            GetCurrentProcess(),
            &mut created,
            &mut exited,
            &mut kernel,
            &mut user,
        );
        GetSystemTimeAsFileTime()
    };
    ticks(now).saturating_sub(ticks(created)) as f64 / 10_000.0
}

fn private_mb() -> f64 {
    // SAFETY: the struct is plain data and `cb` tells Windows its size.
    unsafe {
        let mut counters = PROCESS_MEMORY_COUNTERS {
            cb: std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32,
            ..Default::default()
        };
        let _ = K32GetProcessMemoryInfo(GetCurrentProcess(), &mut counters, counters.cb);
        counters.PagefileUsage as f64 / (1024.0 * 1024.0)
    }
}

unsafe extern "system" fn window_proc(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if message == WM_DESTROY {
        PostQuitMessage(0);
        return LRESULT(0);
    }
    DefWindowProcW(window, message, wparam, lparam)
}

fn create_window(size: (i32, i32), topmost: bool) -> Result<HWND> {
    // SAFETY: the class and window use static strings and a valid window procedure.
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let instance = GetModuleHandleW(None)?;
        let class_name = w!("TraceViewD2DProbe");
        let class = WNDCLASSW {
            lpfnWndProc: Some(window_proc),
            hInstance: instance.into(),
            lpszClassName: class_name,
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            ..Default::default()
        };
        RegisterClassW(&class);
        let mut bounds = RECT {
            left: 0,
            top: 0,
            right: size.0,
            bottom: size.1,
        };
        AdjustWindowRect(&mut bounds, WS_OVERLAPPEDWINDOW, false)?;
        let window = CreateWindowExW(
            if topmost {
                WS_EX_TOPMOST
            } else {
                WINDOW_EX_STYLE(0)
            },
            class_name,
            w!("TraceView Direct2D probe"),
            WS_OVERLAPPEDWINDOW,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            bounds.right - bounds.left,
            bounds.bottom - bounds.top,
            None,
            None,
            Some(instance.into()),
            None,
        )?;
        let _ = ShowWindow(window, SW_SHOW);
        Ok(window)
    }
}

/// Where frames go. Offscreen frames land in a memory bitmap that can be read
/// back, which works even when the session's display isn't being drawn.
enum Surface {
    Window(HWND),
    Flip(HWND, Device),
    Offscreen(i32, i32),
}

struct Renderer {
    target: ID2D1RenderTarget,
    bitmap: Option<IWICBitmap>,
    swap_chain: Option<IDXGISwapChain1>,
    brush: ID2D1SolidColorBrush,
    left_format: IDWriteTextFormat,
    right_format: IDWriteTextFormat,
    palette: &'static Palette,
    width: f32,
    height: f32,
}

impl Renderer {
    fn new(surface: Surface, software: bool, palette: &'static Palette) -> Result<Self> {
        // SAFETY: all calls use live COM objects and locals that outlive the call.
        unsafe {
            let factory: ID2D1Factory = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
            let properties = D2D1_RENDER_TARGET_PROPERTIES {
                r#type: if software {
                    D2D1_RENDER_TARGET_TYPE_SOFTWARE
                } else {
                    D2D1_RENDER_TARGET_TYPE_DEFAULT
                },
                pixelFormat: D2D1_PIXEL_FORMAT {
                    format: DXGI_FORMAT_B8G8R8A8_UNORM,
                    alphaMode: D2D1_ALPHA_MODE_IGNORE,
                },
                dpiX: 96.0,
                dpiY: 96.0,
                ..Default::default()
            };
            let (target, bitmap, swap_chain, width, height): (ID2D1RenderTarget, _, _, _, _) =
                match surface {
                    Surface::Window(window) => {
                        let mut client = RECT::default();
                        GetClientRect(window, &mut client)?;
                        let hwnd_properties = D2D1_HWND_RENDER_TARGET_PROPERTIES {
                            hwnd: window,
                            pixelSize: D2D_SIZE_U {
                                width: client.right as u32,
                                height: client.bottom as u32,
                            },
                            presentOptions: D2D1_PRESENT_OPTIONS_IMMEDIATELY,
                        };
                        let target =
                            factory.CreateHwndRenderTarget(&properties, &hwnd_properties)?;
                        (target.cast()?, None, None, client.right, client.bottom)
                    }
                    Surface::Flip(window, device_kind) => {
                        let mut client = RECT::default();
                        GetClientRect(window, &mut client)?;
                        let mut device = None;
                        D3D11CreateDevice(
                            None,
                            match device_kind {
                                Device::Hardware => D3D_DRIVER_TYPE_HARDWARE,
                                Device::Warp => D3D_DRIVER_TYPE_WARP,
                            },
                            HMODULE::default(),
                            D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                            None,
                            D3D11_SDK_VERSION,
                            Some(&mut device),
                            None,
                            None,
                        )?;
                        let device = device.ok_or_else(windows::core::Error::empty)?;
                        let dxgi_device: IDXGIDevice = device.cast()?;
                        let dxgi_factory: IDXGIFactory2 = dxgi_device.GetAdapter()?.GetParent()?;
                        let description = DXGI_SWAP_CHAIN_DESC1 {
                            Width: client.right as u32,
                            Height: client.bottom as u32,
                            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                            SampleDesc: DXGI_SAMPLE_DESC {
                                Count: 1,
                                Quality: 0,
                            },
                            BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
                            BufferCount: 2,
                            SwapEffect: DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL,
                            AlphaMode: DXGI_ALPHA_MODE_IGNORE,
                            ..Default::default()
                        };
                        let swap_chain = dxgi_factory.CreateSwapChainForHwnd(
                            &device,
                            window,
                            &description,
                            None,
                            None,
                        )?;
                        let context = factory
                            .cast::<ID2D1Factory1>()?
                            .CreateDevice(&dxgi_device)?
                            .CreateDeviceContext(D2D1_DEVICE_CONTEXT_OPTIONS_NONE)?;
                        context.SetDpi(96.0, 96.0);
                        let back_buffer: IDXGISurface = swap_chain.GetBuffer(0)?;
                        let bitmap_properties = D2D1_BITMAP_PROPERTIES1 {
                            pixelFormat: properties.pixelFormat,
                            dpiX: 96.0,
                            dpiY: 96.0,
                            bitmapOptions: D2D1_BITMAP_OPTIONS_TARGET
                                | D2D1_BITMAP_OPTIONS_CANNOT_DRAW,
                            colorContext: std::mem::ManuallyDrop::new(None),
                        };
                        let target_bitmap = context
                            .CreateBitmapFromDxgiSurface(&back_buffer, Some(&bitmap_properties))?;
                        context.SetTarget(&target_bitmap);
                        (
                            context.cast()?,
                            None,
                            Some(swap_chain),
                            client.right,
                            client.bottom,
                        )
                    }
                    Surface::Offscreen(width, height) => {
                        CoInitializeEx(None, COINIT_MULTITHREADED).ok()?;
                        let imaging: IWICImagingFactory =
                            CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)?;
                        let bitmap = imaging.CreateBitmap(
                            width as u32,
                            height as u32,
                            &GUID_WICPixelFormat32bppBGR,
                            WICBitmapCacheOnLoad,
                        )?;
                        let target = factory.CreateWicBitmapRenderTarget(&bitmap, &properties)?;
                        (target, Some(bitmap), None, width, height)
                    }
                };
            target.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_CLEARTYPE);
            let brush = target.CreateSolidColorBrush(&palette.text, None)?;

            let write: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
            let make_format = || -> Result<IDWriteTextFormat> {
                let format = write.CreateTextFormat(
                    w!("Consolas"),
                    None,
                    DWRITE_FONT_WEIGHT_NORMAL,
                    DWRITE_FONT_STYLE_NORMAL,
                    DWRITE_FONT_STRETCH_NORMAL,
                    FONT_SIZE,
                    w!("en-us"),
                )?;
                format.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)?;
                Ok(format)
            };
            let left_format = make_format()?;
            let right_format = make_format()?;
            right_format.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_TRAILING)?;
            Ok(Self {
                target,
                bitmap,
                swap_chain,
                brush,
                left_format,
                right_format,
                palette,
                width: width as f32,
                height: height as f32,
            })
        }
    }

    /// Shows the frame on swap chain surfaces. Other surfaces present in EndDraw.
    /// `scrolled_rows` tells Windows the body moved up, so only new rows and the
    /// scrollbar count as changed. The whole frame is still drawn, which means a
    /// wrong hint would show up as a wrong image in the capture comparison.
    fn present(&self, scrolled_rows: Option<i32>) -> Result<()> {
        let Some(swap_chain) = &self.swap_chain else {
            return Ok(());
        };
        let (width, height) = (self.width as i32, self.height as i32);
        let body_top = (TOP_BAR + ROW_HEIGHT + 4.0) as i32;
        let body_bottom = height - STATUS_BAR as i32;
        let body_right = width - SCROLLBAR as i32;
        let mut parameters = DXGI_PRESENT_PARAMETERS::default();
        let shift = scrolled_rows.map_or(0, |rows| rows * ROW_HEIGHT as i32);
        let mut dirty = [
            RECT {
                left: 0,
                top: body_bottom - shift,
                right: body_right,
                bottom: body_bottom,
            },
            RECT {
                left: body_right,
                top: body_top,
                right: width,
                bottom: body_bottom,
            },
        ];
        let mut scroll_rect = RECT {
            left: 0,
            top: body_top,
            right: body_right,
            bottom: body_bottom - shift,
        };
        let mut scroll_offset = POINT { x: 0, y: -shift };
        if scrolled_rows.is_some() {
            parameters.DirtyRectsCount = dirty.len() as u32;
            parameters.pDirtyRects = dirty.as_mut_ptr();
            parameters.pScrollRect = &mut scroll_rect;
            parameters.pScrollOffset = &mut scroll_offset;
        }
        // SAFETY: the rectangles and point are locals that outlive the call.
        unsafe { swap_chain.Present1(0, DXGI_PRESENT(0), &parameters).ok() }
    }

    fn visible_rows(&self) -> f64 {
        f64::from((self.height - TOP_BAR - STATUS_BAR - ROW_HEIGHT - 4.0) / ROW_HEIGHT)
    }

    fn fill(&self, left: f32, top: f32, right: f32, bottom: f32, color: &D2D1_COLOR_F) {
        // SAFETY: the brush and target are live, and the rectangle is a local.
        unsafe {
            self.brush.SetColor(color);
            self.target.FillRectangle(
                &D2D_RECT_F {
                    left,
                    top,
                    right,
                    bottom,
                },
                &self.brush,
            );
        }
    }

    fn text(
        &self,
        text: &[u8],
        format: &IDWriteTextFormat,
        bounds: (f32, f32, f32, f32),
        color: &D2D1_COLOR_F,
        scratch: &mut Vec<u16>,
    ) {
        scratch.clear();
        let capped = &text[..text.len().min(1024)];
        let line_end = capped
            .iter()
            .position(|byte| matches!(byte, b'\n' | b'\r'))
            .unwrap_or(capped.len());
        scratch.extend(String::from_utf8_lossy(&capped[..line_end]).encode_utf16());
        // SAFETY: the buffer, format, brush and rectangle all outlive the call.
        unsafe {
            self.brush.SetColor(color);
            self.target.DrawText(
                scratch,
                format,
                &D2D_RECT_F {
                    left: bounds.0,
                    top: bounds.1,
                    right: bounds.2,
                    bottom: bounds.3,
                },
                &self.brush,
                D2D1_DRAW_TEXT_OPTIONS_CLIP,
                DWRITE_MEASURING_MODE_GDI_CLASSIC,
            );
        }
    }

    fn draw(&self, log: Option<&LogFile>, top_row: f64, scratch: &mut Vec<u16>) -> Result<()> {
        let palette = self.palette;
        // SAFETY: Begin/EndDraw are paired on a live render target.
        unsafe {
            self.target.BeginDraw();
            self.target.Clear(Some(&palette.background));
        }
        let header_top = TOP_BAR;
        let header_bottom = header_top + ROW_HEIGHT + 4.0;
        let body_right = self.width - SCROLLBAR;
        let body_bottom = self.height - STATUS_BAR;
        let (line_right, time_left) = (88.0, 100.0);
        let (component_left, message_left) = (272.0, 452.0);

        if let Some(log) = log {
            let first = top_row.floor() as usize;
            let count = ((body_bottom - header_bottom) / ROW_HEIGHT).ceil() as usize + 1;
            let offset = (top_row - top_row.floor()) as f32 * ROW_HEIGHT;
            let mut line_number = String::with_capacity(16);
            let mut time = Vec::with_capacity(32);
            // SAFETY: the clip pushed here is popped after the rows are drawn.
            unsafe {
                self.target.PushAxisAlignedClip(
                    &D2D_RECT_F {
                        left: 0.0,
                        top: header_bottom,
                        right: body_right,
                        bottom: body_bottom,
                    },
                    windows::Win32::Graphics::Direct2D::D2D1_ANTIALIAS_MODE_ALIASED,
                );
            }
            let mut buffer = Vec::new();
            let mut row_text = Vec::new();
            let read = log.read_rows(first..first + count, &mut buffer, |row, raw, _| {
                let slot = row - first;
                let y = (header_bottom + slot as f32 * ROW_HEIGHT - offset).round();
                let bottom = y + ROW_HEIGHT;
                match log.level(row) {
                    Level::Debug | Level::Info | Level::Notice => {}
                    Level::Warning => self.fill(0.0, y, body_right, bottom, &palette.warning),
                    Level::Error => self.fill(0.0, y, body_right, bottom, &palette.error),
                    Level::Critical => self.fill(0.0, y, body_right, bottom, &palette.critical),
                }
                let fields = log.fields(log.text(raw, &mut row_text));
                let text_top = y + 1.0;

                line_number.clear();
                use std::fmt::Write as _;
                let _ = write!(line_number, "{}", row + 1);
                self.text(
                    line_number.as_bytes(),
                    &self.right_format,
                    (0.0, text_top, line_right, bottom),
                    &palette.muted,
                    scratch,
                );
                time.clear();
                if let Some(stamp) = fields.stamp {
                    time.extend_from_slice(stamp.to_string().as_bytes());
                }
                self.text(
                    &time,
                    &self.left_format,
                    (time_left, text_top, component_left - 8.0, bottom),
                    &palette.muted,
                    scratch,
                );
                self.text(
                    fields.component,
                    &self.left_format,
                    (component_left, text_top, message_left - 8.0, bottom),
                    &palette.muted,
                    scratch,
                );
                self.text(
                    fields.message,
                    &self.left_format,
                    (message_left, text_top, body_right, bottom),
                    &palette.text,
                    scratch,
                );
            });
            // SAFETY: matches the push above.
            unsafe { self.target.PopAxisAlignedClip() };
            read.map_err(|_| windows::core::Error::empty())?;

            self.fill(
                body_right,
                header_bottom,
                self.width,
                body_bottom,
                &palette.scrollbar,
            );
            let max_top = (log.rows() as f64 - self.visible_rows()).max(1.0);
            let travel = body_bottom - header_bottom - 28.0;
            let thumb = header_bottom + (top_row / max_top) as f32 * travel;
            self.fill(
                body_right + 2.0,
                thumb,
                self.width - 2.0,
                thumb + 28.0,
                &palette.thumb,
            );
        }

        self.fill(0.0, header_top, self.width, header_bottom, &palette.header);
        for (title, format, bounds) in [
            (&b"Line"[..], &self.right_format, (0.0, line_right)),
            (&b"Time"[..], &self.left_format, (time_left, component_left)),
            (
                &b"Component"[..],
                &self.left_format,
                (component_left, message_left),
            ),
            (
                &b"Message"[..],
                &self.left_format,
                (message_left, body_right),
            ),
        ] {
            self.text(
                title,
                format,
                (bounds.0, header_top + 3.0, bounds.1, header_bottom),
                &palette.text,
                scratch,
            );
        }
        // SAFETY: matches BeginDraw above.
        unsafe { self.target.EndDraw(None, None) }
    }
}

fn pump_messages() -> bool {
    let mut message = MSG::default();
    // SAFETY: the message struct is a live local.
    unsafe {
        while PeekMessageW(&mut message, None, 0, 0, PM_REMOVE).as_bool() {
            if message.message == WM_QUIT {
                return false;
            }
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
    true
}

fn summary(mut samples: Vec<f64>) -> String {
    samples.sort_by(f64::total_cmp);
    let percentile = |share: f64| samples[((samples.len() - 1) as f64 * share) as usize];
    format!(
        "{{ \"p50\": {:.2}, \"p95\": {:.2}, \"p99\": {:.2}, \"max\": {:.2} }}",
        percentile(0.50),
        percentile(0.95),
        percentile(0.99),
        samples[samples.len() - 1],
    )
}

/// Saves an offscreen frame as a PNG. Windowed runs have no bitmap to read.
fn save_frame(
    renderer: &Renderer,
    path: &std::path::Path,
) -> std::result::Result<(), Box<dyn std::error::Error>> {
    let Some(bitmap) = &renderer.bitmap else {
        return Ok(());
    };
    let (width, height) = (renderer.width as u32, renderer.height as u32);
    let mut pixels = vec![0u8; (width * height * 4) as usize];
    // SAFETY: the buffer holds exactly `height` rows of `width * 4` bytes.
    unsafe { bitmap.CopyPixels(std::ptr::null(), width * 4, &mut pixels)? };
    for pixel in pixels.as_chunks_mut::<4>().0 {
        pixel.swap(0, 2);
        pixel[3] = 255;
    }
    let file = std::io::BufWriter::new(std::fs::File::create(path)?);
    let mut encoder = png::Encoder::new(file, width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.write_header()?.write_image_data(&pixels)?;
    Ok(())
}

/// Copies the window's area from the desktop: what a person sees, whatever path
/// the renderer used. Runs that capture keep the window on top so nothing covers it.
fn capture_desktop(
    window: HWND,
    path: &std::path::Path,
) -> std::result::Result<(), Box<dyn std::error::Error>> {
    // Let the compositor show the last frame first.
    std::thread::sleep(std::time::Duration::from_millis(400));
    // SAFETY: each GDI object is created here, used on this thread, and released below.
    let (width, height, mut pixels) = unsafe {
        let mut client = RECT::default();
        GetClientRect(window, &mut client)?;
        let (width, height) = (client.right, client.bottom);
        let mut origin = POINT::default();
        let _ = ClientToScreen(window, &mut origin);
        let screen = GetDC(None);
        let memory = CreateCompatibleDC(Some(screen));
        let bitmap = CreateCompatibleBitmap(screen, width, height);
        let previous = SelectObject(memory, bitmap.into());
        let copied = BitBlt(
            memory,
            0,
            0,
            width,
            height,
            Some(screen),
            origin.x,
            origin.y,
            SRCCOPY,
        );
        let mut info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width,
                biHeight: -height,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut pixels = vec![0u8; (width * height * 4) as usize];
        GetDIBits(
            memory,
            bitmap,
            0,
            height as u32,
            Some(pixels.as_mut_ptr().cast()),
            &mut info,
            DIB_RGB_COLORS,
        );
        SelectObject(memory, previous);
        let _ = DeleteObject(bitmap.into());
        let _ = DeleteDC(memory);
        ReleaseDC(None, screen);
        copied?;
        (width as u32, height as u32, pixels)
    };
    for pixel in pixels.as_chunks_mut::<4>().0 {
        pixel.swap(0, 2);
        pixel[3] = 255;
    }
    let file = std::io::BufWriter::new(std::fs::File::create(path)?);
    let mut encoder = png::Encoder::new(file, width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.write_header()?.write_image_data(&pixels)?;
    Ok(())
}

/// Waits until the frame has used its share of a 60 Hz interval.
fn wait_for_frame_end(started: Instant) {
    let interval = std::time::Duration::from_micros(16_667);
    while started.elapsed() + std::time::Duration::from_millis(2) < interval {
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    while started.elapsed() < interval {
        std::hint::spin_loop();
    }
}

fn run(options: &Options) -> std::result::Result<(), Box<dyn std::error::Error>> {
    let window = if options.offscreen {
        None
    } else {
        Some(create_window(options.window, options.capture)?)
    };
    let surface = match (window, options.flip) {
        (None, _) => Surface::Offscreen(options.window.0, options.window.1),
        (Some(window), Some(device)) => Surface::Flip(window, device),
        (Some(window), None) => Surface::Window(window),
    };
    let palette = if options.light { &LIGHT } else { &CONSOLE };
    let renderer = Renderer::new(surface, options.software, palette)?;
    let mut scratch = Vec::with_capacity(1024);

    renderer.draw(None, 0.0, &mut scratch)?;
    renderer.present(None)?;
    let launch_ms = ms_since_process_start();
    if options.launch_only {
        std::fs::write(
            &options.bench,
            format!("{{ \"launch_to_first_present_ms\": {launch_ms:.1} }}\n"),
        )?;
        return Ok(());
    }

    let path = options.path.as_ref().ok_or("a log path is required")?;
    let (log, stats) = LogFile::open(path, &AtomicU64::new(0))?;
    renderer.draw(Some(&log), 0.0, &mut scratch)?;
    renderer.present(None)?;
    pump_messages();
    save_frame(&renderer, &options.bench.with_extension("png"))?;

    let max_top = (log.rows() as f64 - renderer.visible_rows()).max(0.0);
    let page = renderer.visible_rows().floor();
    let mut random = 0x2545_F491_4F6C_DD1Du64;
    let mut stages = Vec::new();
    let mut top = 0.0f64;
    for name in ["wheel", "page_down", "random_jump", "scrollbar_sweep"] {
        if options.stage.as_deref().is_some_and(|stage| stage != name) {
            continue;
        }
        let mut frame_ms = Vec::with_capacity(options.frames);
        for _ in 0..options.frames {
            if !pump_messages() {
                return Ok(());
            }
            top = match name {
                "wheel" => top + 3.0,
                "page_down" => top + page,
                "random_jump" => {
                    random ^= random << 13;
                    random ^= random >> 7;
                    random ^= random << 17;
                    (random % max_top.max(1.0) as u64) as f64
                }
                _ => top + max_top / options.frames as f64,
            }
            .clamp(0.0, max_top);
            let started = Instant::now();
            renderer.draw(Some(&log), top, &mut scratch)?;
            let hint = (options.scroll_hints && name == "wheel").then_some(3);
            renderer.present(hint)?;
            frame_ms.push(started.elapsed().as_secs_f64() * 1000.0);
            if options.paced {
                wait_for_frame_end(started);
            }
        }
        stages.push(format!(
            "    \"{name}\": {{ \"frame_ms\": {} }}",
            summary(frame_ms)
        ));
    }
    // A second frame proves rows were still fully drawn at the end of the run.
    let after_run = options.bench.with_extension("").display().to_string() + "-end.png";
    save_frame(&renderer, std::path::Path::new(&after_run))?;
    if let (true, Some(window)) = (options.capture, window) {
        pump_messages();
        capture_desktop(window, std::path::Path::new(&after_run))?;
    }

    std::fs::write(
        &options.bench,
        format!(
            "{{\n  \"mode\": \"{}\",\n  \"launch_to_first_present_ms\": {launch_ms:.1},\n  \"rows\": {},\n  \"open_seconds\": {:.3},\n  \"cache_hit\": {},\n  \"private_mb\": {:.1},\n  \"stages\": {{\n{}\n  }}\n}}\n",
            match (options.offscreen, options.flip, options.software) {
                (true, _, _) => "offscreen",
                (false, Some(Device::Hardware), _) => "flip-hardware",
                (false, Some(Device::Warp), _) => "flip-warp",
                (false, None, true) => "hwnd-software",
                (false, None, false) => "hwnd-default",
            },
            log.rows(),
            stats.seconds,
            stats.cache == tv_engine::CacheUse::Hit,
            private_mb(),
            stages.join(",\n"),
        ),
    )?;
    Ok(())
}

fn main() {
    let Some(options) = parse_args() else {
        eprintln!("usage: d2d-probe [log] --bench out.json [--software] [--offscreen] [--light] [--launch-only] [--frames N] [--window WxH]");
        std::process::exit(2);
    };
    if let Err(error) = run(&options) {
        let _ = std::fs::write(options.bench.with_extension("err"), error.to_string());
        std::process::exit(1);
    }
}
