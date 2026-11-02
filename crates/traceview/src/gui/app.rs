//! Connects the window and the real renderer to the workspace, which holds
//! everything else. Slow work runs on other threads, which wake the window when
//! something's ready, so the UI thread never waits.

use super::accessibility;
#[cfg(feature = "bench")]
use super::bench::{Bench, Options as BenchOptions};
use super::settings::{self, Store};
use super::workspace::Workspace;
use accesskit::{ActionRequest, TreeUpdate};
use std::path::PathBuf;
use tv_engine::{Encoding, Format};
use tv_platform::{Command, Commands, Handler, WindowContext};
use tv_render::{Fonts, FrameResult, Presentation, Renderer, DEFAULT_ORDER};
use tv_ui::{Event, Theme};

pub fn encoding_name(encoding: Encoding) -> &'static str {
    match encoding {
        Encoding::Utf8 => "UTF-8",
        Encoding::Utf16Le | Encoding::Utf16Be => "UTF-16",
    }
}

pub fn format_name(format: Format) -> &'static str {
    match format {
        Format::Plain => "Text",
        Format::Sccm => "SCCM",
    }
}

pub fn readable_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut size = bytes as f64;
    let mut unit = 0;
    while size >= 1024.0 && unit + 1 < UNITS.len() {
        size /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{size:.1} {}", UNITS[unit])
    }
}

pub(super) fn fonts_for(theme: &Theme) -> Fonts {
    Fonts {
        ui_size: theme.ui_font_size,
        grid_size: theme.grid_font_size,
        ..Fonts::default()
    }
}

pub struct App {
    renderer: Renderer,
    workspace: Workspace,
    /// The theme and title last applied to the window, to update them on change.
    applied_theme: Option<&'static str>,
    applied_title: String,
    #[cfg(feature = "bench")]
    bench: Option<Bench>,
}

impl App {
    pub fn create(
        context: WindowContext,
        path: Option<PathBuf>,
        #[cfg(feature = "bench")] bench: Option<BenchOptions>,
        #[cfg(not(feature = "bench"))] _bench: Option<()>,
    ) -> Result<Self, String> {
        #[cfg(feature = "bench")]
        let bench = bench.map(Bench::new);
        #[cfg(feature = "bench")]
        let single;
        #[cfg(feature = "bench")]
        let order: &[Presentation] = match &bench {
            Some(bench) => {
                single = [bench.presentation()];
                &single
            }
            None => &DEFAULT_ORDER,
        };
        #[cfg(not(feature = "bench"))]
        let order: &[Presentation] = &DEFAULT_ORDER;

        let mut renderer = Renderer::for_window(
            context.hwnd,
            context.width,
            context.height,
            context.dpi,
            order,
        )
        .map_err(|error| format!("couldn't set up drawing: {error}"))?;
        #[cfg(feature = "bench")]
        if bench.is_some() {
            renderer.set_vsync(false);
        }
        let mut workspace = Workspace::new(context.waker, context.hwnd, context.system_is_dark);
        // A benchmark runs with defaults, so saved settings can't change its
        // results and it never writes to them.
        #[cfg(feature = "bench")]
        let use_saved = bench.is_none();
        #[cfg(not(feature = "bench"))]
        let use_saved = true;
        if use_saved {
            let (store, settings, problem) = Store::open(settings::location());
            workspace.use_settings(settings, store, problem);
        }
        renderer.set_fonts(fonts_for(workspace.theme()));
        if let Some(path) = path {
            workspace.open_paths(vec![path]);
        }
        Ok(Self {
            renderer,
            workspace,
            applied_theme: None,
            applied_title: String::new(),
            #[cfg(feature = "bench")]
            bench,
        })
    }

