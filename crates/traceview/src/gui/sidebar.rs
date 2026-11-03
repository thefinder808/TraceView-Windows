//! The sidebar: open tabs, bookmarks, logs beside the active one, the usual
//! Windows log folders, and recent logs. It's hidden at first, which keeps the
//! window as plain as CMTrace's until someone wants more.

use super::app::readable_size;
use super::places::{FolderJob, LogEntry, Place, PlacesJob};
use super::settings::Settings;
use super::tab::{file_name, Tab};
use std::path::{Path, PathBuf};
use tv_platform::Waker;
use tv_ui::list_view::{ListItem, ListView};
use tv_ui::splitter::{Orientation, Splitter};
use tv_ui::{Canvas, Event, Point, Rect, Theme};

pub const DEFAULT_WIDTH: f32 = 240.0;
const MIN_WIDTH: f32 = 140.0;
/// The logs always keep at least this much room.
const MIN_CONTENT: f32 = 320.0;
/// Bookmarks listed at most, across every open log.
const MOST_BOOKMARKS: usize = 200;
/// Logs in an opened folder are indented under it.
const INDENT: &str = "    ";

/// What choosing an entry does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    Tab(usize),
    Bookmark {
        tab: usize,
        row: usize,
    },
    Open(PathBuf),
    /// Shows or hides a Windows folder's logs.
    Place(usize),
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SidebarResponse {
    pub redraw: bool,
    pub chosen: Option<Target>,
    /// The divider was dragged, and this is where it was let go.
    pub resized: Option<f32>,
}

pub struct Sidebar {
    pub list: ListView,
    splitter: Splitter,
    targets: Vec<Target>,
    places: Vec<Place>,
    places_job: Option<PlacesJob>,
    /// The open Windows folder, and its logs once listed.
    expanded: Option<(usize, Vec<LogEntry>)>,
    expanded_job: Option<FolderJob>,
    nearby_folder: Option<PathBuf>,
    nearby: Vec<LogEntry>,
    nearby_job: Option<FolderJob>,
    pointer: Point,
}

fn folder_label(path: &Path) -> String {
    path.parent()
        .map_or_else(String::new, |folder| folder.display().to_string())
}

/// Adds an entry, with the target its id stands for.
fn push(
    items: &mut Vec<ListItem>,
    targets: &mut Vec<Target>,
    label: &str,
    detail: &str,
    target: Target,
) {
    items.push(ListItem::entry(label, detail, targets.len() as u32));
    targets.push(target);
}

fn push_log(items: &mut Vec<ListItem>, targets: &mut Vec<Target>, log: &LogEntry, indent: &str) {
    let label = format!("{indent}{}", file_name(&log.path));
    push(
        items,
        targets,
        &label,
        &readable_size(log.size),
        Target::Open(log.path.clone()),
    );
}

impl Sidebar {
    pub fn new(width: f32, waker: &Waker) -> Self {
        Self {
            list: ListView::new(Vec::new()),
            splitter: Splitter::new(Orientation::Vertical, width, MIN_WIDTH, MIN_CONTENT),
            targets: Vec::new(),
            places: Vec::new(),
            places_job: Some(PlacesJob::start(waker)),
            expanded: None,
            expanded_job: None,
            nearby_folder: None,
            nearby: Vec::new(),
            nearby_job: None,
            pointer: Point::default(),
        }
    }

    /// The width it's laid out at, for tests to check against.
    #[cfg(test)]
    pub fn width(&self) -> f32 {
        self.splitter.position()
    }

    /// The width chosen, to save. Unlike `width`, it's known before the
    /// sidebar is first shown, and doesn't shrink with the window.
    pub fn chosen_width(&self) -> f32 {
        self.splitter.wanted()
    }

    /// Ignores a width that isn't a number, as a hand-edited file might give.
    pub fn set_width(&mut self, width: f32) {
        if width.is_finite() {
            self.splitter.set_position(width);
        }
    }

    /// What choosing a list item does, for screen readers that choose it.
    pub fn target_of(&self, item: usize) -> Option<Target> {
        let id = self.list.items.get(item)?.id()?;
        self.targets.get(id as usize).cloned()
    }

    /// Picks up finished folder listings. Returns true if anything changed.
    pub fn poll(&mut self) -> bool {
        let mut changed = false;
        if let Some(places) = self.places_job.as_ref().and_then(PlacesJob::poll) {
            self.places = places;
            self.places_job = None;
            changed = true;
        }
        if let Some(outcome) = self.nearby_job.as_ref().and_then(FolderJob::poll) {
            self.nearby = outcome.unwrap_or_default();
            self.nearby_job = None;
            changed = true;
        }
        if let Some(outcome) = self.expanded_job.as_ref().and_then(FolderJob::poll) {
            if let Some((_, logs)) = &mut self.expanded {
                *logs = outcome.unwrap_or_default();
            }
            self.expanded_job = None;
            changed = true;
        }
        changed
    }

    /// Opens or closes a Windows folder in the list.
    pub fn toggle_place(&mut self, index: usize, waker: &Waker) {
        if self
            .expanded
            .as_ref()
            .is_some_and(|(open, _)| *open == index)
        {
            self.expanded = None;
            self.expanded_job = None;
        } else if let Some(place) = self.places.get(index) {
            self.expanded = Some((index, Vec::new()));
            self.expanded_job = Some(FolderJob::start(place.folder.clone(), waker));
        }
    }

