//! Draws with Direct2D and DirectWrite. By default frames go through a
//! flip-model swap chain on a software D3D11 device (WARP): measured over RDP it
//! launches in about 60 ms and draws in about 1.5 ms, with no GPU dependency.

use std::collections::HashMap;
use tv_ui::{Align, Canvas, Color, FontRole, Rect, TextStyle};
use windows::core::{w, Interface, Result, HSTRING};
use windows::Win32::Foundation::{D2DERR_RECREATE_TARGET, HMODULE, HWND, RECT};
use windows::Win32::Graphics::Direct2D::Common::{
    D2D1_ALPHA_MODE_IGNORE, D2D1_COLOR_F, D2D1_PIXEL_FORMAT, D2D_RECT_F, D2D_SIZE_U,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1CreateFactory, ID2D1DeviceContext, ID2D1Factory, ID2D1Factory1, ID2D1HwndRenderTarget,
    ID2D1RenderTarget, ID2D1SolidColorBrush, D2D1_ANTIALIAS_MODE_ALIASED,
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
    DWRITE_MEASURING_MODE, DWRITE_MEASURING_MODE_GDI_CLASSIC, DWRITE_MEASURING_MODE_NATURAL,
    DWRITE_PARAGRAPH_ALIGNMENT_CENTER, DWRITE_PARAGRAPH_ALIGNMENT_NEAR,
    DWRITE_TEXT_ALIGNMENT_LEADING, DWRITE_TEXT_ALIGNMENT_TRAILING, DWRITE_TEXT_METRICS,
    DWRITE_WORD_WRAPPING_NO_WRAP, DWRITE_WORD_WRAPPING_WRAP,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_ALPHA_MODE_IGNORE, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_UNKNOWN, DXGI_SAMPLE_DESC,
};
use windows::Win32::Graphics::Dxgi::{
    IDXGIDevice, IDXGIFactory2, IDXGISurface, IDXGISwapChain1, DXGI_ERROR_DEVICE_REMOVED,
    DXGI_ERROR_DEVICE_RESET, DXGI_PRESENT, DXGI_PRESENT_PARAMETERS, DXGI_SWAP_CHAIN_DESC1,
    DXGI_SWAP_CHAIN_FLAG, DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL, DXGI_USAGE_RENDER_TARGET_OUTPUT,
};
use windows::Win32::Graphics::Imaging::{
    CLSID_WICImagingFactory, GUID_WICPixelFormat32bppBGR, IWICBitmap, IWICImagingFactory,
    WICBitmapCacheOnLoad,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
};

/// Ways to get frames to a window, in the order they're worth trying.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Presentation {
    /// Flip-model swap chain on a software D3D11 device. The default everywhere.
    FlipWarp,
    /// Flip-model swap chain on the GPU. Uses less CPU but launches about 190 ms slower.
    FlipHardware,
    /// Direct2D's software target. The fallback if D3D11 can't be used at all.
    SoftwareWindow,
}

pub const DEFAULT_ORDER: [Presentation; 2] = [Presentation::FlipWarp, Presentation::SoftwareWindow];

/// Fonts by role. Sizes are in device-independent pixels.
#[derive(Clone, Debug, PartialEq)]
pub struct Fonts {
    pub ui_family: String,
    pub ui_size: f32,
    pub grid_family: String,
    pub grid_size: f32,
}

impl Default for Fonts {
    fn default() -> Self {
        Self {
            ui_family: "Segoe UI".to_string(),
            ui_size: 12.0,
            grid_family: "Consolas".to_string(),
            grid_size: 13.0,
        }
    }
}

/// The result of finishing a frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameResult {
    Presented,
    /// The graphics device was lost and has been recreated. Draw the frame again.
    DeviceRecreated,
}

enum Surface {
    Flip {
        swap_chain: IDXGISwapChain1,
        context: ID2D1DeviceContext,
    },
    Window(ID2D1HwndRenderTarget),
    Offscreen(IWICBitmap),
}

