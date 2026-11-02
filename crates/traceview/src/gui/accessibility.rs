//! Describes the window to screen readers: the menu bar, tabs, filter and find
//! boxes, the grid, the details pane and the status bar.
//!
//! The grid is exposed as a list, because AccessKit's Windows adapter gives table
//! rows no selection support, and a list also announces "item 2 of 26,904,325".
//! It's virtual, so only the rows on screen exist as nodes, and each one's text
//! reads as a complete sentence.

use super::log_view::{level_name, LogView, RowSource};
use super::workspace::{Focus, Workspace};
use accesskit::{
    Action, ActionRequest, Node, NodeId, Rect as AccessRect, Role, TreeId, TreeInfo, TreeUpdate,
};
use tv_platform::Commands;
use tv_ui::list_view::ListItem;
use tv_ui::menu::MenuItem;
use tv_ui::Rect;

const WINDOW_ID: NodeId = NodeId(1);
pub const GRID_ID: NodeId = NodeId(2);
const STATUS_ID: NodeId = NodeId(3);
const MENU_BAR_ID: NodeId = NodeId(4);
const TAB_LIST_ID: NodeId = NodeId(5);
const FILTER_ID: NodeId = NodeId(6);
const FIND_ID: NodeId = NodeId(7);
const DETAILS_ID: NodeId = NodeId(8);
const MENU_POPUP_ID: NodeId = NodeId(9);
const LOOKUP_ID: NodeId = NodeId(10);
const LOOKUP_FIELD_ID: NodeId = NodeId(11);
const LOOKUP_RESULT_ID: NodeId = NodeId(12);
const FIRST_MENU_ID: u64 = 100;
const FIRST_MENU_ITEM_ID: u64 = 200;
const FIRST_TAB_ID: u64 = 300;
const SIDEBAR_ID: NodeId = NodeId(13);
const HISTOGRAM_ID: NodeId = NodeId(14);
const FIRST_CHIP_ID: u64 = 400;
const FIRST_SIDEBAR_ID: u64 = 10_000;
/// Rows come last, so any row number fits after this.
const FIRST_ROW_ID: u64 = 1_000_000;
/// Screen readers read the details pane's text, so it's capped to stay usable.
const MAX_DETAIL_CHARS: usize = 2000;

fn row_id(row: usize) -> NodeId {
    NodeId(FIRST_ROW_ID + row as u64)
}

/// The view row a node stands for, if it's a row node.
pub fn row_from_id(id: NodeId) -> Option<usize> {
    id.0.checked_sub(FIRST_ROW_ID).map(|row| row as usize)
}

fn in_range(id: NodeId, first: u64, count: usize) -> Option<usize> {
    id.0.checked_sub(first)
        .map(|index| index as usize)
        .filter(|index| *index < count)
}

/// Accessibility positions are in physical pixels.
fn physical(rect: Rect, scale: f32) -> AccessRect {
    AccessRect::new(
        f64::from(rect.left * scale),
        f64::from(rect.top * scale),
        f64::from(rect.right * scale),
        f64::from(rect.bottom * scale),
    )
}

fn row_label(source: &dyn RowSource, row: usize) -> String {
    let mut level = level_name(source.level(row)).to_string();
    if source.bookmarked(row) {
        level.push_str(", bookmarked");
    }
    let number = source.file_row(row) + 1;
    match source.cells(row) {
        Some(cells) => {
            let mut label = format!("Row {number}, {level}");
            for part in [&cells.time, &cells.component, &cells.message] {
                if !part.is_empty() {
                    label.push_str(", ");
                    label.push_str(part);
                }
            }
            label
        }
        None => format!("Row {number}, {level}, loading"),
    }
}

/// The grid's nodes: the list and its visible rows. Returns the list's node and
/// the node that should have focus within it.
fn grid_nodes(
    view: &LogView,
    source: Option<&dyn RowSource>,
    scale: f32,
    nodes: &mut Vec<(NodeId, Node)>,
) -> NodeId {
    let mut row_ids = Vec::new();
    if let Some(source) = source {
        let selection = view.selection();
        for row in view.visible_rows() {
            let id = row_id(row);
            row_ids.push(id);
            let mut node = Node::new(Role::ListBoxOption);
            node.set_label(row_label(source, row));
            node.set_position_in_set(row + 1);
            node.set_size_of_set(source.row_count());
            node.set_bounds(physical(view.row_rect(row), scale));
            node.set_selected(selection.as_ref().is_some_and(|range| range.contains(&row)));
            node.add_action(Action::Focus);
            node.add_action(Action::Click);
            nodes.push((id, node));
        }
    }
    let mut grid = Node::new(Role::ListBox);
    grid.set_label("Log rows");
    grid.set_multiselectable();
    grid.set_bounds(physical(view.body_rect(), scale));
    grid.set_children(row_ids);
    grid.add_action(Action::Focus);
    grid.add_action(Action::ScrollUp);
    grid.add_action(Action::ScrollDown);
    nodes.push((GRID_ID, grid));

    let visible = view.visible_rows();
    view.cursor()
        .filter(|row| source.is_some() && visible.contains(row))
        .map_or(GRID_ID, row_id)
}

