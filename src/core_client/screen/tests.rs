use super::*;

fn row(y: u16, text: &str) -> PaneSurfacePatchRow {
    PaneSurfacePatchRow {
        x: 0,
        y,
        cells: text
            .chars()
            .map(|ch| {
                let mut cell = ratatui::buffer::Cell::default();
                cell.set_char(ch);
                CellData::from_ratatui_cell(&cell)
            })
            .collect(),
    }
}

#[test]
fn a_new_size_waits_for_rows_to_redraw_with() {
    let mut screen = Screen::default();
    let mut out = Vec::new();

    screen
        .present(&mut out, (4, 2), Vec::new(), None)
        .expect("present");
    assert!(out.is_empty());

    screen
        .present(&mut out, (4, 2), vec![row(0, "abcd"), row(1, "efgh")], None)
        .expect("present");
    let written = String::from_utf8_lossy(&out);
    assert!(
        written.contains("abcd") && written.contains("efgh"),
        "{written:?}"
    );
}

#[test]
fn rows_past_the_screen_are_clipped() {
    let mut screen = Screen::default();
    let mut out = Vec::new();

    screen
        .present(
            &mut out,
            (3, 1),
            vec![row(0, "abcdef"), row(1, "ghi")],
            None,
        )
        .expect("present");

    let written = String::from_utf8_lossy(&out);
    assert!(written.contains("abc"), "{written:?}");
    assert!(
        !written.contains('d') && !written.contains('g'),
        "{written:?}"
    );
}