struct Target {
    surface: Surface,
    target: ID2D1RenderTarget,
    brush: ID2D1SolidColorBrush,
}

pub struct Renderer {
    factory: ID2D1Factory,
    write: IDWriteFactory,
    hwnd: Option<HWND>,
    order: Vec<Presentation>,
    active: Option<Presentation>,
    target: Target,
    width: u32,
    height: u32,
    dpi: f32,
    fonts: Fonts,
    formats: HashMap<(FontRole, Align, bool), IDWriteTextFormat>,
    char_widths: HashMap<FontRole, f32>,
    utf16: Vec<u16>,
    vsync: bool,
}

fn to_d2d_color(color: Color) -> D2D1_COLOR_F {
    D2D1_COLOR_F {
        r: f32::from(color.r) / 255.0,
        g: f32::from(color.g) / 255.0,
        b: f32::from(color.b) / 255.0,
        a: 1.0,
    }
}

fn to_d2d_rect(rect: Rect) -> D2D_RECT_F {
    D2D_RECT_F {
        left: rect.left,
        top: rect.top,
        right: rect.right,
        bottom: rect.bottom,
    }
}

fn render_target_properties(software: bool) -> D2D1_RENDER_TARGET_PROPERTIES {
    D2D1_RENDER_TARGET_PROPERTIES {
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
    }
}

fn finish_target(surface: Surface, target: ID2D1RenderTarget, dpi: f32) -> Result<Target> {
    // SAFETY: the target is live and the color is a local that outlives the call.
    let brush = unsafe {
        target.SetDpi(dpi, dpi);
        target.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_CLEARTYPE);
        // Aliased edges keep row backgrounds crisp at fractional display scales.
        target.SetAntialiasMode(D2D1_ANTIALIAS_MODE_ALIASED);
        target.CreateSolidColorBrush(&to_d2d_color(Color::rgb(0)), None)?
    };
    Ok(Target {
        surface,
        target,
        brush,
    })
}

/// Points the device context at the swap chain's current back buffer.
fn bind_back_buffer(
    swap_chain: &IDXGISwapChain1,
    context: &ID2D1DeviceContext,
    dpi: f32,
) -> Result<()> {
    // SAFETY: all objects are live and the properties are a local.
    unsafe {
        let back_buffer: IDXGISurface = swap_chain.GetBuffer(0)?;
        let properties = D2D1_BITMAP_PROPERTIES1 {
            pixelFormat: D2D1_PIXEL_FORMAT {
                format: DXGI_FORMAT_B8G8R8A8_UNORM,
                alphaMode: D2D1_ALPHA_MODE_IGNORE,
            },
            dpiX: dpi,
            dpiY: dpi,
            bitmapOptions: D2D1_BITMAP_OPTIONS_TARGET | D2D1_BITMAP_OPTIONS_CANNOT_DRAW,
            colorContext: std::mem::ManuallyDrop::new(None),
        };
        let bitmap = context.CreateBitmapFromDxgiSurface(&back_buffer, Some(&properties))?;
        context.SetTarget(&bitmap);
    }
    Ok(())
}

