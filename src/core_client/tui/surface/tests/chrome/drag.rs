use super::*;
use crate::session_doc::{Arrangement, Place};

fn press(surface: &mut Surface, column: u16, row: u16) -> Option<Operation> {
    point(
        surface,
        MouseEventKind::Down(MouseButton::Left),
        column,
        row,
    )
}

fn drag(surface: &mut Surface, column: u16, row: u16) -> Option<Operation> {
    point(
        surface,
        MouseEventKind::Drag(MouseButton::Left),
        column,
        row,
    )
}

fn release(surface: &mut Surface, column: u16, row: u16) -> Option<Operation> {
    point(surface, MouseEventKind::Up(MouseButton::Left), column, row)
}

fn marks(surface: &mut Surface, y: u16, glyph: &str) -> Vec<u16> {
    frame(surface, &HashSet::new());
    let accent = styled(Style::default().fg(Palette::catppuccin().accent)).fg;
    (0..surface.canvas_cols)
        .filter(|x| cell(surface, *x, y).symbol == glyph && cell(surface, *x, y).fg == accent)
        .collect()
}

fn tabs() -> Surface {
    let mut surface = plain(120, 30);
    surface.update(spaces(&[(None, &[None, None, None])]));
    frame(&mut surface, &HashSet::new());
    surface
}

fn moved(arrangement: Arrangement) -> Option<Operation> {
    Some(Operation::Arrange(arrangement))
}

#[tokio::test]
async fn dragging_a_tab_marks_where_it_will_land_and_moves_it_there_on_release() {
    let mut surface = tabs();
    let (first, third, plus) = (
        find(&surface, 0, " 1 ") + 1,
        find(&surface, 0, " 3 ") + 1,
        find(&surface, 0, " + ") + 1,
    );

    assert_eq!(press(&mut surface, first, 0), None);
    assert_eq!(drag(&mut surface, first, 0), None);
    assert_eq!(marks(&mut surface, 0, "│"), Vec::<u16>::new());

    assert_eq!(drag(&mut surface, plus, 0), None);
    let marked = marks(&mut surface, 0, "│");
    assert!(marked.len() == 1 && marked[0] > third, "{marked:?}");

    assert_eq!(
        release(&mut surface, plus, 0),
        moved(Arrangement::MoveTab {
            tab: "tab1".to_owned(),
            to: Place::End,
        })
    );
    assert_eq!(tab(&surface), "tab1");
    assert_eq!(marks(&mut surface, 0, "│"), Vec::<u16>::new());
}

#[tokio::test]
async fn a_tab_dragged_to_the_first_slot_lands_before_the_first_tab() {
    let mut surface = tabs();
    let (first, third) = (find(&surface, 0, " 1 "), find(&surface, 0, " 3 ") + 1);

    press(&mut surface, third, 0);
    drag(&mut surface, first, 0);

    let marked = marks(&mut surface, 0, "│");
    assert!(marked.len() == 1 && marked[0] <= first, "{marked:?}");
    assert_eq!(
        release(&mut surface, first, 0),
        moved(Arrangement::MoveTab {
            tab: "tab3".to_owned(),
            to: Place::Before("tab1".to_owned()),
        })
    );
}

#[tokio::test]
async fn a_tab_dropped_on_the_left_half_of_another_lands_before_it_and_on_the_right_half_after_it()
{
    let mut surface = tabs();
    let (first, second, third) = (
        find(&surface, 0, " 1 ") + 1,
        find(&surface, 0, " 2 "),
        find(&surface, 0, " 3 "),
    );
    let before = |tab: &str| Place::Before(tab.to_owned());
    for (x, to) in [
        (second + 2, before("tab3")),
        (third, before("tab3")),
        (third + 2, Place::End),
    ] {
        press(&mut surface, first, 0);
        drag(&mut surface, x, 0);
        assert_eq!(
            release(&mut surface, x, 0),
            moved(Arrangement::MoveTab {
                tab: "tab1".to_owned(),
                to,
            }),
            "{x}"
        );
    }
    press(&mut surface, first, 0);
    drag(&mut surface, second, 0);
    assert_eq!(release(&mut surface, second, 0), None);
}

#[tokio::test]
async fn a_tab_released_off_the_tab_row_moves_nothing_and_takes_no_focus() {
    let mut surface = tabs();
    let (second, third) = (find(&surface, 0, " 2 ") + 1, find(&surface, 0, " 3 ") + 1);

    press(&mut surface, second, 0);
    drag(&mut surface, third, 0);
    drag(&mut surface, third, 5);
    assert_eq!(marks(&mut surface, 0, "│"), Vec::<u16>::new());

    assert_eq!(release(&mut surface, second, 5), None);
    assert_eq!(tab(&surface), "tab1");
}

