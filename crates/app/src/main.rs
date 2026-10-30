#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod bench;
mod grid;
mod metrics;
mod theme;

use eframe::{egui, egui_wgpu, wgpu};
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Clone, Copy, PartialEq)]
pub enum RendererChoice {
    Auto,
    Wgpu,
    Glow,
}

#[derive(Clone)]
pub struct Options {
    pub path: Option<PathBuf>,
    pub renderer: RendererChoice,
    pub software_adapter: bool,
    pub bench: Option<PathBuf>,
    pub launch_only: bool,
    pub light: bool,
    pub font_size: f32,
    pub top_row: f64,
    pub window: [f32; 2],
    pub frames: usize,
}

fn parse_args() -> Result<Options, String> {
    let mut options = Options {
        path: None,
        renderer: RendererChoice::Auto,
        software_adapter: false,
        bench: None,
        launch_only: false,
        light: false,
        font_size: 13.0,
        top_row: 0.0,
        window: [1400.0, 850.0],
        frames: 600,
    };
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut value = || args.next().ok_or(format!("{arg} needs a value"));
        match arg.as_str() {
            "--renderer" => {
                options.renderer = match value()?.as_str() {
                    "auto" => RendererChoice::Auto,
                    "wgpu" => RendererChoice::Wgpu,
                    "glow" => RendererChoice::Glow,
                    other => return Err(format!("unknown renderer {other}")),
                }
            }
            "--software-adapter" => options.software_adapter = true,
            "--bench" => options.bench = Some(PathBuf::from(value()?)),
            "--launch-only" => options.launch_only = true,
            "--light" => options.light = true,
            "--font-size" => {
                options.font_size = value()?.parse().map_err(|_| "bad --font-size")?;
            }
            "--top-row" => options.top_row = value()?.parse().map_err(|_| "bad --top-row")?,
            "--frames" => options.frames = value()?.parse().map_err(|_| "bad --frames")?,
            "--window" => {
                let size = value()?;
                let (width, height) = size.split_once('x').ok_or("--window needs WIDTHxHEIGHT")?;
                options.window = [
                    width.parse().map_err(|_| "bad --window width")?,
                    height.parse().map_err(|_| "bad --window height")?,
                ];
            }
            flag if flag.starts_with("--") => return Err(format!("unknown option {flag}")),
            path => options.path = Some(PathBuf::from(path)),
        }
    }
    Ok(options)
}

fn native_options(options: &Options, renderer: eframe::Renderer) -> eframe::NativeOptions {
    let benching = options.bench.is_some();
    let mut wgpu_options = egui_wgpu::WgpuConfiguration::default();
    wgpu_options.surface.desired_maximum_frame_latency = Some(1);
    if benching {
        // Without vsync, frame times show the real cost rather than the refresh rate.
        wgpu_options.surface.present_mode = wgpu::PresentMode::AutoNoVsync;
    }
    if let egui_wgpu::WgpuSetup::CreateNew(create_new) = &mut wgpu_options.wgpu_setup {
        create_new.instance_descriptor.backends = wgpu::Backends::DX12;
        if options.software_adapter {
            create_new.native_adapter_selector = Some(Arc::new(|adapters, _surface| {
                adapters
                    .iter()
                    .find(|adapter| adapter.get_info().device_type == wgpu::DeviceType::Cpu)
                    .cloned()
                    .ok_or_else(|| "no software adapter found".to_string())
            }));
        }
    }
    let mut native = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("TraceView")
            .with_inner_size(options.window)
            .with_drag_and_drop(true),
        renderer,
        wgpu_options,
        ..Default::default()
    };
    native.glow_options.vsync = !benching;
    native
}

fn run(options: &Options, renderer: eframe::Renderer) -> Result<(), String> {
    let app_options = options.clone();
    eframe::run_native(
        "TraceView",
        native_options(options, renderer),
        Box::new(move |creation| Ok(Box::new(app::App::new(creation, app_options)))),
    )
    .map_err(|error| error.to_string())
}

fn report_error(options: Option<&Options>, message: &str) {
    let path = options
        .and_then(|options| options.bench.as_ref())
        .map(|bench| bench.with_extension("err"));
    match path {
        Some(path) => {
            let _ = std::fs::write(path, message);
        }
        None => eprintln!("{message}"),
    }
}

fn main() {
    metrics::mark("main_entered");
    let options = match parse_args() {
        Ok(options) => options,
        Err(message) => {
            report_error(None, &message);
            std::process::exit(2);
        }
    };
    let result = match options.renderer {
        RendererChoice::Wgpu => run(&options, eframe::Renderer::Wgpu),
        RendererChoice::Glow => run(&options, eframe::Renderer::Glow),
        RendererChoice::Auto => run(&options, eframe::Renderer::Wgpu).or_else(|wgpu_error| {
            run(&options, eframe::Renderer::Glow)
                .map_err(|glow_error| format!("wgpu: {wgpu_error}\nglow: {glow_error}"))
        }),
    };
    if let Err(message) = result {
        report_error(Some(&options), &message);
        std::process::exit(1);
    }
}
