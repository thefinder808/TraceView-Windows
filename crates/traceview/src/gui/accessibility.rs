//! Describes the window to screen readers: the menu bar, tabs, filter and find
//! boxes, the grid, the details pane and the status bar.
//!
//! The grid is exposed as a list, because AccessKit's Windows adapter gives table
//! rows no selection support, and a list also announces "item 2 of 26,904,325".
//! It's virtual, so only the rows on screen exist as nodes, and each one's text
//! reads as a complete sentence.

use super::columns::Column;
use super::columns_panel::{self, ColumnsPanel, Part as ColumnsPart};
use super::fonts_panel::{self, FontsPanel, Part as FontsPart};
use super::formats_panel::{self, FormatsPanel, FormatsResponse, Part as FormatsPart};
use super::log_view::{level_name, LogView, RowSource};
use super::rules::Style;
use super::rules_panel::{self, Part, RulesPanel};
use super::shortcut_panel::{self, ShortcutPanel};
use super::workspace::{Focus, Workspace};
use accesskit::{
    Action, ActionRequest, Node, NodeId, Rect as AccessRect, Role, TreeId, TreeInfo, TreeUpdate,
};
use tv_platform::Commands;
use tv_ui::list_view::ListItem;
use tv_ui::menu::MenuItem;
use tv_ui::theme::RULE_COLOR_NAMES;
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
const PALETTE_ID: NodeId = NodeId(15);
const PALETTE_FIELD_ID: NodeId = NodeId(16);
const PALETTE_LIST_ID: NodeId = NodeId(17);
const FIRST_PALETTE_ID: u64 = 5_000;
const RULES_ID: NodeId = NodeId(18);
const RULES_FIELD_ID: NodeId = NodeId(19);
const RULES_LIST_ID: NodeId = NodeId(20);
const RULES_NOTE_ID: NodeId = NodeId(21);
const FIRST_RULE_COLOR_ID: u64 = 600;
const FIRST_RULE_OPTION_ID: u64 = 620;
const FIRST_RULE_ID: u64 = 7_000;
const COLUMNS_ID: NodeId = NodeId(22);
const COLUMNS_LIST_ID: NodeId = NodeId(23);
const FIRST_COLUMN_BUTTON_ID: u64 = 640;
const FIRST_COLUMN_ITEM_ID: u64 = 7_200;
const FONTS_ID: NodeId = NodeId(24);
const FONTS_LIST_ID: NodeId = NodeId(25);
const FIRST_FONT_BUTTON_ID: u64 = 660;
const FIRST_FONT_ITEM_ID: u64 = 7_300;
const SHORTCUT_ID: NodeId = NodeId(26);
const FIRST_SHORTCUT_BUTTON_ID: u64 = 680;
const FORMATS_ID: NodeId = NodeId(27);
const FORMATS_FIELD_ID: NodeId = NodeId(28);
const FORMATS_LIST_ID: NodeId = NodeId(29);
const FORMATS_NOTE_ID: NodeId = NodeId(30);
const FIRST_FORMAT_OPTION_ID: u64 = 700;
const FIRST_FORMAT_ITEM_ID: u64 = 7_400;
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