    /// Keeps the title bar, fonts and IME in step with the workspace.
    fn sync_window(&mut self, commands: &mut Commands) {
        let theme = self.workspace.theme();
        if self.applied_theme != Some(theme.name) {
            if self.applied_theme.is_none() {
                // The grid has focus at start, so the IME starts off.
                commands.push(Command::SetImeEnabled(false));
            }
            self.applied_theme = Some(theme.name);
            self.renderer.set_fonts(fonts_for(theme));
            commands.push(Command::SetCaptionColors {
                background: theme.status_background,
                text: theme.primary_text,
                dark: theme.is_dark,
            });
        }
        let title = self.workspace.title();
        if title != self.applied_title {
            commands.push(Command::SetTitle(title.clone()));
            self.applied_title = title;
        }
    }
}

impl Handler for App {
    fn event(&mut self, event: Event, commands: &mut Commands) {
        self.workspace.event(event, &mut self.renderer, commands);
        self.sync_window(commands);
    }

    fn paint(&mut self, commands: &mut Commands) {
        #[cfg(feature = "bench")]
        if let Some(bench) = &mut self.bench {
            bench.frame_started();
            let (width, height) = self.renderer.logical_size();
            let scale = self.renderer.scale();
            bench.set_window_pixels(width * scale, height * scale);
        }
        let size = self.renderer.logical_size();
        let scale = self.renderer.scale();
        self.renderer.begin();
        self.workspace
            .draw(&mut self.renderer, size, scale, commands);
        match self.renderer.end() {
            Ok(FrameResult::Presented) => {}
            Ok(FrameResult::DeviceRecreated) => commands.redraw(),
            Err(_) => commands.redraw(),
        }
        #[cfg(feature = "bench")]
        if let Some(bench) = &mut self.bench {
            let presentation = self.renderer.presentation();
            if let Some(tab) = self.workspace.active_tab_mut() {
                // The benchmark scrolls the view itself.
                tab.following = false;
                let index_bytes = tab
                    .document()
                    .and_then(|document| {
                        document.sources[0]
                            .log
                            .read()
                            .ok()
                            .map(|log| log.index_memory_bytes())
                    })
                    .unwrap_or(0);
                let (view, rows) = tab.split();
                let source = rows
                    .as_ref()
                    .map(|rows| rows as &dyn super::log_view::RowSource);
                bench.after_frame(view, source, index_bytes, presentation, commands);
            } else {
                let mut view = super::log_view::LogView::default();
                bench.after_frame(&mut view, None, 0, presentation, commands);
            }
        }
    }

    fn resized(&mut self, width: u32, height: u32, dpi: f32, commands: &mut Commands) {
        let _ = self.renderer.resize(width, height, dpi);
        self.sync_window(commands);
        commands.accessibility_changed();
    }

    fn woken(&mut self, commands: &mut Commands) {
        self.workspace.woken(commands);
        self.sync_window(commands);
    }

    fn files_chosen(&mut self, paths: Vec<PathBuf>, commands: &mut Commands) {
        self.workspace.open_paths(paths);
        self.sync_window(commands);
        commands.redraw();
        commands.accessibility_changed();
    }

    fn save_path_chosen(&mut self, path: PathBuf, commands: &mut Commands) {
        self.workspace.export_to(path);
        commands.redraw();
    }

    fn system_theme_changed(&mut self, dark: bool, commands: &mut Commands) {
        self.workspace.system_is_dark = dark;
        self.sync_window(commands);
        commands.redraw();
    }

    fn accessibility_tree(&mut self) -> TreeUpdate {
        accessibility::build(&self.workspace, self.renderer.scale())
    }

    fn accessibility_action(&mut self, request: ActionRequest, commands: &mut Commands) {
        accessibility::act(&mut self.workspace, &request, commands);
        self.sync_window(commands);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_are_readable() {
        assert_eq!(readable_size(512), "512 B");
        assert_eq!(readable_size(164 * 1024), "164.0 KB");
        assert_eq!(readable_size(5_368_709_143), "5.0 GB");
    }
}
