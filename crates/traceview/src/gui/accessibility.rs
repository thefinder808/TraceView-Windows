//! Describes the window to screen readers. The grid is exposed as a list, because
//! AccessKit's Windows adapter gives table rows no selection support, and a list
//! also announces "item 2 of 26,904,325". The grid is virtual, so only the rows
//! on screen exist as nodes, and each one's text reads as a complete sentence.

use super::log_view::{level_name, LogView, RowSource};
use accesskit::{Action, Node, NodeId, Rect as AccessRect, Role, TreeId, TreeInfo, TreeUpdate};
use tv_ui::Rect;

const WINDOW_ID: NodeId = NodeId(1);
pub const GRID_ID: NodeId = NodeId(2);
const STATUS_ID: NodeId = NodeId(3);
const FIRST_ROW_ID: u64 = 1000;

fn row_id(row: usize) -> NodeId {
    NodeId(FIRST_ROW_ID + row as u64)
}

/// The row a node stands for, if it's a row node.
pub fn row_from_id(id: NodeId) -> Option<usize> {
    id.0.checked_sub(FIRST_ROW_ID).map(|row| row as usize)
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
    let level = level_name(source.level(row));
    match source.cells(row) {
        Some(cells) => {
            let mut label = format!("Row {}, {level}", row + 1);
            for part in [&cells.time, &cells.component, &cells.message] {
                if !part.is_empty() {
                    label.push_str(", ");
                    label.push_str(part);
                }
            }
            label
        }
        None => format!("Row {}, {level}, loading", row + 1),
    }
}

pub fn build(
    view: &LogView,
    source: Option<&dyn RowSource>,
    title: &str,
    status: &str,
    scale: f32,
) -> TreeUpdate {
    let mut nodes = Vec::new();
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

    let mut status_node = Node::new(Role::Status);
    status_node.set_label(status);
    nodes.push((STATUS_ID, status_node));

    let mut window = Node::new(Role::Window);
    window.set_label(title);
    window.set_children(vec![GRID_ID, STATUS_ID]);
    nodes.push((WINDOW_ID, window));

    let visible = view.visible_rows();
    let focus = view
        .cursor()
        .filter(|row| source.is_some() && visible.contains(row))
        .map_or(GRID_ID, row_id);
    TreeUpdate {
        nodes,
        tree: Some(TreeInfo::new(WINDOW_ID)),
        tree_id: TreeId::ROOT,
        focus,
    }
}

#[cfg(test)]
mod tests {
    use super::super::log_view::Metrics;
    use super::super::row_cache::RowCells;
    use super::*;
    use std::sync::Arc;
    use tv_engine::Level;
    use tv_ui::Theme;

    struct Rows;

    impl RowSource for Rows {
        fn row_count(&self) -> usize {
            1000
        }

        fn level(&self, row: usize) -> Level {
            if row == 1 {
                Level::Error
            } else {
                Level::Info
            }
        }

        fn cells(&self, row: usize) -> Option<Arc<RowCells>> {
            (row < 5).then(|| {
                Arc::new(RowCells {
                    time: "2026-01-01 10:00:00.000".to_string(),
                    component: "App".to_string(),
                    thread: String::new(),
                    message: "disk full".to_string(),
                })
            })
        }
    }

    fn view() -> LogView {
        let mut view = LogView::default();
        view.arrange(
            Rect::new(0.0, 0.0, 800.0, 400.0),
            1000,
            Metrics {
                row_height: 18.0,
                char_width: 7.0,
            },
            &Theme::CLASSIC,
            1.0,
        );
        view
    }

    fn node(update: &TreeUpdate, id: NodeId) -> &Node {
        &update
            .nodes
            .iter()
            .find(|(node_id, _)| *node_id == id)
            .unwrap()
            .1
    }

    #[test]
    fn rows_read_as_full_sentences() {
        let update = build(&view(), Some(&Rows), "TraceView", "1000 rows", 1.0);
        assert_eq!(
            node(&update, row_id(1)).label(),
            Some("Row 2, Error, 2026-01-01 10:00:00.000, App, disk full")
        );
    }

    #[test]
    fn rows_still_loading_say_so() {
        let update = build(&view(), Some(&Rows), "TraceView", "", 1.0);
        assert_eq!(
            node(&update, row_id(6)).label(),
            Some("Row 7, Information, loading")
        );
    }

    #[test]
    fn only_visible_rows_become_nodes() {
        let update = build(&view(), Some(&Rows), "TraceView", "", 1.0);
        let grid = node(&update, GRID_ID);
        assert_eq!(node(&update, row_id(0)).size_of_set(), Some(1000));
        assert_eq!(grid.children().len(), view().visible_rows().len());
    }

    #[test]
    fn focus_follows_the_selected_row() {
        let mut view = view();
        assert_eq!(build(&view, Some(&Rows), "", "", 1.0).focus, GRID_ID);
        view.select(3, false);
        let update = build(&view, Some(&Rows), "", "", 1.0);
        assert_eq!(update.focus, row_id(3));
        assert_eq!(node(&update, row_id(3)).is_selected(), Some(true));
        assert_eq!(node(&update, row_id(2)).is_selected(), Some(false));
    }

    #[test]
    fn focus_returns_to_the_grid_when_the_selected_row_scrolls_away() {
        let mut view = view();
        view.select(3, false);
        view.scroll_to(500.0);
        assert_eq!(build(&view, Some(&Rows), "", "", 1.0).focus, GRID_ID);
    }

    #[test]
    fn with_no_log_the_tree_is_still_valid() {
        let update = build(&LogView::default(), None, "TraceView", "Open a log", 1.0);
        assert_eq!(update.focus, GRID_ID);
        assert_eq!(node(&update, STATUS_ID).label(), Some("Open a log"));
    }

    #[test]
    fn bounds_use_physical_pixels() {
        let update = build(&view(), Some(&Rows), "", "", 2.0);
        let bounds = node(&update, GRID_ID).bounds().unwrap();
        assert_eq!(bounds.x1, 2.0 * f64::from(view().body_rect().right));
    }

    #[test]
    fn row_ids_map_back_to_rows() {
        assert_eq!(row_from_id(row_id(42)), Some(42));
        assert_eq!(row_from_id(GRID_ID), None);
    }
}