fn create_flip_target(
    factory: &ID2D1Factory,
    hwnd: HWND,
    warp: bool,
    width: u32,
    height: u32,
    dpi: f32,
) -> Result<Target> {
    // SAFETY: the window handle is live for the renderer's lifetime, and all
    // other arguments are locals or live COM objects.
    unsafe {
        let mut device = None;
        D3D11CreateDevice(
            None,
            if warp {
                D3D_DRIVER_TYPE_WARP
            } else {
                D3D_DRIVER_TYPE_HARDWARE
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
            Width: width.max(1),
            Height: height.max(1),
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
        let swap_chain =
            dxgi_factory.CreateSwapChainForHwnd(&device, hwnd, &description, None, None)?;
        let context = factory
            .cast::<ID2D1Factory1>()?
            .CreateDevice(&dxgi_device)?
            .CreateDeviceContext(D2D1_DEVICE_CONTEXT_OPTIONS_NONE)?;
        bind_back_buffer(&swap_chain, &context, dpi)?;
        let target: ID2D1RenderTarget = context.cast()?;
        finish_target(
            Surface::Flip {
                swap_chain,
                context,
            },
            target,
            dpi,
        )
    }
}

fn create_window_target(
    factory: &ID2D1Factory,
    hwnd: HWND,
    width: u32,
    height: u32,
    dpi: f32,
) -> Result<Target> {
    let hwnd_properties = D2D1_HWND_RENDER_TARGET_PROPERTIES {
        hwnd,
        pixelSize: D2D_SIZE_U {
            width: width.max(1),
            height: height.max(1),
        },
        presentOptions: D2D1_PRESENT_OPTIONS_IMMEDIATELY,
    };
    // SAFETY: the window handle is live and the properties are locals.
    let hwnd_target = unsafe {
        factory.CreateHwndRenderTarget(&render_target_properties(true), &hwnd_properties)?
    };
    let target: ID2D1RenderTarget = hwnd_target.cast()?;
    finish_target(Surface::Window(hwnd_target), target, dpi)
}

fn create_offscreen_target(
    factory: &ID2D1Factory,
    width: u32,
    height: u32,
    dpi: f32,
) -> Result<Target> {
    // SAFETY: COM is initialized here, and all arguments are live or locals.
    unsafe {
        CoInitializeEx(None, COINIT_MULTITHREADED).ok().ok();
        let imaging: IWICImagingFactory =
            CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)?;
        let bitmap = imaging.CreateBitmap(
            width.max(1),
            height.max(1),
            &GUID_WICPixelFormat32bppBGR,
            WICBitmapCacheOnLoad,
        )?;
        let target =
            factory.CreateWicBitmapRenderTarget(&bitmap, &render_target_properties(false))?;
        finish_target(Surface::Offscreen(bitmap), target, dpi)
    }
}

impl Renderer {
    /// Creates a renderer for a window, trying each presentation path in order.
    /// `hwnd` is the window's handle, and must outlive the renderer.
    pub fn for_window(
        hwnd: isize,
        width: u32,
        height: u32,
        dpi: f32,
        order: &[Presentation],
    ) -> Result<Self> {
        let hwnd = HWND(hwnd as *mut _);
        let (factory, write) = create_factories()?;
        let (target, active) = create_first_working(&factory, hwnd, order, width, height, dpi)?;
        Ok(Self::assemble(
            factory,
            write,
            Some(hwnd),
            order.to_vec(),
            Some(active),
            target,
            width,
            height,
            dpi,
        ))
    }

    /// Creates a renderer that draws into memory, for tests and screenshots.
    pub fn offscreen(width: u32, height: u32, dpi: f32) -> Result<Self> {
        let (factory, write) = create_factories()?;
        let target = create_offscreen_target(&factory, width, height, dpi)?;
        Ok(Self::assemble(
            factory,
            write,
            None,
            Vec::new(),
            None,
            target,
            width,
            height,
            dpi,
        ))
    }

    #[allow(clippy::too_many_arguments)]
    fn assemble(
        factory: ID2D1Factory,
        write: IDWriteFactory,
        hwnd: Option<HWND>,
        order: Vec<Presentation>,
        active: Option<Presentation>,
        target: Target,
        width: u32,
        height: u32,
        dpi: f32,
    ) -> Self {
        Self {
            factory,
            write,
            hwnd,
            order,
            active,
            target,
            width,
            height,
            dpi,
            fonts: Fonts::default(),
            formats: HashMap::new(),
            char_widths: HashMap::new(),
            utf16: Vec::with_capacity(1024),
            vsync: true,
        }
    }

    /// Which path is delivering frames, or `None` when drawing into memory.
    pub fn presentation(&self) -> Option<Presentation> {
        self.active
    }

    /// Display scale, where 1.0 means 100%.
    pub fn scale(&self) -> f32 {
        self.dpi / 96.0
    }

    /// Size in device-independent pixels.
    pub fn logical_size(&self) -> (f32, f32) {
        let scale = self.scale();
        (self.width as f32 / scale, self.height as f32 / scale)
    }

    /// Turns off waiting for the display's refresh, for measuring real frame cost.
    pub fn set_vsync(&mut self, vsync: bool) {
        self.vsync = vsync;
    }

    pub fn set_fonts(&mut self, fonts: Fonts) {
        if fonts != self.fonts {
            self.fonts = fonts;
            self.formats.clear();
            self.char_widths.clear();
        }
    }

    /// Matches a new window size in physical pixels, and its monitor's DPI.
    pub fn resize(&mut self, width: u32, height: u32, dpi: f32) -> Result<()> {
        let dpi_changed = dpi != self.dpi;
        if width == self.width && height == self.height && !dpi_changed {
            return Ok(());
        }
        self.width = width;
        self.height = height;
        self.dpi = dpi;
        if dpi_changed {
            self.char_widths.clear();
        }
        // SAFETY: all objects are live. The device context's target is released
        // before resizing, because DXGI requires no outstanding buffer references.
        unsafe {
            match &self.target.surface {
                Surface::Flip {
                    swap_chain,
                    context,
                } => {
                    context.SetTarget(None);
                    swap_chain.ResizeBuffers(
                        0,
                        width.max(1),
                        height.max(1),
                        DXGI_FORMAT_UNKNOWN,
                        DXGI_SWAP_CHAIN_FLAG(0),
                    )?;
                    bind_back_buffer(swap_chain, context, dpi)?;
                }
                Surface::Window(window) => {
                    window.Resize(&D2D_SIZE_U {
                        width: width.max(1),
                        height: height.max(1),
                    })?;
                }
                Surface::Offscreen(_) => {
                    self.target = create_offscreen_target(&self.factory, width, height, dpi)?;
                    return Ok(());
                }
            }
            self.target.target.SetDpi(dpi, dpi);
        }
        Ok(())
    }

    pub fn begin(&mut self) {
        // SAFETY: the target is live, and `end` makes the matching EndDraw call.
        unsafe { self.target.target.BeginDraw() };
    }

    /// Finishes the frame and shows it. If the graphics device was lost, it's
    /// recreated, which means the caller must draw the frame again.
    pub fn end(&mut self) -> Result<FrameResult> {
        // SAFETY: matches the BeginDraw in `begin`.
        let drawn = unsafe { self.target.target.EndDraw(None, None) };
        let result = drawn.and_then(|()| self.present());
        match result {
            Ok(()) => Ok(FrameResult::Presented),
            Err(error) if is_device_loss(&error) => {
                self.recreate_target()?;
                Ok(FrameResult::DeviceRecreated)
            }
            Err(error) => Err(error),
        }
    }

    fn present(&self) -> Result<()> {
        let Surface::Flip { swap_chain, .. } = &self.target.surface else {
            return Ok(());
        };
        let parameters = DXGI_PRESENT_PARAMETERS::default();
        // SAFETY: the swap chain is live and the parameters are a local.
        unsafe {
            swap_chain
                .Present1(u32::from(self.vsync), DXGI_PRESENT(0), &parameters)
                .ok()
        }
    }

    fn recreate_target(&mut self) -> Result<()> {
        let Some(hwnd) = self.hwnd else {
            self.target =
                create_offscreen_target(&self.factory, self.width, self.height, self.dpi)?;
            return Ok(());
        };
        let (target, active) = create_first_working(
            &self.factory,
            hwnd,
            &self.order,
            self.width,
            self.height,
            self.dpi,
        )?;
        self.target = target;
        self.active = Some(active);
        Ok(())
    }

    /// Reads back an offscreen frame as top-down RGBA. Returns `None` for
    /// window renderers.
    pub fn rgba_pixels(&self) -> Option<Vec<u8>> {
        let Surface::Offscreen(bitmap) = &self.target.surface else {
            return None;
        };
        let mut pixels = vec![0u8; (self.width.max(1) * self.height.max(1) * 4) as usize];
        // SAFETY: the buffer holds exactly `height` rows of `width * 4` bytes.
        unsafe {
            bitmap
                .CopyPixels(std::ptr::null(), self.width.max(1) * 4, &mut pixels)
                .ok()?;
        }
        for pixel in pixels.as_chunks_mut::<4>().0 {
            pixel.swap(0, 2);
            pixel[3] = 255;
        }
        Some(pixels)
    }

    fn format(&mut self, style: TextStyle, wrap: bool) -> Option<IDWriteTextFormat> {
        let key = (style.font, style.align, wrap);
        if let Some(format) = self.formats.get(&key) {
            return Some(format.clone());
        }
        let (family, size) = match style.font {
            FontRole::Ui => (&self.fonts.ui_family, self.fonts.ui_size),
            FontRole::Grid => (&self.fonts.grid_family, self.fonts.grid_size),
        };
        // SAFETY: the strings outlive the call and the factory is live.
        let format = unsafe {
            let format = self
                .write
                .CreateTextFormat(
                    &HSTRING::from(family.as_str()),
                    None,
                    DWRITE_FONT_WEIGHT_NORMAL,
                    DWRITE_FONT_STYLE_NORMAL,
                    DWRITE_FONT_STRETCH_NORMAL,
                    size,
                    w!("en-us"),
                )
                .ok()?;
            if wrap {
                format.SetWordWrapping(DWRITE_WORD_WRAPPING_WRAP).ok()?;
                format
                    .SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_NEAR)
                    .ok()?;
            } else {
                format.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP).ok()?;
                format
                    .SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)
                    .ok()?;
            }
            format
                .SetTextAlignment(match style.align {
                    Align::Left => DWRITE_TEXT_ALIGNMENT_LEADING,
                    Align::Right => DWRITE_TEXT_ALIGNMENT_TRAILING,
                })
                .ok()?;
            format
        };
        self.formats.insert(key, format.clone());
        Some(format)
    }

    /// GDI-style measuring is sharpest at 100% scale. Natural spacing suits
    /// higher densities, where pixels are small enough to place glyphs freely.
    fn measuring_mode(&self) -> DWRITE_MEASURING_MODE {
        if self.dpi <= 96.0 {
            DWRITE_MEASURING_MODE_GDI_CLASSIC
        } else {
            DWRITE_MEASURING_MODE_NATURAL
        }
    }
}