pub fn build(workspace: &Workspace, scale: f32) -> TreeUpdate {
    let mut nodes = Vec::new();
    let mut window_children = vec![MENU_BAR_ID];

    // The menu bar, and the open menu's items.
    let menu = workspace.menu();
    let mut menu_ids = Vec::new();
    let mut menu_focus = None;
    for (index, entry) in menu.menus.iter().enumerate() {
        let id = NodeId(FIRST_MENU_ID + index as u64);
        menu_ids.push(id);
        let mut node = Node::new(Role::MenuItem);
        node.set_label(entry.title.as_str());
        node.add_action(Action::Click);
        if menu.open_menu() == Some(index) {
            node.set_expanded(true);
            let mut item_ids = Vec::new();
            for (item_index, item) in entry.items.iter().enumerate() {
                if let MenuItem::Command {
                    label,
                    shortcut,
                    enabled,
                    ..
                } = item
                {
                    let item_id = NodeId(FIRST_MENU_ITEM_ID + item_index as u64);
                    item_ids.push(item_id);
                    let mut item_node = Node::new(Role::MenuItem);
                    item_node.set_label(label.as_str());
                    if !shortcut.is_empty() {
                        item_node.set_keyboard_shortcut(shortcut.as_str());
                    }
                    if *enabled {
                        item_node.add_action(Action::Click);
                    } else {
                        item_node.set_disabled();
                    }
                    if menu.highlighted_item() == Some(item_index) {
                        menu_focus = Some(item_id);
                    }
                    nodes.push((item_id, item_node));
                }
            }
            let mut popup = Node::new(Role::Menu);
            popup.set_label(entry.title.as_str());
            popup.set_children(item_ids);
            nodes.push((MENU_POPUP_ID, popup));
            node.set_children(vec![MENU_POPUP_ID]);
            menu_focus = menu_focus.or(Some(id));
        }
        nodes.push((id, node));
    }
    let mut bar = Node::new(Role::MenuBar);
    bar.set_children(menu_ids);
    nodes.push((MENU_BAR_ID, bar));

    // Tabs.
    let titles = workspace.tab_titles();
    if !titles.is_empty() {
        let mut tab_ids = Vec::new();
        for (index, title) in titles.iter().enumerate() {
            let id = NodeId(FIRST_TAB_ID + index as u64);
            tab_ids.push(id);
            let mut node = Node::new(Role::Tab);
            node.set_label(title.as_str());
            node.set_selected(index == workspace.active);
            node.add_action(Action::Click);
            nodes.push((id, node));
        }
        let mut list = Node::new(Role::TabList);
        list.set_children(tab_ids);
        nodes.push((TAB_LIST_ID, list));
        window_children.push(TAB_LIST_ID);
    }

    // The active tab: its boxes, grid and details.
    let mut grid_focus = GRID_ID;
    if let Some(tab) = workspace.active_tab() {
        if tab.document().is_some() {
            for (id, label, text, rect) in [
                (
                    FILTER_ID,
                    "Filter",
                    tab.filter_field.text(),
                    workspace.filter_rect(),
                ),
                (
                    FIND_ID,
                    "Find",
                    tab.find_field.text(),
                    workspace.find_rect(),
                ),
            ] {
                let mut node = Node::new(Role::TextInput);
                node.set_label(label);
                node.set_value(text);
                node.set_bounds(physical(rect, scale));
                node.add_action(Action::Focus);
                nodes.push((id, node));
                window_children.push(id);
            }
        }
        // Severity chips are toggle buttons, and the histogram a labelled picture.
        for (index, (chip, rect)) in workspace.chip_list().enumerate() {
            let Some(rect) = rect else { continue };
            let id = NodeId(FIRST_CHIP_ID + index as u64);
            let mut node = Node::new(Role::Button);
            node.set_label(format!("{}, {}", chip.label, chip.count));
            node.set_toggled(chip.active.into());
            node.set_bounds(physical(rect, scale));
            node.add_action(Action::Click);
            nodes.push((id, node));
            window_children.push(id);
        }
        if let (Some(histogram), true) = (
            &tab.overview.counts.histogram,
            workspace.histogram_rect().height() > 0.0,
        ) {
            let first = tv_engine::time::Stamp::from_seconds(histogram.range.first).label();
            let last = tv_engine::time::Stamp::from_seconds(histogram.range.last).label();
            let mut node = Node::new(Role::Image);
            node.set_label(format!("Event histogram, {first} to {last}"));
            node.set_bounds(physical(workspace.histogram_rect(), scale));
            nodes.push((HISTOGRAM_ID, node));
            window_children.push(HISTOGRAM_ID);
        }
        let rows = tab.rows();
        grid_focus = grid_nodes(
            &tab.view,
            rows.as_ref().map(|rows| rows as &dyn RowSource),
            scale,
            &mut nodes,
        );
        window_children.push(GRID_ID);
        if let Some(detail) = tab.details.shown() {
            let mut node = Node::new(Role::Group);
            node.set_label("Details");
            node.set_description(
                detail
                    .message
                    .chars()
                    .take(MAX_DETAIL_CHARS)
                    .collect::<String>(),
            );
            nodes.push((DETAILS_ID, node));
            window_children.push(DETAILS_ID);
        }
    } else {
        grid_nodes(&LogView::default(), None, scale, &mut nodes);
        window_children.push(GRID_ID);
    }

    if let Some(panel) = &workspace.lookup {
        let mut field = Node::new(Role::TextInput);
        field.set_label("Error code or log line");
        field.set_value(panel.field.text());
        field.set_bounds(physical(panel.field_rect(), scale));
        nodes.push((LOOKUP_FIELD_ID, field));
        let mut result = Node::new(Role::Label);
        result.set_value(
            panel
                .result()
                .chars()
                .take(MAX_DETAIL_CHARS)
                .collect::<String>(),
        );
        nodes.push((LOOKUP_RESULT_ID, result));
        let mut dialog = Node::new(Role::Dialog);
        dialog.set_label("Error lookup");
        dialog.set_modal();
        dialog.set_bounds(physical(panel.rect(), scale));
        dialog.set_children(vec![LOOKUP_FIELD_ID, LOOKUP_RESULT_ID]);
        nodes.push((LOOKUP_ID, dialog));
        window_children.push(LOOKUP_ID);
    }

    // The sidebar's visible entries, with their section headings.
    let mut sidebar_focus = SIDEBAR_ID;
    if workspace.show_sidebar {
        let list = &workspace.sidebar.list;
        let mut ids = Vec::new();
        for index in list.visible_items() {
            let Some(rect) = list.item_rect(index) else {
                continue;
            };
            let id = NodeId(FIRST_SIDEBAR_ID + index as u64);
            let mut node = match &list.items[index] {
                ListItem::Heading(text) => {
                    let mut node = Node::new(Role::Heading);
                    node.set_label(text.as_str());
                    node
                }
                ListItem::Entry { label, detail, .. } => {
                    let mut node = Node::new(Role::ListItem);
                    let label = label.trim();
                    node.set_label(if detail.is_empty() {
                        label.to_string()
                    } else {
                        format!("{label}, {detail}")
                    });
                    node.set_selected(list.selected() == Some(index));
                    node.add_action(Action::Click);
                    node.add_action(Action::Focus);
                    if list.selected() == Some(index) {
                        sidebar_focus = id;
                    }
                    node
                }
            };
            node.set_bounds(physical(rect, scale));
            ids.push(id);
            nodes.push((id, node));
        }
        let mut sidebar = Node::new(Role::List);
        sidebar.set_label("Sidebar");
        sidebar.set_children(ids);
        nodes.push((SIDEBAR_ID, sidebar));
        window_children.push(SIDEBAR_ID);
    }

    let mut status = Node::new(Role::Status);
    status.set_label(workspace.status());
    nodes.push((STATUS_ID, status));
    window_children.push(STATUS_ID);

    let mut window = Node::new(Role::Window);
    window.set_label(workspace.title());
    window.set_children(window_children);
    nodes.push((WINDOW_ID, window));

    let has_fields = workspace
        .active_tab()
        .is_some_and(|tab| tab.document().is_some());
    let focus = menu_focus.unwrap_or(match workspace.focus {
        _ if workspace.lookup.is_some() => LOOKUP_FIELD_ID,
        Focus::Sidebar if workspace.show_sidebar => sidebar_focus,
        Focus::Filter if has_fields => FILTER_ID,
        Focus::Find if has_fields => FIND_ID,
        _ => grid_focus,
    });
    TreeUpdate {
        nodes,
        tree: Some(TreeInfo::new(WINDOW_ID)),
        tree_id: TreeId::ROOT,
        focus,
    }
}