    /// Lists what's current. Cheap enough to run every frame. A new active
    /// folder starts listing its logs in the background.
    pub fn rebuild(&mut self, tabs: &[Tab], active: usize, settings: &Settings, waker: &Waker) {
        let folder = tabs.get(active).and_then(|tab| match tab.paths.as_slice() {
            [path] => path.parent().map(Path::to_path_buf),
            _ => None,
        });
        if folder != self.nearby_folder {
            self.nearby.clear();
            self.nearby_job = folder.clone().map(|folder| FolderJob::start(folder, waker));
            self.nearby_folder = folder;
        }

        let mut items = Vec::new();
        let mut targets = Vec::new();
        if !tabs.is_empty() {
            items.push(ListItem::heading("OPEN"));
            for (index, tab) in tabs.iter().enumerate() {
                let detail = match tab.paths.as_slice() {
                    [path] => folder_label(path),
                    paths => format!("{} logs merged", paths.len()),
                };
                push(
                    &mut items,
                    &mut targets,
                    &tab.title(),
                    &detail,
                    Target::Tab(index),
                );
            }
        }

        // Bookmarks come from single-log tabs, which every bookmark belongs to.
        let mut bookmarks = Vec::new();
        for (index, tab) in tabs
            .iter()
            .enumerate()
            .filter(|(_, tab)| tab.paths.len() == 1)
        {
            let Some(source) = tab.document().and_then(|document| document.sources.first()) else {
                continue;
            };
            for row in &source.bookmarks {
                let row = *row as usize;
                let label = match source.cache.get(row) {
                    Some(cells) if !cells.message.is_empty() => {
                        format!("{}: {}", row + 1, cells.message)
                    }
                    _ => format!("Row {}", row + 1),
                };
                bookmarks.push((label, tab.title(), Target::Bookmark { tab: index, row }));
            }
        }
        if !bookmarks.is_empty() {
            items.push(ListItem::heading("BOOKMARKS"));
            for (label, detail, target) in bookmarks.into_iter().take(MOST_BOOKMARKS) {
                push(&mut items, &mut targets, &label, &detail, target);
            }
        }

        if !self.nearby.is_empty() {
            items.push(ListItem::heading("NEARBY"));
            for log in &self.nearby {
                push_log(&mut items, &mut targets, log, "");
            }
        }

        if !self.places.is_empty() {
            items.push(ListItem::heading("WINDOWS"));
            for (index, place) in self.places.iter().enumerate() {
                push(
                    &mut items,
                    &mut targets,
                    place.name,
                    &place.folder.display().to_string(),
                    Target::Place(index),
                );
                if let Some((open, logs)) = &self.expanded {
                    if *open == index {
                        for log in logs {
                            push_log(&mut items, &mut targets, log, INDENT);
                        }
                    }
                }
            }
        }

        if !settings.recent.is_empty() {
            items.push(ListItem::heading("RECENT"));
            for recent in &settings.recent {
                let path = Path::new(recent);
                push(
                    &mut items,
                    &mut targets,
                    &file_name(path),
                    &folder_label(path),
                    Target::Open(path.to_path_buf()),
                );
            }
        }
        self.list.items = items;
        self.targets = targets;
    }

    /// Splits `area` into the sidebar and what's left for the logs.
    pub fn arrange(&mut self, area: Rect) -> Rect {
        self.splitter.arrange(area);
        self.list.arrange(self.splitter.first_rect());
        self.splitter.second_rect()
    }

    pub fn contains(&self, point: Point) -> bool {
        self.list.contains(point) || self.splitter.contains(point)
    }

    pub fn is_dragging(&self) -> bool {
        self.splitter.is_dragging()
    }

    pub fn shows_resize_cursor(&self) -> bool {
        self.splitter.shows_resize_cursor()
    }

    /// Handles input meant for the sidebar. Keys only come here while it has focus.
    pub fn handle(&mut self, event: Event) -> SidebarResponse {
        let mut response = SidebarResponse::default();
        if let Event::MouseMove(point) = event {
            self.pointer = point;
        }
        let to_splitter = match event {
            Event::MouseDown(point, ..) => self.splitter.contains(point),
            Event::MouseMove(_) | Event::MouseUp(..) => true,
            _ => false,
        };
        if to_splitter {
            let was_dragging = self.splitter.is_dragging();
            let split = self.splitter.handle(event);
            response.redraw |= split.redraw;
            if was_dragging && !self.splitter.is_dragging() {
                response.resized = Some(self.splitter.position());
            }
            if was_dragging || matches!(event, Event::MouseDown(..)) {
                return response;
            }
        }
        let to_list = match event {
            Event::Wheel(_) => self.list.contains(self.pointer),
            Event::MouseDown(point, ..) => self.list.contains(point),
            _ => true,
        };
        if to_list {
            let listed = self.list.handle(event);
            response.redraw |= listed.redraw;
            response.chosen = listed
                .activated
                .and_then(|id| self.targets.get(id as usize).cloned());
        }
        response
    }

    pub fn draw(&self, canvas: &mut dyn Canvas, theme: &Theme, focused: bool) {
        self.list.draw(canvas, theme, focused);
        self.splitter.draw(canvas, theme, false);
    }
}