fn create_factories() -> Result<(ID2D1Factory, IDWriteFactory)> {
    // SAFETY: both calls only create factories.
    unsafe {
        let factory: ID2D1Factory = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
        let write: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
        Ok((factory, write))
    }
}

fn create_first_working(
    factory: &ID2D1Factory,
    hwnd: HWND,
    order: &[Presentation],
    width: u32,
    height: u32,
    dpi: f32,
) -> Result<(Target, Presentation)> {
    first_working(order, |presentation| match presentation {
        Presentation::FlipWarp => create_flip_target(factory, hwnd, true, width, height, dpi),
        Presentation::FlipHardware => create_flip_target(factory, hwnd, false, width, height, dpi),
        Presentation::SoftwareWindow => create_window_target(factory, hwnd, width, height, dpi),
    })
}

/// Tries each path in order and keeps the first that works. If none does, the
/// last error is returned.
fn first_working<T>(
    order: &[Presentation],
    mut create: impl FnMut(Presentation) -> Result<T>,
) -> Result<(T, Presentation)> {
    let mut last_error = windows::core::Error::empty();
    for presentation in order {
        match create(*presentation) {
            Ok(created) => return Ok((created, *presentation)),
            Err(error) => last_error = error,
        }
    }
    Err(last_error)
}