/// A row as one sentence: its number and severity, then the columns on screen
/// in their order. The severity is said even when its column is hidden, as the
/// row's color shows it anyway.
fn row_label(source: &dyn RowSource, row: usize, columns: &[Column]) -> String {
    let mut level = level_name(source.level(row)).to_string();
    if source.bookmarked(row) {
        level.push_str(", bookmarked");
    }
    let number = source.file_row(row) + 1;
    match source.cells(row) {
        Some(cells) => {
            if let Some(mark) = cells.mark {
                let color = RULE_COLOR_NAMES
                    .get(mark.color)
                    .map_or_else(String::new, |name| name.to_lowercase());
                level.push_str(&match mark.style {
                    Style::Row => format!(", highlighted {color}"),
                    Style::Marker => format!(", marked {color}"),
                });
            }
            let mut label = format!("Row {number}, {level}");
            for part in columns.iter().map(|column| column.text(&cells)) {
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
        let columns: Vec<Column> = view.shown_columns().collect();
        for row in view.visible_rows() {
            let id = row_id(row);
            row_ids.push(id);
            let mut node = Node::new(Role::ListBoxOption);
            node.set_label(row_label(source, row, &columns));
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

/// The columns editor, as a modal dialog. Returns the node with focus.
fn columns_nodes(panel: &ColumnsPanel, scale: f32, nodes: &mut Vec<(NodeId, Node)>) -> NodeId {
    let list = &panel.list;
    let mut focus = COLUMNS_LIST_ID;
    let mut options = Vec::new();
    for index in list.visible_items() {
        let (Some(rect), Some(spec)) = (list.item_rect(index), panel.columns.get(index)) else {
            continue;
        };
        let id = NodeId(FIRST_COLUMN_ITEM_ID + index as u64);
        let mut node = Node::new(Role::ListBoxOption);
        node.set_label(format!(
            "{}, {}",
            spec.column.label(),
            columns_panel::describe(spec)
        ));
        node.set_selected(panel.selected() == index);
        node.set_bounds(physical(rect, scale));
        node.add_action(Action::Focus);
        node.add_action(Action::Click);
        if panel.focus == ColumnsPart::List && panel.selected() == index {
            focus = id;
        }
        options.push(id);
        nodes.push((id, node));
    }
    let mut list_node = Node::new(Role::ListBox);
    list_node.set_label("Columns, left to right");
    list_node.set_children(options);
    nodes.push((COLUMNS_LIST_ID, list_node));
    let mut children = vec![COLUMNS_LIST_ID];

    for (index, chip) in panel.button_chips().iter().enumerate() {
        let Some(rect) = panel.buttons.chip_rect(index) else {
            continue;
        };
        let id = NodeId(FIRST_COLUMN_BUTTON_ID + index as u64);
        let mut node = Node::new(Role::Button);
        node.set_label(chip.label.as_str());
        if index == 0 {
            node.set_toggled(chip.active.into());
        }
        if panel.button_applies(index) {
            node.add_action(Action::Click);
        } else {
            node.set_disabled();
        }
        node.add_action(Action::Focus);
        node.set_bounds(physical(rect, scale));
        if panel.focus == ColumnsPart::Buttons && panel.buttons.focus() == Some(index) {
            focus = id;
        }
        children.push(id);
        nodes.push((id, node));
    }
    let mut dialog = Node::new(Role::Dialog);
    dialog.set_label(panel.title());
    dialog.set_modal();
    dialog.set_bounds(physical(panel.rect(), scale));
    dialog.set_children(children);
    nodes.push((COLUMNS_ID, dialog));
    focus
}

/// Carries out a screen reader's request in the columns editor. Clicking a
/// button presses it, and focusing a column selects it.
fn act_on_columns(workspace: &mut Workspace, request: &ActionRequest) {
    let Some(panel) = workspace.columns_panel.as_mut() else {
        return;
    };
    let target = request.target_node;
    let changed =
        if let Some(index) = in_range(target, FIRST_COLUMN_BUTTON_ID, panel.button_chips().len()) {
            panel.focus = ColumnsPart::Buttons;
            request.action == Action::Click && panel.press(index)
        } else if let Some(index) = in_range(target, FIRST_COLUMN_ITEM_ID, panel.columns.len()) {
            panel.focus = ColumnsPart::List;
            panel.select(index);
            false
        } else {
            false
        };
    if changed {
        workspace.columns_edited();
    }
}

/// The fonts editor, as a modal dialog. Returns the node with focus.
fn fonts_nodes(panel: &FontsPanel, scale: f32, nodes: &mut Vec<(NodeId, Node)>) -> NodeId {
    let list = &panel.list;
    let mut focus = FONTS_LIST_ID;
    let mut options = Vec::new();
    for index in list.visible_items() {
        let (Some(rect), ListItem::Entry { label, detail, .. }) =
            (list.item_rect(index), &list.items[index])
        else {
            continue;
        };
        let id = NodeId(FIRST_FONT_ITEM_ID + index as u64);
        let selected = list.selected() == Some(index);
        let mut node = Node::new(Role::ListBoxOption);
        node.set_label(format!("{label}, {detail}"));
        node.set_selected(selected);
        node.set_bounds(physical(rect, scale));
        node.add_action(Action::Focus);
        node.add_action(Action::Click);
        if panel.focus == FontsPart::List && selected {
            focus = id;
        }
        options.push(id);
        nodes.push((id, node));
    }
    let mut list_node = Node::new(Role::ListBox);
    list_node.set_label("Fonts and spacing");
    list_node.set_children(options);
    nodes.push((FONTS_LIST_ID, list_node));
    let mut children = vec![FONTS_LIST_ID];
    for (index, chip) in panel.button_chips().iter().enumerate() {
        let Some(rect) = panel.buttons.chip_rect(index) else {
            continue;
        };
        let id = NodeId(FIRST_FONT_BUTTON_ID + index as u64);
        let mut node = Node::new(Role::Button);
        node.set_label(chip.label.as_str());
        if panel.button_applies(index) {
            node.add_action(Action::Click);
        } else {
            node.set_disabled();
        }
        node.add_action(Action::Focus);
        node.set_bounds(physical(rect, scale));
        if panel.focus == FontsPart::Buttons && panel.buttons.focus() == Some(index) {
            focus = id;
        }
        children.push(id);
        nodes.push((id, node));
    }
    let mut dialog = Node::new(Role::Dialog);
    dialog.set_label(fonts_panel::TITLE);
    dialog.set_modal();
    dialog.set_bounds(physical(panel.rect(), scale));
    dialog.set_children(children);
    nodes.push((FONTS_ID, dialog));
    focus
}

/// Carries out a screen reader's request in the fonts editor. Clicking a
/// button presses it, and focusing a row selects it.
fn act_on_fonts(workspace: &mut Workspace, request: &ActionRequest, commands: &mut Commands) {
    let Some(panel) = workspace.fonts_panel.as_mut() else {
        return;
    };
    let target = request.target_node;
    if let Some(index) = in_range(target, FIRST_FONT_BUTTON_ID, panel.button_chips().len()) {
        panel.focus = FontsPart::Buttons;
        if request.action == Action::Click {
            let response = panel.press(index);
            workspace.fonts_responded(response, commands);
        }
    } else if let Some(index) = in_range(target, FIRST_FONT_ITEM_ID, panel.list.items.len()) {
        panel.focus = FontsPart::List;
        panel.list.select_id(Some(index as u32));
    }
}

/// The log formats editor, as a modal dialog. Returns the node with focus.
fn formats_nodes(panel: &FormatsPanel, scale: f32, nodes: &mut Vec<(NodeId, Node)>) -> NodeId {
    let mut field = Node::new(Role::TextInput);
    field.set_label(panel.property().label());
    field.set_value(panel.field.text());
    field.set_bounds(physical(panel.field_rect(), scale));
    field.add_action(Action::Focus);
    nodes.push((FORMATS_FIELD_ID, field));
    let mut children = vec![FORMATS_FIELD_ID];
    let mut focus = FORMATS_FIELD_ID;

    // Name, pattern and files are a choice of one, day first stays pressed,
    // and the rest are buttons.
    for (index, chip) in panel.option_chips().iter().enumerate() {
        let Some(rect) = panel.options.chip_rect(index) else {
            continue;
        };
        let id = NodeId(FIRST_FORMAT_OPTION_ID + index as u64);
        let mut node = Node::new(if index < 3 {
            Role::RadioButton
        } else {
            Role::Button
        });
        node.set_label(chip.label.as_str());
        if index < 4 {
            node.set_toggled(chip.active.into());
        }
        if panel.option_applies(index) {
            node.add_action(Action::Click);
        } else {
            node.set_disabled();
        }
        node.add_action(Action::Focus);
        node.set_bounds(physical(rect, scale));
        if panel.focus == FormatsPart::Options && panel.options.focus() == Some(index) {
            focus = id;
        }
        children.push(id);
        nodes.push((id, node));
    }

    let list = &panel.list;
    let mut options = Vec::new();
    for index in list.visible_items() {
        let (Some(rect), ListItem::Entry { label, detail, .. }) =
            (list.item_rect(index), &list.items[index])
        else {
            continue;
        };
        let id = NodeId(FIRST_FORMAT_ITEM_ID + index as u64);
        let mut node = Node::new(Role::ListBoxOption);
        node.set_label(format!("{label}, {detail}"));
        node.set_selected(panel.selected() == index);
        node.set_bounds(physical(rect, scale));
        node.add_action(Action::Focus);
        node.add_action(Action::Click);
        if panel.focus == FormatsPart::List && panel.selected() == index {
            focus = id;
        }
        options.push(id);
        nodes.push((id, node));
    }
    let mut list_node = Node::new(Role::ListBox);
    list_node.set_label("Formats, tried in order");
    list_node.set_children(options);
    nodes.push((FORMATS_LIST_ID, list_node));
    children.push(FORMATS_LIST_ID);

    let note = match panel.problem() {
        Some(problem) => problem.to_string(),
        None => panel.test().join(". "),
    };
    if !note.is_empty() {
        let mut node = Node::new(Role::Label);
        node.set_value(note);
        nodes.push((FORMATS_NOTE_ID, node));
        children.push(FORMATS_NOTE_ID);
    }
    let mut dialog = Node::new(Role::Dialog);
    dialog.set_label(formats_panel::TITLE);
    dialog.set_modal();
    dialog.set_bounds(physical(panel.rect(), scale));
    dialog.set_children(children);
    nodes.push((FORMATS_ID, dialog));
    focus
}

/// Carries out a screen reader's request in the log formats editor.
fn act_on_formats(workspace: &mut Workspace, request: &ActionRequest, commands: &mut Commands) {
    let Some(panel) = workspace.formats_panel.as_mut() else {
        return;
    };
    let target = request.target_node;
    let click = request.action == Action::Click;
    if let Some(index) = in_range(target, FIRST_FORMAT_OPTION_ID, panel.option_chips().len()) {
        panel.focus = FormatsPart::Options;
        if click {
            let response = panel.choose_option(index);
            workspace.formats_responded(&response, commands);
        }
    } else if let Some(index) = in_range(target, FIRST_FORMAT_ITEM_ID, panel.formats.len() + 1) {
        panel.focus = FormatsPart::List;
        let changed = panel.select(index);
        let response = FormatsResponse {
            changed,
            ..FormatsResponse::default()
        };
        workspace.formats_responded(&response, commands);
    } else if target == FORMATS_FIELD_ID {
        panel.focus = FormatsPart::Field;
    }
}

/// The panel waiting for a shortcut, as a modal dialog that has focus itself,
/// since it listens for keys rather than offering a box to type in.
fn shortcut_nodes(panel: &ShortcutPanel, scale: f32, nodes: &mut Vec<(NodeId, Node)>) -> NodeId {
    let mut children = Vec::new();
    for (index, chip) in panel.button_chips().iter().enumerate() {
        let Some(rect) = panel.buttons.chip_rect(index) else {
            continue;
        };
        let id = NodeId(FIRST_SHORTCUT_BUTTON_ID + index as u64);
        let mut node = Node::new(Role::Button);
        node.set_label(chip.label.as_str());
        node.add_action(Action::Click);
        node.set_bounds(physical(rect, scale));
        children.push(id);
        nodes.push((id, node));
    }
    let mut dialog = Node::new(Role::Dialog);
    dialog.set_label(panel.title());
    dialog.set_description(panel.prompt());
    dialog.set_modal();
    dialog.set_bounds(physical(panel.rect(), scale));
    dialog.set_children(children);
    dialog.add_action(Action::Focus);
    nodes.push((SHORTCUT_ID, dialog));
    SHORTCUT_ID
}

/// The highlight rules editor, as a modal dialog. Returns the node with focus.
fn rules_nodes(panel: &RulesPanel, scale: f32, nodes: &mut Vec<(NodeId, Node)>) -> NodeId {
    let mut children = vec![RULES_FIELD_ID];
    let mut field = Node::new(Role::TextInput);
    field.set_label("Rule pattern");
    field.set_value(panel.field.text());
    field.set_bounds(physical(panel.field_rect(), scale));
    field.add_action(Action::Focus);
    nodes.push((RULES_FIELD_ID, field));
    let mut focus = RULES_FIELD_ID;

    // Colors and the two styles are each a choice of one, and the rest are buttons.
    let bars = [
        (
            panel.color_chips(),
            &panel.colors,
            FIRST_RULE_COLOR_ID,
            Part::Colors,
        ),
        (
            panel.option_chips(),
            &panel.options,
            FIRST_RULE_OPTION_ID,
            Part::Options,
        ),
    ];
    for (chips, bar, first, part) in bars {
        for (index, chip) in chips.iter().enumerate() {
            let Some(rect) = bar.chip_rect(index) else {
                continue;
            };
            let id = NodeId(first + index as u64);
            let choice = part == Part::Colors || index < 2;
            let mut node = Node::new(if choice {
                Role::RadioButton
            } else {
                Role::Button
            });
            node.set_label(chip.label.as_str());
            if choice || RulesPanel::option_is_toggle(index) {
                node.set_toggled(chip.active.into());
            }
            if part == Part::Options && !panel.option_applies(index) {
                node.set_disabled();
            } else {
                node.add_action(Action::Click);
            }
            node.add_action(Action::Focus);
            node.set_bounds(physical(rect, scale));
            if panel.focus == part && bar.focus() == Some(index) {
                focus = id;
            }
            children.push(id);
            nodes.push((id, node));
        }
    }

    let list = &panel.list;
    let mut options = Vec::new();
    for index in list.visible_items() {
        let (Some(rect), ListItem::Entry { label, detail, .. }) =
            (list.item_rect(index), &list.items[index])
        else {
            continue;
        };
        let id = NodeId(FIRST_RULE_ID + index as u64);
        let mut node = Node::new(Role::ListBoxOption);
        node.set_label(format!("{label}, {detail}"));
        node.set_selected(panel.selected() == index);
        node.set_bounds(physical(rect, scale));
        node.add_action(Action::Focus);
        node.add_action(Action::Click);
        if panel.focus == Part::List && panel.selected() == index {
            focus = id;
        }
        options.push(id);
        nodes.push((id, node));
    }
    let mut list_node = Node::new(Role::ListBox);
    list_node.set_label("Rules, first match first");
    list_node.set_children(options);
    nodes.push((RULES_LIST_ID, list_node));
    children.push(RULES_LIST_ID);

    if let Some(problem) = panel.problem() {
        let mut note = Node::new(Role::Label);
        note.set_value(problem);
        nodes.push((RULES_NOTE_ID, note));
        children.push(RULES_NOTE_ID);
    }
    let mut dialog = Node::new(Role::Dialog);
    dialog.set_label(rules_panel::TITLE);
    dialog.set_modal();
    dialog.set_bounds(physical(panel.rect(), scale));
    dialog.set_children(children);
    nodes.push((RULES_ID, dialog));
    focus
}

/// Carries out a screen reader's request in the rules editor. Clicking a
/// color or option presses it, and focusing a rule selects it.
fn act_on_rules(workspace: &mut Workspace, request: &ActionRequest) {
    let Some(panel) = workspace.rules_panel.as_mut() else {
        return;
    };
    let target = request.target_node;
    let click = request.action == Action::Click;
    let changed = if let Some(index) =
        in_range(target, FIRST_RULE_COLOR_ID, panel.color_chips().len())
    {
        panel.focus = Part::Colors;
        click && panel.choose_color(index)
    } else if let Some(index) = in_range(target, FIRST_RULE_OPTION_ID, panel.option_chips().len()) {
        panel.focus = Part::Options;
        click && panel.option_applies(index) && panel.choose_option(index)
    } else if let Some(index) = in_range(target, FIRST_RULE_ID, panel.rules.len() + 1) {
        panel.focus = Part::List;
        panel.select(index)
    } else {
        if target == RULES_FIELD_ID {
            panel.focus = Part::Field;
        }
        false
    };
    if changed {
        workspace.rules_edited();
    }
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
            node.set_label(match chip.count {
                Some(count) => format!("{}, {count}", chip.label),
                None => chip.label.clone(),
            });
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

    if let Some(palette) = &workspace.palette {
        let mut field = Node::new(Role::TextInput);
        field.set_label(palette.placeholder());
        field.set_value(palette.palette.query());
        field.set_bounds(physical(palette.palette.field_rect(), scale));
        nodes.push((PALETTE_FIELD_ID, field));
        let mut options = Vec::new();
        for index in palette.palette.visible_matches() {
            let (Some(item), Some(rect)) = (
                palette.palette.matched_item(index),
                palette.palette.match_rect(index),
            ) else {
                continue;
            };
            let id = NodeId(FIRST_PALETTE_ID + index as u64);
            let mut node = Node::new(Role::ListBoxOption);
            let mut label = item.label.clone();
            for part in [&item.detail, &item.shortcut] {
                if !part.is_empty() {
                    label.push_str(", ");
                    label.push_str(part);
                }
            }
            node.set_label(label);
            node.set_selected(palette.palette.selected() == Some(index));
            node.set_bounds(physical(rect, scale));
            node.add_action(Action::Click);
            options.push(id);
            nodes.push((id, node));
        }
        let mut list = Node::new(Role::ListBox);
        list.set_label("Matches");
        list.set_children(options);
        nodes.push((PALETTE_LIST_ID, list));
        let mut dialog = Node::new(Role::Dialog);
        dialog.set_label(palette.title());
        dialog.set_modal();
        dialog.set_bounds(physical(palette.palette.rect(), scale));
        dialog.set_children(vec![PALETTE_FIELD_ID, PALETTE_LIST_ID]);
        nodes.push((PALETTE_ID, dialog));
        window_children.push(PALETTE_ID);
    }

    let rules_focus = workspace
        .rules_panel
        .as_ref()
        .map(|panel| rules_nodes(panel, scale, &mut nodes));
    if rules_focus.is_some() {
        window_children.push(RULES_ID);
    }
    let columns_focus = workspace
        .columns_panel
        .as_ref()
        .map(|panel| columns_nodes(panel, scale, &mut nodes));
    if columns_focus.is_some() {
        window_children.push(COLUMNS_ID);
    }
    let fonts_focus = workspace
        .fonts_panel
        .as_ref()
        .map(|panel| fonts_nodes(panel, scale, &mut nodes));
    if fonts_focus.is_some() {
        window_children.push(FONTS_ID);
    }
    let shortcut_focus = workspace
        .shortcut_panel
        .as_ref()
        .map(|panel| shortcut_nodes(panel, scale, &mut nodes));
    if shortcut_focus.is_some() {
        window_children.push(SHORTCUT_ID);
    }
    let formats_focus = workspace
        .formats_panel
        .as_ref()
        .map(|panel| formats_nodes(panel, scale, &mut nodes));
    if formats_focus.is_some() {
        window_children.push(FORMATS_ID);
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
    let focus = menu_focus
        .or(rules_focus)
        .or(columns_focus)
        .or(fonts_focus)
        .or(shortcut_focus)
        .or(formats_focus)
        .unwrap_or(match workspace.focus {
            _ if workspace.palette.is_some() => PALETTE_FIELD_ID,
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
            if workspace.shortcut_panel.is_some() {
                let buttons = shortcut_panel::BUTTONS.len();
                if let Some(index) = in_range(target, FIRST_SHORTCUT_BUTTON_ID, buttons) {
                    if request.action == Action::Click {
                        workspace.press_shortcut_button(index, commands);
                    }
                }
            } else if workspace.formats_panel.is_some() {
                act_on_formats(workspace, request, commands);
            } else if workspace.rules_panel.is_some() {
                act_on_rules(workspace, request);
            } else if workspace.columns_panel.is_some() {
                act_on_columns(workspace, request);
            } else if workspace.fonts_panel.is_some() {
                act_on_fonts(workspace, request, commands);
            } else if let Some(row) = row_from_id(target) {
                if let Some(tab) = workspace.active_tab_mut() {
                    tab.view.select(row, false);
                    tab.following = tab.view.at_bottom();
                }
            } else if let Some(index) = workspace.palette.as_ref().and_then(|palette| {
                in_range(target, FIRST_PALETTE_ID, palette.palette.match_count())
            }) {
                workspace.choose_palette_match(index, commands);
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
    fn the_rules_editor_is_a_modal_dialog_a_screen_reader_can_drive() {
        let (_dir, mut workspace) = opened();
        let mut commands = Commands::default();
        workspace.run_command(super::super::workspace::command::RULES, &mut commands);
        for character in "disk full 1".chars() {
            workspace.event(
                Event::Char(character),
                &mut RecordingCanvas::new(),
                &mut commands,
            );
        }
        let draw = |workspace: &mut Workspace| {
            let mut commands = Commands::default();
            workspace.woken(&mut commands);
            workspace.draw(
                &mut RecordingCanvas::new(),
                (1000.0, 600.0),
                1.0,
                &mut commands,
            );
        };
        draw(&mut workspace);
        let update = build(&workspace, 1.0);
        assert_eq!(node(&update, RULES_ID).role(), Role::Dialog);
        assert!(node(&update, RULES_ID).is_modal());
        assert_eq!(update.focus, RULES_FIELD_ID);
        assert_eq!(node(&update, RULES_FIELD_ID).value(), Some("disk full 1"));
        let yellow = NodeId(FIRST_RULE_COLOR_ID + 2);
        assert_eq!(node(&update, yellow).role(), Role::RadioButton);
        assert_eq!(
            node(&update, yellow).toggled(),
            Some(accesskit::Toggled::True)
        );
        assert!(
            node(&update, NodeId(FIRST_RULE_OPTION_ID + 3)).is_disabled(),
            "nothing above to move past"
        );

        // Clicking Blue recolors the rule.
        let blue = NodeId(FIRST_RULE_COLOR_ID + 4);
        act(
            &mut workspace,
            &ActionRequest {
                action: Action::Click,
                target_tree: TreeId::ROOT,
                target_node: blue,
                data: None,
            },
            &mut commands,
        );
        assert_eq!(
            workspace.rules_panel.as_ref().unwrap().rules[0].color,
            "blue"
        );
        assert_eq!(workspace.rules_panel.as_ref().unwrap().focus, Part::Colors);

        // Row 2 matches, and says so.
        let started = Instant::now();
        loop {
            draw(&mut workspace);
            let update = build(&workspace, 1.0);
            let label = node(&update, row_id(1))
                .label()
                .unwrap_or_default()
                .to_string();
            if label.contains("highlighted blue") {
                assert!(
                    label.starts_with("Row 2, Error, highlighted blue, "),
                    "{label}"
                );
                break;
            }
            assert!(started.elapsed() < Duration::from_secs(10), "{label}");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn the_columns_editor_is_a_modal_dialog_and_rows_follow_the_columns() {
        let (_dir, mut workspace) = opened();
        let mut commands = Commands::default();
        workspace.run_command(super::super::workspace::command::COLUMNS, &mut commands);
        let draw = |workspace: &mut Workspace| {
            let mut commands = Commands::default();
            workspace.woken(&mut commands);
            workspace.draw(
                &mut RecordingCanvas::new(),
                (1000.0, 600.0),
                1.0,
                &mut commands,
            );
        };
        draw(&mut workspace);
        let update = build(&workspace, 1.0);
        assert_eq!(node(&update, COLUMNS_ID).role(), Role::Dialog);
        assert_eq!(
            node(&update, COLUMNS_ID).label(),
            Some("Columns for other logs")
        );
        let time = NodeId(FIRST_COLUMN_ITEM_ID);
        assert_eq!(update.focus, time);
        assert_eq!(
            node(&update, time).label(),
            Some("Time, shown, 23 characters wide")
        );
        assert!(node(&update, NodeId(FIRST_COLUMN_BUTTON_ID + 1)).is_disabled());

        // Choosing the component and pressing Shown hides it.
        let click = |workspace: &mut Workspace, target: NodeId| {
            act(
                workspace,
                &ActionRequest {
                    action: Action::Click,
                    target_tree: TreeId::ROOT,
                    target_node: target,
                    data: None,
                },
                &mut Commands::default(),
            );
        };
        click(&mut workspace, NodeId(FIRST_COLUMN_ITEM_ID + 2));
        click(&mut workspace, NodeId(FIRST_COLUMN_BUTTON_ID));
        workspace.event(
            Event::KeyDown(Key::Escape, Modifiers::default()),
            &mut RecordingCanvas::new(),
            &mut commands,
        );
        draw(&mut workspace);
        let update = build(&workspace, 1.0);
        assert_eq!(
            node(&update, row_id(1)).label(),
            Some("Row 2, Error, 2026-01-01 10:00:00.000, disk full 1")
        );
    }

    #[test]
    fn the_shortcut_panel_is_a_dialog_that_says_what_it_wants() {
        let (_dir, mut workspace) = opened();
        let mut commands = Commands::default();
        workspace.run_command(super::super::workspace::command::KEYS, &mut commands);
        for character in "toggle bookmark".chars() {
            workspace.event(
                Event::Char(character),
                &mut RecordingCanvas::new(),
                &mut commands,
            );
        }
        workspace.event(
            Event::KeyDown(Key::Enter, Modifiers::default()),
            &mut RecordingCanvas::new(),
            &mut commands,
        );
        workspace.draw(
            &mut RecordingCanvas::new(),
            (1000.0, 600.0),
            1.0,
            &mut commands,
        );
        let update = build(&workspace, 1.0);
        assert_eq!(update.focus, SHORTCUT_ID);
        let dialog = node(&update, SHORTCUT_ID);
        assert_eq!(dialog.label(), Some("Shortcut for Toggle bookmark"));
        assert_eq!(
            dialog.description(),
            Some("Press the new keys. Now Ctrl+D, and by default Ctrl+D.")
        );
        // The Remove button takes the shortcut away.
        act(
            &mut workspace,
            &ActionRequest {
                action: Action::Click,
                target_tree: TreeId::ROOT,
                target_node: NodeId(FIRST_SHORTCUT_BUTTON_ID),
                data: None,
            },
            &mut commands,
        );
        assert!(workspace.shortcut_panel.is_none());
        assert!(workspace
            .status()
            .contains("Toggle bookmark has no shortcut now"));
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