#[tokio::test]
async fn a_tab_dragged_back_onto_itself_still_takes_no_focus() {
    let mut surface = tabs();
    let second = find(&surface, 0, " 2 ") + 1;

    press(&mut surface, second, 0);
    drag(&mut surface, second + 1, 0);
    release(&mut surface, second, 0);

    assert_eq!(tab(&surface), "tab1");
}

#[tokio::test]
async fn a_tab_drag_redraws_the_chrome_only_when_its_landing_place_changes() {
    let mut surface = tabs();
    let (first, plus) = (find(&surface, 0, " 1 ") + 1, find(&surface, 0, " + ") + 1);

    press(&mut surface, first, 0);
    drag(&mut surface, plus, 0);
    assert!(surface.needs_frame());
    frame(&mut surface, &HashSet::new());

    drag(&mut surface, plus + 1, 0);
    assert!(!surface.needs_frame());

    release(&mut surface, plus + 1, 0);
    assert!(surface.needs_frame());
}

fn workspaces() -> Surface {
    let mut surface = plain(120, 30);
    surface.update(many(3));
    frame(&mut surface, &HashSet::new());
    surface
}

#[tokio::test]
async fn dragging_a_workspace_marks_its_row_and_its_drop_line_and_moves_it_there_on_release() {
    let mut surface = workspaces();
    let dragged = background(Palette::catppuccin().surface1);

    press(&mut surface, 5, 2);
    drag(&mut surface, 5, 2);
    assert_eq!(marks(&mut surface, 5, "─"), Vec::<u16>::new());
    assert_ne!(cell(&surface, 1, 2).bg, dragged);

    drag(&mut surface, 5, 6);
    assert_eq!(marks(&mut surface, 5, "─"), (0..25).collect::<Vec<_>>());
    assert_eq!(cell(&surface, 1, 2).bg, dragged);

    assert_eq!(
        release(&mut surface, 5, 6),
        moved(Arrangement::MoveWorkspace {
            workspace: "w0".to_owned(),
            to: Place::End,
        })
    );
    assert_eq!(workspace(&surface), "w0");
    assert_eq!(marks(&mut surface, 5, "─"), Vec::<u16>::new());
    assert_ne!(cell(&surface, 1, 2).bg, dragged);
}

#[tokio::test]
async fn a_workspace_lands_at_the_nearest_gap_and_one_dropped_where_it_is_moves_nothing() {
    let mut surface = workspaces();
    for (row, expected) in [
        (
            1,
            moved(Arrangement::MoveWorkspace {
                workspace: "w1".to_owned(),
                to: Place::Before("w0".to_owned()),
            }),
        ),
        (2, None),
        (3, None),
    ] {
        press(&mut surface, 5, 3);
        drag(&mut surface, 6, row);
        assert_eq!(release(&mut surface, 6, row), expected, "{row}");
        assert_eq!(workspace(&surface), "w0", "{row}");
    }
}

#[tokio::test]
async fn a_workspace_dragged_off_the_list_shows_no_line_and_moves_nothing() {
    let mut surface = workspaces();

    press(&mut surface, 5, 3);
    drag(&mut surface, 5, 1);
    assert_eq!(marks(&mut surface, 1, "─"), (0..25).collect::<Vec<_>>());
    drag(&mut surface, 5, 0);
    assert_eq!(marks(&mut surface, 0, "─"), Vec::<u16>::new());
    assert_eq!(release(&mut surface, 5, 0), None);

    press(&mut surface, 5, 3);
    drag(&mut surface, 5, 28);
    assert_eq!(marks(&mut surface, 28, "─"), Vec::<u16>::new());
    assert_eq!(release(&mut surface, 5, 28), None);
}

#[tokio::test]
async fn a_new_press_clears_a_drag_whose_release_was_lost() {
    let mut surface = tabs();
    let (first, plus) = (find(&surface, 0, " 1 ") + 1, find(&surface, 0, " + ") + 1);

    press(&mut surface, first, 0);
    drag(&mut surface, plus, 0);
    assert_eq!(marks(&mut surface, 0, "│").len(), 1);

    press(&mut surface, 60, 10);
    assert_eq!(marks(&mut surface, 0, "│"), Vec::<u16>::new());
}

