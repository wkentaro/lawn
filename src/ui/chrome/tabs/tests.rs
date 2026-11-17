use super::{max_tab_scroll, render_tab_strip, Tab, TabBarOutcome, TabBarState, TabBarStyle};
use crate::ui::theme::Palette;
use ratatui::{buffer::Buffer, layout::Rect};

fn tab(key: &str, interactive: bool) -> Tab<'_> {
    Tab {
        key,
        label: key,
        focused: false,
        custom_label: true,
        zoomed: false,
        interactive,
        status: None,
    }
}

fn draw(tabs: &[Tab<'_>], insert: Option<usize>) -> (Buffer, Vec<(Rect, String)>, TabBarOutcome) {
    let area = Rect::new(0, 0, 60, 1);
    let mut buffer = Buffer::empty(area);
    let palette = Palette::catppuccin();
    let mut hits = Vec::new();
    let outcome = render_tab_strip(
        &mut buffer,
        area,
        tabs,
        TabBarStyle {
            palette: &palette,
            indicators: crate::config::StatusIndicatorStyle::Dots,
            mouse: true,
        },
        TabBarState {
            scroll: 0,
            reveal_focused: true,
        },
        insert,
        |rect, key| hits.push((rect, key.to_owned())),
    );
    (buffer, hits, outcome)
}

#[test]
fn temporary_tabs_are_visible_without_mouse_targets() {
    let (buffer, hits, outcome) = draw(&[tab("a", true), tab("preview", false)], None);
    assert_eq!(
        hits.iter().map(|(_, key)| key.as_str()).collect::<Vec<_>>(),
        ["a"]
    );
    let text = buffer
        .content
        .iter()
        .map(|cell| cell.symbol())
        .collect::<String>();
    assert!(text.contains("preview"));
    assert_eq!(outcome.state, TabBarState::default());
}

#[test]
fn insertion_indices_exclude_temporary_tabs() {
    let (buffer, hits, _) = draw(
        &[
            tab("a", true),
            tab("preview", false),
            tab("b", true),
            tab("c", true),
        ],
        Some(2),
    );
    let before_c = hits
        .iter()
        .find(|(_, key)| key == "c")
        .expect("c target")
        .0
        .x
        - 1;
    assert_eq!(buffer[(before_c, 0)].symbol(), "│");
}

#[test]
fn trailing_scroll_limit_accounts_for_full_widths_and_separators() {
    for (widths, available, expected) in [
        (&[][..], 0, 0),
        (&[8, 13][..], 0, 1),
        (&[8, 13][..], 1, 1),
        (&[8, 13][..], 12, 1),
        (&[8, 13][..], 21, 1),
        (&[8, 13][..], 22, 0),
        (&[8, 13][..], 30, 0),
        (&[8, u16::MAX][..], u16::MAX, 1),
    ] {
        assert_eq!(
            max_tab_scroll(widths, available),
            expected,
            "widths={widths:?}, available={available}"
        );
    }
}