/// Carries out a request from a screen reader or other assistive technology.
pub fn act(workspace: &mut Workspace, request: &ActionRequest, commands: &mut Commands) {
    let target = request.target_node;
    match request.action {
        Action::Focus | Action::Click => {
            if let Some(row) = row_from_id(target) {
                if let Some(tab) = workspace.active_tab_mut() {
                    tab.view.select(row, false);
                    tab.following = tab.view.at_bottom();
                }
            } else if let Some(index) =
                in_range(target, FIRST_CHIP_ID, workspace.chip_list().count())
            {
                workspace.toggle_chip(index);
            } else if let Some(index) =
                in_range(target, FIRST_SIDEBAR_ID, workspace.sidebar.list.items.len())
            {
                if let Some(chosen) = workspace.sidebar.target_of(index) {
                    workspace.choose(chosen, commands);
                }
            } else if let Some(index) = in_range(target, FIRST_TAB_ID, workspace.tabs.len()) {
                workspace.active = index;
            } else if target == FILTER_ID {
                workspace.run_command(super::workspace::command::FILTER, commands);
            } else if target == FIND_ID {
                workspace.run_command(super::workspace::command::FIND, commands);
            } else if let Some(open) = workspace.menu().open_menu() {
                let items = workspace.menu().menus[open].items.len();
                if let Some(item) = in_range(target, FIRST_MENU_ITEM_ID, items) {
                    if let MenuItem::Command {
                        command,
                        enabled: true,
                        ..
                    } = workspace.menu().menus[open].items[item]
                    {
                        workspace.close_menu();
                        workspace.run_command(command, commands);
                    }
                }
            }
        }
        Action::ScrollUp | Action::ScrollDown => {
            if let Some(tab) = workspace.active_tab_mut() {
                let page = tab.view.page_rows() as f64;
                let delta = if request.action == Action::ScrollUp {
                    -page
                } else {
                    page
                };
                tab.view.scroll_to(tab.view.top_row() as f64 + delta);
                tab.following = tab.view.at_bottom();
            }
        }
        _ => return,
    }
    commands.redraw();
    commands.accessibility_changed();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};
    use tv_platform::Waker;
    use tv_ui::recording::RecordingCanvas;
    use tv_ui::{Event, Key, Modifiers};

    fn node(update: &TreeUpdate, id: NodeId) -> &Node {
        &update
            .nodes
            .iter()
            .find(|(node_id, _)| *node_id == id)
            .unwrap()
            .1
    }

    fn opened() -> (tempfile::TempDir, Workspace) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("app.log");
        let text: String = (0..1000)
            .map(|row| {
                format!(
                    "2026-01-01 10:00:00 host app[1]: {}: disk full {row}\n",
                    if row == 1 { "ERROR" } else { "INFO" }
                )
            })
            .collect();
        std::fs::write(&path, text).unwrap();
        let mut workspace = Workspace::new(Waker::detached(), 0, false);
        workspace.open_paths(vec![path]);
        // These tests read the first rows, so stay at the top rather than following.
        workspace.tabs[0].following = false;
        let started = Instant::now();
        loop {
            let mut commands = Commands::default();
            workspace.woken(&mut commands);
            workspace.draw(
                &mut RecordingCanvas::new(),
                (1000.0, 600.0),
                1.0,
                &mut commands,
            );
            let loaded = workspace
                .active_tab()
                .and_then(|tab| tab.rows())
                .is_some_and(|rows| rows.cells(1).is_some());
            if loaded {
                break;
            }
            assert!(started.elapsed() < Duration::from_secs(10));
            std::thread::sleep(Duration::from_millis(5));
        }
        (dir, workspace)
    }

    #[test]
    fn rows_read_as_full_sentences() {
        let (_dir, workspace) = opened();
        let update = build(&workspace, 1.0);
        assert_eq!(
            node(&update, row_id(1)).label(),
            Some("Row 2, Error, 2026-01-01 10:00:00.000, app, disk full 1")
        );
        assert_eq!(node(&update, row_id(0)).size_of_set(), Some(1000));
    }

    #[test]
    fn only_visible_rows_become_nodes() {
        let (_dir, workspace) = opened();
        let update = build(&workspace, 1.0);
        let visible = workspace.active_tab().unwrap().view.visible_rows().len();
        assert_eq!(node(&update, GRID_ID).children().len(), visible);
    }

    #[test]
    fn focus_follows_the_selected_row_then_the_boxes() {
        let (_dir, mut workspace) = opened();
        assert_eq!(build(&workspace, 1.0).focus, GRID_ID);
        workspace.tabs[0].view.select(3, false);
        let update = build(&workspace, 1.0);
        assert_eq!(update.focus, row_id(3));
        assert_eq!(node(&update, row_id(3)).is_selected(), Some(true));
        workspace.event(
            Event::KeyDown(
                Key::Char('F'),
                Modifiers {
                    ctrl: true,
                    shift: true,
                    alt: false,
                },
            ),
            &mut RecordingCanvas::new(),
            &mut Commands::default(),
        );
        assert_eq!(build(&workspace, 1.0).focus, FILTER_ID);
    }

    #[test]
    fn boxes_tabs_and_menus_are_exposed() {
        let (_dir, workspace) = opened();
        let update = build(&workspace, 1.0);
        assert_eq!(node(&update, FILTER_ID).label(), Some("Filter"));
        assert_eq!(node(&update, FIND_ID).label(), Some("Find"));
        assert_eq!(node(&update, NodeId(FIRST_TAB_ID)).label(), Some("app.log"));
        assert_eq!(
            node(&update, NodeId(FIRST_TAB_ID)).is_selected(),
            Some(true)
        );
        assert_eq!(node(&update, NodeId(FIRST_MENU_ID)).label(), Some("File"));
    }

    #[test]
    fn an_open_menu_lists_its_items_and_takes_focus() {
        let (_dir, mut workspace) = opened();
        workspace.event(
            Event::KeyDown(
                Key::Char('F'),
                Modifiers {
                    alt: true,
                    ..Modifiers::default()
                },
            ),
            &mut RecordingCanvas::new(),
            &mut Commands::default(),
        );
        let update = build(&workspace, 1.0);
        let first_item = NodeId(FIRST_MENU_ITEM_ID);
        assert_eq!(node(&update, first_item).label(), Some("Open..."));
        assert_eq!(
            node(&update, first_item).keyboard_shortcut(),
            Some("Ctrl+O")
        );
        assert_eq!(update.focus, first_item);
    }

    #[test]
    fn screen_reader_actions_select_rows_and_tabs() {
        let (_dir, mut workspace) = opened();
        let mut commands = Commands::default();
        let request = ActionRequest {
            action: Action::Click,
            target_tree: TreeId::ROOT,
            target_node: row_id(5),
            data: None,
        };
        act(&mut workspace, &request, &mut commands);
        assert_eq!(workspace.tabs[0].view.cursor(), Some(5));
    }

    #[test]
    fn with_no_log_the_tree_is_still_valid() {
        let workspace = Workspace::new(Waker::detached(), 0, false);
        let update = build(&workspace, 1.0);
        assert_eq!(update.focus, GRID_ID);
        assert!(update.nodes.iter().any(|(id, _)| *id == STATUS_ID));
    }

    #[test]
    fn bounds_use_physical_pixels() {
        let (_dir, workspace) = opened();
        let update = build(&workspace, 2.0);
        let bounds = node(&update, GRID_ID).bounds().unwrap();
        let body = workspace.active_tab().unwrap().view.body_rect();
        assert_eq!(bounds.x1, 2.0 * f64::from(body.right));
    }

    #[test]
    fn the_lookup_panel_is_a_modal_dialog_with_focus() {
        let mut workspace = Workspace::new(Waker::detached(), 0, false);
        workspace.run_command(
            super::super::workspace::command::ERROR_LOOKUP,
            &mut Commands::default(),
        );
        let update = build(&workspace, 1.0);
        assert_eq!(update.focus, LOOKUP_FIELD_ID);
        assert_eq!(node(&update, LOOKUP_ID).role(), Role::Dialog);
        assert!(node(&update, LOOKUP_ID).is_modal());
    }

    #[test]
    fn row_ids_map_back_to_rows() {
        assert_eq!(row_from_id(row_id(42)), Some(42));
        assert_eq!(row_from_id(GRID_ID), None);
    }
}