#[tokio::test]
async fn a_tab_dropped_on_a_scroll_button_lands_at_that_end_of_every_tab() {
    let mut surface = plain(90, 10);
    surface.update(spaces(&[(None, &[None; 20])]));
    frame(&mut surface, &HashSet::new());
    let right = find(&surface, 0, " > ") + 1;
    tap(&mut surface, right, 0);
    frame(&mut surface, &HashSet::new());
    let third = find(&surface, 0, " 3 ") + 1;

    for (x, to) in [(27, Place::Before("tab1".to_owned())), (right, Place::End)] {
        press(&mut surface, third, 0);
        drag(&mut surface, x, 0);
        assert_eq!(
            release(&mut surface, x, 0),
            moved(Arrangement::MoveTab {
                tab: "tab3".to_owned(),
                to,
            }),
            "{x}"
        );
    }
}

#[tokio::test]
async fn a_tab_held_over_a_one_cell_first_tab_marks_the_slot_before_it() {
    let mut surface = dressed(30, 30, |config| config.ui.mobile_width_threshold = 0);
    surface.update(spaces(&[(None, &[None, None])]));
    frame(&mut surface, &HashSet::new());
    let first = find(&surface, 0, "1");

    press(&mut surface, first, 0);
    drag(&mut surface, first - 1, 0);
    drag(&mut surface, first, 0);

    assert_eq!(marks(&mut surface, 0, "│"), [first]);
}

#[tokio::test]
async fn a_tab_drag_ends_when_the_host_loses_focus() {
    let mut surface = tabs();
    let (first, plus) = (find(&surface, 0, " 1 ") + 1, find(&surface, 0, " + ") + 1);
    press(&mut surface, first, 0);
    drag(&mut surface, plus, 0);

    surface.host_focus(false);
    drag(&mut surface, plus - 1, 0);

    assert_eq!(marks(&mut surface, 0, "│"), Vec::<u16>::new());
    assert_eq!(release(&mut surface, plus, 0), None);
}

#[tokio::test]
async fn a_tab_drag_ends_when_a_dialog_opens() {
    let mut surface = tabs();
    let (first, plus) = (find(&surface, 0, " 1 ") + 1, find(&surface, 0, " + ") + 1);
    press(&mut surface, first, 0);
    drag(&mut surface, plus, 0);

    surface.act(KeybindAction::Help, false);
    surface.dialog_key(&TerminalKey::new(KeyCode::Esc, KeyModifiers::NONE));

    assert_eq!(release(&mut surface, plus, 0), None);
}

#[tokio::test]
async fn a_tab_dragged_under_the_resize_bar_has_nowhere_to_land() {
    let mut surface = dressed(120, 30, |config| {
        config.ui.tab_bar_position = TabBarPositionConfig::Bottom;
    });
    surface.update(spaces(&[(None, &[None, None, None])]));
    frame(&mut surface, &HashSet::new());
    let (first, plus) = (find(&surface, 29, " 1 ") + 1, find(&surface, 29, " + ") + 1);
    press(&mut surface, first, 29);

    surface.act(KeybindAction::EnterResizeMode, false);
    frame(&mut surface, &HashSet::new());
    drag(&mut surface, plus, 29);

    assert_eq!(release(&mut surface, plus, 29), None);
}

#[tokio::test]
async fn a_tab_dragged_under_the_navigate_bar_has_nowhere_to_land() {
    let mut surface = dressed(120, 30, |config| {
        config.ui.tab_bar_position = TabBarPositionConfig::Bottom;
    });
    surface.update(spaces(&[(None, &[None, None, None])]));
    frame(&mut surface, &HashSet::new());
    let (first, plus) = (find(&surface, 29, " 1 ") + 1, find(&surface, 29, " + ") + 1);
    press(&mut surface, first, 29);

    surface.act(KeybindAction::WorkspacePicker, false);
    frame(&mut surface, &HashSet::new());
    drag(&mut surface, plus, 29);

    assert_eq!(release(&mut surface, plus, 29), None);
}

#[tokio::test]
async fn a_tab_drag_has_nowhere_to_land_while_the_navigate_bar_covers_the_tab_strip() {
    let mut surface = dressed(120, 30, |config| {
        config.ui.tab_bar_position = TabBarPositionConfig::Bottom;
    });
    surface.update(spaces(&[(None, &[None, None, None])]));
    frame(&mut surface, &HashSet::new());
    let third = find(&surface, 29, " 3 ") + 1;
    press(&mut surface, third, 29);

    surface.act(KeybindAction::WorkspacePicker, false);
    frame(&mut surface, &HashSet::new());
    drag(&mut surface, 5, 29);

    assert_eq!(release(&mut surface, 5, 29), None);
}

