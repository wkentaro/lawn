use super::*;

fn row(kind: NavigatorRowKind, label: &str) -> NavigatorRow<'_> {
    NavigatorRow {
        kind,
        depth: u8::from(kind == NavigatorRowKind::Tab),
        label,
        meta: "",
        detail: "",
        agent: None,
        status: None,
        stale: false,
        current: false,
        parked: false,
        moving: false,
    }
}

fn tabs() -> Vec<NavigatorRow<'static>> {
    vec![
        row(NavigatorRowKind::Workspace, "main"),
        row(NavigatorRowKind::Tab, "logs"),
        row(NavigatorRowKind::Tab, "build"),
        row(NavigatorRowKind::NewWorkspace, "+ New workspace"),
    ]
}

fn drawn(cols: u16, rows: &[NavigatorRow<'_>]) -> (Vec<String>, NavigatorDialog) {
    let mut buffer = Buffer::empty(Rect::new(0, 0, cols, 30));
    let query = TextEditor::default();
    let dialog = render_navigator(
        &mut buffer,
        &Navigator {
            listing: NavigatorListing::Tabs,
            query: &query,
            parked: false,
            moving: Some(NavigatorMove {
                label: "terminal · 1",
                tab: false,
            }),
            scroll: None,
            selected: Some(1),
            rows,
            background: &|_| 0,
        },
        &Palette::catppuccin(),
    )
    .expect("navigator");
    let lines = (0..30)
        .map(|y| (0..cols).map(|x| buffer[(x, y)].symbol()).collect())
        .collect();
    (lines, dialog)
}

#[test]
fn a_tab_list_has_no_preview_and_is_as_wide_as_the_list_column_of_the_widest_legacy_navigator() {
    for cols in [120, 80] {
        let (lines, dialog) = drawn(cols, &tabs());
        assert!(!lines.iter().any(|line| line.contains("Preview")), "{cols}");
        assert_eq!(dialog.area.width, LIST_WIDTH + 2, "{cols}");
        assert!(dialog.preview.width > 0 && dialog.content_preview.is_empty());
        let edge = dialog.area.right() - 1;
        assert!(dialog
            .rows
            .iter()
            .all(|(rect, _)| rect.x > dialog.area.x && rect.right() <= edge));
        assert!(dialog.close.right() <= edge);
    }
    assert_eq!(drawn(40, &tabs()).1.area.width, 36);
}

#[test]
fn a_tab_list_footer_leaves_out_a_hint_that_would_run_into_the_close_label() {
    let (lines, dialog) = drawn(120, &tabs());
    let footer = &lines[usize::from(dialog.close.y)];
    assert!(footer.contains(" Split right Enter "), "{footer}");
    assert!(!footer.contains("↑↓"), "{footer}");
    assert!(footer.contains("Esc close"), "{footer}");
}

#[test]
fn tab_rows_nest_under_their_workspace_without_an_agent_column_and_count_as_tabs() {
    let (lines, _) = drawn(120, &tabs());
    let text = lines.join("\n");
    let listed = |label: &str| lines.iter().find(|line| line.contains(label)).expect(label);
    for label in ["├─ logs", "└─ build"] {
        let line = listed(label);
        assert!(
            !line.contains("terminal") && !line.contains("shell"),
            "{line}"
        );
    }
    assert!(listed(" 2 tabs│").contains("/"), "{text}");

    let counted = |rows: &[NavigatorRow<'_>], count: &str| {
        drawn(120, rows).0.iter().any(|line| line.contains(count))
    };
    assert!(counted(&[row(NavigatorRowKind::Tab, "logs")], " 1 tab│"));
    let none = [row(NavigatorRowKind::NewWorkspace, "+ New workspace")];
    assert!(counted(&none, " 0 tabs│"));
}