fn is_device_loss(error: &windows::core::Error) -> bool {
    let code = error.code();
    code == D2DERR_RECREATE_TARGET
        || code == DXGI_ERROR_DEVICE_REMOVED
        || code == DXGI_ERROR_DEVICE_RESET
}

impl Canvas for Renderer {
    fn clear(&mut self, color: Color) {
        // SAFETY: called between BeginDraw and EndDraw on a live target.
        unsafe { self.target.target.Clear(Some(&to_d2d_color(color))) };
    }

    fn fill_rect(&mut self, rect: Rect, color: Color) {
        // SAFETY: called between BeginDraw and EndDraw, with locals as arguments.
        unsafe {
            self.target.brush.SetColor(&to_d2d_color(color));
            self.target
                .target
                .FillRectangle(&to_d2d_rect(rect), &self.target.brush);
        }
    }

    fn text(&mut self, text: &str, rect: Rect, style: TextStyle, color: Color) {
        if text.is_empty() {
            return;
        }
        let Some(format) = self.format(style, false) else {
            return;
        };
        self.draw_utf16(text, rect, &format, color);
    }

    fn text_width(&mut self, text: &str, font: FontRole) -> f32 {
        if text.is_empty() {
            return 0.0;
        }
        let Some(format) = self.format(TextStyle::new(font, Align::Left), false) else {
            return 0.0;
        };
        self.utf16.clear();
        self.utf16.extend(text.encode_utf16());
        // SAFETY: the text and format outlive the call, and metrics is a local.
        unsafe {
            self.write
                .CreateTextLayout(&self.utf16, &format, 1_000_000.0, 1_000.0)
                .and_then(|layout| {
                    let mut metrics = DWRITE_TEXT_METRICS::default();
                    layout.GetMetrics(&mut metrics)?;
                    Ok(metrics.widthIncludingTrailingWhitespace)
                })
                .unwrap_or(0.0)
        }
    }