#[tokio::test]
async fn a_workspace_released_on_the_navigate_bar_moves_nothing_even_with_a_slot_marked() {
    let mut surface = workspaces();
    press(&mut surface, 5, 2);
    drag(&mut surface, 5, 6);

    surface.act(KeybindAction::WorkspacePicker, false);
    frame(&mut surface, &HashSet::new());

    assert_eq!(release(&mut surface, 40, 29), None);
}

#[tokio::test]
async fn a_workspace_drag_ends_when_a_redraw_moves_the_rows_under_it() {
    let mut surface = workspaces();
    press(&mut surface, 5, 2);
    drag(&mut surface, 5, 6);
    frame(&mut surface, &HashSet::new());

    surface.act(KeybindAction::ToggleSidebar, false);
    frame(&mut surface, &HashSet::new());

    assert_eq!(release(&mut surface, 5, 6), None);
}

#[tokio::test]
async fn a_workspace_drag_ends_when_its_rows_are_reordered_under_it() {
    let mut surface = workspaces();
    press(&mut surface, 5, 4);
    drag(&mut surface, 5, 2);
    frame(&mut surface, &HashSet::new());

    let mut reordered = many(3);
    reordered.workspaces.swap(0, 1);
    surface.update(reordered);
    frame(&mut surface, &HashSet::new());

    assert_eq!(release(&mut surface, 5, 2), None);
}

#[tokio::test]
async fn a_tab_drag_ends_when_its_tabs_are_reordered_under_it() {
    let mut surface = tabs();
    let (first, third) = (find(&surface, 0, " 1 "), find(&surface, 0, " 3 ") + 1);
    press(&mut surface, third, 0);
    drag(&mut surface, first, 0);
    frame(&mut surface, &HashSet::new());

    let mut reordered = spaces(&[(None, &[None, None, None])]);
    reordered.workspaces[0].tabs.swap(0, 1);
    surface.update(reordered);
    frame(&mut surface, &HashSet::new());

    assert_eq!(release(&mut surface, first, 0), None);
}

#[tokio::test]
async fn a_tab_drag_ends_when_a_status_mark_widens_the_tabs_under_it() {
    let mut surface = symbols(120, 30);
    surface.update(spaces(&[(None, &[Some("alphabet"), Some("beta")])]));
    frame(&mut surface, &HashSet::new());
    let (first, second) = (find(&surface, 0, "alphabet"), find(&surface, 0, "beta"));
    press(&mut surface, second, 0);
    drag(&mut surface, first, 0);
    frame(&mut surface, &HashSet::new());

    said(&mut surface, "t1", "working");
    frame(&mut surface, &HashSet::new());

    assert_eq!(release(&mut surface, first, 0), None);
}

#[tokio::test]
async fn a_workspace_drag_survives_a_status_mark_that_widens_a_tab_and_still_drops() {
    let mut surface = symbols(120, 30);
    surface.update(spaces(&[
        (Some("ws0"), &[Some("alphabet")]),
        (Some("ws1"), &[None]),
        (Some("ws2"), &[None]),
    ]));
    frame(&mut surface, &HashSet::new());
    press(&mut surface, 5, 2);
    drag(&mut surface, 5, 6);
    frame(&mut surface, &HashSet::new());

    said(&mut surface, "t1", "working");
    assert!(tab_bar(&mut surface).contains("◐ alphabet"));

    assert_eq!(
        release(&mut surface, 5, 6),
        moved(Arrangement::MoveWorkspace {
            workspace: "w0".to_owned(),
            to: Place::End,
        })
    );
}

#[tokio::test]
async fn a_tab_drag_survives_a_workspace_added_to_the_sidebar_and_still_drops() {
    let mut surface = tabs();
    let (first, third) = (find(&surface, 0, " 1 "), find(&surface, 0, " 3 ") + 1);
    press(&mut surface, third, 0);
    drag(&mut surface, first, 0);
    frame(&mut surface, &HashSet::new());

    surface.update(spaces(&[(None, &[None, None, None]), (None, &[None])]));
    frame(&mut surface, &HashSet::new());

    assert_eq!(
        release(&mut surface, first, 0),
        moved(Arrangement::MoveTab {
            tab: "tab3".to_owned(),
            to: Place::Before("tab1".to_owned()),
        })
    );
}