    fn text_block(&mut self, text: &str, rect: Rect, font: FontRole, color: Color) {
        if text.is_empty() {
            return;
        }
        let Some(format) = self.format(TextStyle::new(font, Align::Left), true) else {
            return;
        };
        self.draw_utf16(text, rect, &format, color);
    }

    fn push_clip(&mut self, rect: Rect) {
        // SAFETY: called between BeginDraw and EndDraw. `pop_clip` balances it.
        unsafe {
            self.target
                .target
                .PushAxisAlignedClip(&to_d2d_rect(rect), D2D1_ANTIALIAS_MODE_ALIASED);
        }
    }

    fn pop_clip(&mut self) {
        // SAFETY: matches `push_clip`.
        unsafe { self.target.target.PopAxisAlignedClip() };
    }

    fn char_width(&mut self, font: FontRole) -> f32 {
        if let Some(width) = self.char_widths.get(&font) {
            return *width;
        }
        let fallback = match font {
            FontRole::Ui => self.fonts.ui_size * 0.6,
            FontRole::Grid => self.fonts.grid_size * 0.6,
        };
        let width = match self.text_width("0000000000", font) {
            measured if measured > 0.0 => measured / 10.0,
            _ => fallback,
        };
        self.char_widths.insert(font, width);
        width
    }
}

impl Renderer {
    fn draw_utf16(&mut self, text: &str, rect: Rect, format: &IDWriteTextFormat, color: Color) {
        let mode = self.measuring_mode();
        self.utf16.clear();
        self.utf16.extend(text.encode_utf16());
        // SAFETY: called between BeginDraw and EndDraw, with arguments that
        // outlive the call.
        unsafe {
            self.target.brush.SetColor(&to_d2d_color(color));
            self.target.target.DrawText(
                &self.utf16,
                format,
                &to_d2d_rect(rect),
                &self.target.brush,
                D2D1_DRAW_TEXT_OPTIONS_CLIP,
                mode,
            );
        }
    }
}

/// The window's current client size, for callers that only have a handle.
pub fn client_size(hwnd: isize) -> Option<(u32, u32)> {
    let mut client = RECT::default();
    // SAFETY: the handle is the caller's live window, and client is a local.
    unsafe {
        windows::Win32::UI::WindowsAndMessaging::GetClientRect(HWND(hwnd as *mut _), &mut client)
            .ok()?;
    }
    Some((client.right as u32, client.bottom as u32))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tv_ui::Theme;
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DestroyWindow, WINDOW_EX_STYLE, WS_OVERLAPPEDWINDOW,
    };

    const GRID_LEFT: TextStyle = TextStyle::new(FontRole::Grid, Align::Left);

    fn pixel(pixels: &[u8], width: u32, x: u32, y: u32) -> [u8; 3] {
        let at = ((y * width + x) * 4) as usize;
        [pixels[at], pixels[at + 1], pixels[at + 2]]
    }

    fn hidden_window() -> HWND {
        // SAFETY: creates a hidden window of a built-in class, destroyed by the test.
        unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("STATIC"),
                w!("test"),
                WS_OVERLAPPEDWINDOW,
                0,
                0,
                400,
                300,
                None,
                None,
                GetModuleHandleW(None).ok().map(Into::into),
                None,
            )
            .unwrap()
        }
    }

    #[test]
    fn offscreen_frames_contain_what_was_drawn() {
        let mut renderer = Renderer::offscreen(200, 100, 96.0).unwrap();
        renderer.begin();
        renderer.clear(Color::rgb(0xFFFFFF));
        renderer.fill_rect(Rect::new(0.0, 0.0, 50.0, 50.0), Color::rgb(0xFF0000));
        renderer.text(
            "Hello",
            Rect::new(60.0, 0.0, 200.0, 50.0),
            GRID_LEFT,
            Color::rgb(0x000000),
        );
        assert_eq!(renderer.end().unwrap(), FrameResult::Presented);

        let pixels = renderer.rgba_pixels().unwrap();
        assert_eq!(pixel(&pixels, 200, 10, 10), [0xFF, 0, 0]);
        assert_eq!(pixel(&pixels, 200, 150, 90), [0xFF, 0xFF, 0xFF]);
        let text_area_has_ink = (60..200)
            .flat_map(|x| (0..50).map(move |y| (x, y)))
            .any(|(x, y)| pixel(&pixels, 200, x, y) != [0xFF, 0xFF, 0xFF]);
        assert!(text_area_has_ink, "text drew nothing");
    }

    #[test]
    fn clips_limit_drawing() {
        let mut renderer = Renderer::offscreen(100, 100, 96.0).unwrap();
        renderer.begin();
        renderer.clear(Color::rgb(0xFFFFFF));
        renderer.push_clip(Rect::new(0.0, 0.0, 50.0, 100.0));
        renderer.fill_rect(Rect::new(0.0, 0.0, 100.0, 100.0), Color::rgb(0x00FF00));
        renderer.pop_clip();
        renderer.end().unwrap();
        let pixels = renderer.rgba_pixels().unwrap();
        assert_eq!(pixel(&pixels, 100, 25, 50), [0, 0xFF, 0]);
        assert_eq!(pixel(&pixels, 100, 75, 50), [0xFF, 0xFF, 0xFF]);
    }

    #[test]
    fn higher_dpi_scales_drawing_but_not_coordinates() {
        let mut renderer = Renderer::offscreen(200, 200, 192.0).unwrap();
        assert_eq!(renderer.scale(), 2.0);
        assert_eq!(renderer.logical_size(), (100.0, 100.0));
        renderer.begin();
        renderer.clear(Color::rgb(0xFFFFFF));
        renderer.fill_rect(Rect::new(0.0, 0.0, 50.0, 50.0), Color::rgb(0x0000FF));
        renderer.end().unwrap();
        let pixels = renderer.rgba_pixels().unwrap();
        assert_eq!(pixel(&pixels, 200, 99, 99), [0, 0, 0xFF]);
        assert_eq!(pixel(&pixels, 200, 101, 101), [0xFF, 0xFF, 0xFF]);
    }

    #[test]
    fn grid_font_is_fixed_width_and_measured() {
        let mut renderer = Renderer::offscreen(10, 10, 96.0).unwrap();
        let width = renderer.char_width(FontRole::Grid);
        assert!((5.0..12.0).contains(&width), "measured {width}");
    }

    #[test]
    fn empty_text_and_missing_fonts_are_safe() {
        let mut renderer = Renderer::offscreen(100, 50, 96.0).unwrap();
        renderer.set_fonts(Fonts {
            grid_family: "No Such Font Anywhere".to_string(),
            ..Fonts::default()
        });
        renderer.begin();
        renderer.clear(Color::rgb(0xFFFFFF));
        let area = Rect::new(0.0, 0.0, 100.0, 50.0);
        renderer.text("", area, GRID_LEFT, Theme::CLASSIC.primary_text);
        renderer.text("still draws", area, GRID_LEFT, Theme::CLASSIC.primary_text);
        assert_eq!(renderer.end().unwrap(), FrameResult::Presented);
    }

    #[test]
    fn default_path_is_warp_and_it_presents_and_resizes() {
        let window = hidden_window();
        let mut renderer =
            Renderer::for_window(window.0 as isize, 400, 300, 96.0, &DEFAULT_ORDER).unwrap();
        assert_eq!(renderer.presentation(), Some(Presentation::FlipWarp));
        renderer.set_vsync(false);
        for (width, height, dpi) in [(400, 300, 96.0), (800, 600, 96.0), (800, 600, 144.0)] {
            renderer.resize(width, height, dpi).unwrap();
            renderer.begin();
            renderer.clear(Color::rgb(0x112233));
            renderer.text(
                "frame",
                Rect::new(0.0, 0.0, 100.0, 20.0),
                GRID_LEFT,
                Color::rgb(0xFFFFFF),
            );
            renderer.end().unwrap();
        }
        drop(renderer);
        // SAFETY: the window was created by this test and the renderer is gone.
        unsafe { DestroyWindow(window).unwrap() };
    }

    #[test]
    fn software_window_path_works_for_the_fallback() {
        let window = hidden_window();
        let mut renderer = Renderer::for_window(
            window.0 as isize,
            400,
            300,
            96.0,
            &[Presentation::SoftwareWindow],
        )
        .unwrap();
        assert_eq!(renderer.presentation(), Some(Presentation::SoftwareWindow));
        renderer.begin();
        renderer.clear(Color::rgb(0x112233));
        renderer.end().unwrap();
        renderer.resize(500, 400, 96.0).unwrap();
        renderer.begin();
        renderer.clear(Color::rgb(0x112233));
        renderer.end().unwrap();
        drop(renderer);
        // SAFETY: the window was created by this test and the renderer is gone.
        unsafe { DestroyWindow(window).unwrap() };
    }

    #[test]
    fn a_failing_path_falls_back_to_the_next() {
        let (created, used) = first_working(&DEFAULT_ORDER, |presentation| {
            if presentation == Presentation::FlipWarp {
                Err(windows::core::Error::empty())
            } else {
                Ok(presentation)
            }
        })
        .unwrap();
        assert_eq!(used, Presentation::SoftwareWindow);
        assert_eq!(created, Presentation::SoftwareWindow);
    }

    #[test]
    fn the_first_working_path_stops_the_search() {
        let mut tried = Vec::new();
        let (_, used) = first_working(&DEFAULT_ORDER, |presentation| {
            tried.push(presentation);
            Ok(())
        })
        .unwrap();
        assert_eq!(used, Presentation::FlipWarp);
        assert_eq!(tried, vec![Presentation::FlipWarp]);
    }

    /// A destroyed window makes every path fail. That must be an error the app
    /// can report, not a crash.
    #[test]
    fn an_impossible_target_is_an_error_not_a_crash() {
        let window = hidden_window();
        // SAFETY: the window was created by this test.
        unsafe { DestroyWindow(window).unwrap() };
        assert!(Renderer::for_window(window.0 as isize, 10, 10, 96.0, &DEFAULT_ORDER).is_err());
    }

    #[test]
    fn device_loss_errors_are_recognized() {
        assert!(is_device_loss(&windows::core::Error::from_hresult(
            D2DERR_RECREATE_TARGET
        )));
        assert!(is_device_loss(&windows::core::Error::from_hresult(
            DXGI_ERROR_DEVICE_REMOVED
        )));
        assert!(!is_device_loss(&windows::core::Error::empty()));
    }
}
