use super::{display_width, panel_contrast_fg, put_text, status_color, status_icon};
use crate::config::StatusIndicatorStyle;
use crate::ui::theme::Palette;
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
};

const MIN_TAB_WIDTH: u16 = 8;
const NEW_TAB_WIDTH: u16 = 3;
const TAB_SCROLL_BUTTON_WIDTH: u16 = 3;
const MIN_TAB_STRIP_WIDTH: u16 =
    MIN_TAB_WIDTH + NEW_TAB_WIDTH + TAB_SCROLL_BUTTON_WIDTH.saturating_mul(2);

pub(crate) struct Tab<'a> {
    pub(crate) key: &'a str,
    pub(crate) label: &'a str,
    pub(crate) focused: bool,
    pub(crate) custom_label: bool,
    pub(crate) zoomed: bool,
    pub(crate) interactive: bool,
    pub(crate) status: Option<(crate::api::schema::AgentStatus, usize)>,
}

pub(crate) struct TabBarStyle<'a> {
    pub(crate) palette: &'a Palette,
    pub(crate) indicators: StatusIndicatorStyle,
    pub(crate) mouse: bool,
}

#[derive(Default, Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TabBarState {
    pub(crate) scroll: usize,
    pub(crate) reveal_focused: bool,
}

#[derive(Default)]
pub(crate) struct TabBarOutcome {
    pub(crate) state: TabBarState,
    pub(crate) new_tab: Rect,
    pub(crate) tab_scroll_left: Rect,
    pub(crate) tab_scroll_right: Rect,
}

pub(crate) struct StatusSegment<'a> {
    pub(crate) text: &'a str,
    pub(crate) accent: bool,
}

pub(crate) fn render_tab_strip(
    buffer: &mut Buffer,
    area: Rect,
    tabs: &[Tab<'_>],
    chrome: TabBarStyle<'_>,
    mut state: TabBarState,
    tab_drag_insert_index: Option<usize>,
    mut tab_hit: impl FnMut(Rect, &str),
) -> TabBarOutcome {
    let palette = chrome.palette;
    let tab_scroll = &mut state.scroll;
    let reveal_focused_tab = &mut state.reveal_focused;
    let mut hits = TabBarOutcome::default();
    let mut visible_for_drag = tab_drag_insert_index.map(|_| Vec::new());
    let names = tabs
        .iter()
        .map(|tab| {
            let name = tab_label(tab);
            if let Some((status, blocked)) = tab.status {
                let count = if blocked > 1 {
                    blocked.to_string()
                } else {
                    String::new()
                };
                format!("{}{count} {name}", status_icon(status, chrome.indicators))
            } else {
                name
            }
        })
        .collect::<Vec<_>>();
    let desired_widths = names
        .iter()
        .map(|name| display_width(name).saturating_add(4).max(MIN_TAB_WIDTH))
        .collect::<Vec<_>>();
    let content = area;
    let mouse_chrome = chrome.mouse;
    let new_tab_width = if mouse_chrome { NEW_TAB_WIDTH } else { 0 };
    let desired_total = desired_widths
        .iter()
        .copied()
        .fold(0_u16, u16::saturating_add)
        .saturating_add(tabs.len().saturating_sub(1).min(u16::MAX as usize) as u16)
        .saturating_add(new_tab_width);
    let overflow =
        desired_total > content.width && (!mouse_chrome || content.width >= MIN_TAB_STRIP_WIDTH);
    let available = if overflow && mouse_chrome {
        content
            .width
            .saturating_sub(NEW_TAB_WIDTH)
            .saturating_sub(TAB_SCROLL_BUTTON_WIDTH.saturating_mul(2))
    } else {
        content.width.saturating_sub(new_tab_width)
    };
    let max_scroll = max_tab_scroll(&desired_widths, available);
    if !overflow {
        *tab_scroll = 0;
    } else if *reveal_focused_tab {
        if let Some(focused) = tabs.iter().position(|tab| tab.focused) {
            *tab_scroll = centered_tab_scroll(focused, &desired_widths, available).min(max_scroll);
        }
    } else {
        *tab_scroll = (*tab_scroll).min(max_scroll);
    }
    *reveal_focused_tab = false;
    let mut real_tab_index = tab_drag_insert_index
        .map(|_| {
            tabs.iter()
                .take(*tab_scroll)
                .filter(|tab| tab.interactive)
                .count()
        })
        .unwrap_or_default();

    let mut x = content.x;
    let tab_right = if overflow && mouse_chrome {
        hits.tab_scroll_left = Rect::new(
            content.x,
            content.y,
            TAB_SCROLL_BUTTON_WIDTH.min(content.width),
            1,
        );
        put_text(
            buffer,
            hits.tab_scroll_left.x,
            content.y,
            hits.tab_scroll_left.width,
            " < ",
            Style::default()
                .fg(if *tab_scroll > 0 {
                    palette.overlay1
                } else {
                    palette.overlay0
                })
                .bg(palette.surface0),
        );
        x = hits.tab_scroll_left.right();
        content
            .right()
            .saturating_sub(NEW_TAB_WIDTH + TAB_SCROLL_BUTTON_WIDTH)
    } else {
        content.right().saturating_sub(new_tab_width)
    };

    let mut first_visible = None;
    let mut last_visible = None;
    for (index, tab) in tabs.iter().enumerate().skip(*tab_scroll) {
        let name = &names[index];
        let desired = desired_widths[index];
        let remaining = tab_right.saturating_sub(x);
        let width = desired.min(remaining);
        if width == 0 {
            break;
        }
        let rect = Rect::new(x, area.y, width, 1);
        let style = if tab.focused {
            Style::default()
                .fg(palette.text)
                .bg(palette.panel_bg)
                .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
        } else if tab.custom_label {
            Style::default().fg(palette.overlay1).bg(palette.surface0)
        } else {
            Style::default().fg(palette.overlay0).bg(palette.surface0)
        };
        let padding = width.saturating_sub(display_width(name));
        let left = padding / 2;
        let text = format!(
            "{empty:left$}{name}{empty:right_padding$}",
            empty = "",
            left = left as usize,
            right_padding = padding.saturating_sub(left) as usize,
        );
        put_text(buffer, rect.x, rect.y, rect.width, &text, style);
        if let Some((status, _)) = tab.status {
            put_text(
                buffer,
                rect.x.saturating_add(left),
                rect.y,
                rect.width.saturating_sub(left),
                status_icon(status, chrome.indicators),
                style
                    .fg(status_color(status, palette))
                    .remove_modifier(Modifier::UNDERLINED),
            );
        }
        if tab.interactive {
            tab_hit(rect, tab.key);
            if let Some(visible) = &mut visible_for_drag {
                visible.push((real_tab_index, rect));
                real_tab_index += 1;
            }
        }
        first_visible.get_or_insert(index);
        last_visible = Some(index);
        x = x.saturating_add(width + 1);
        if width < desired {
            break;
        }
    }

    if overflow && mouse_chrome {
        hits.tab_scroll_right = Rect::new(tab_right, area.y, TAB_SCROLL_BUTTON_WIDTH, 1);
        let can_scroll_right = *tab_scroll < max_scroll;
        put_text(
            buffer,
            hits.tab_scroll_right.x,
            area.y,
            hits.tab_scroll_right.width,
            " > ",
            Style::default()
                .fg(if can_scroll_right {
                    palette.overlay1
                } else {
                    palette.overlay0
                })
                .bg(palette.surface0),
        );
        hits.new_tab = Rect::new(
            hits.tab_scroll_right.right(),
            area.y,
            content
                .right()
                .saturating_sub(hits.tab_scroll_right.right())
                .min(NEW_TAB_WIDTH),
            1,
        );
    } else if mouse_chrome {
        hits.new_tab = Rect::new(
            x.min(content.right()),
            area.y,
            content.right().saturating_sub(x).min(NEW_TAB_WIDTH),
            1,
        );
    }
    if mouse_chrome {
        put_text(
            buffer,
            hits.new_tab.x,
            area.y,
            hits.new_tab.width,
            " + ",
            Style::default().fg(palette.overlay1).bg(palette.panel_bg),
        );
    }

    if first_visible.is_some_and(|index| index > 0) {
        let ellipsis_x = if hits.tab_scroll_left.width > 0 {
            hits.tab_scroll_left.right()
        } else {
            content.x
        };
        put_text(
            buffer,
            ellipsis_x,
            area.y,
            u16::from(ellipsis_x < content.right()),
            "…",
            Style::default().fg(palette.overlay0),
        );
    }
    if last_visible.is_some_and(|index| index + 1 < tabs.len()) {
        let ellipsis_x = if hits.tab_scroll_right.width > 0 {
            hits.tab_scroll_right.x.saturating_sub(1)
        } else {
            content.right().saturating_sub(1)
        };
        put_text(
            buffer,
            ellipsis_x,
            area.y,
            u16::from(ellipsis_x >= content.x && ellipsis_x < content.right()),
            "…",
            Style::default().fg(palette.overlay0),
        );
    }

    if let Some(insert_index) = tab_drag_insert_index {
        let real_tab_count = tabs.iter().filter(|tab| tab.interactive).count();
        if let Some(indicator_x) = tab_drop_indicator_x(
            visible_for_drag.as_deref().unwrap_or_default(),
            hits.tab_scroll_left,
            hits.tab_scroll_right,
            real_tab_count,
            insert_index,
        ) {
            put_text(
                buffer,
                indicator_x.min(content.right().saturating_sub(1)),
                area.y,
                1,
                "│",
                Style::default().fg(palette.accent),
            );
        }
    }

    hits.state = state;
    hits
}

pub(crate) fn tab_bar_status_width<'a>(
    segments: impl Iterator<Item = StatusSegment<'a>>,
    separator: &str,
) -> u16 {
    let mut count = 0usize;
    let content = segments.fold(0u16, |width, segment| {
        count += 1;
        width.saturating_add(display_width(segment.text))
    });
    content.saturating_add(
        display_width(separator)
            .saturating_mul(count.saturating_sub(1).min(u16::MAX as usize) as u16),
    )
}

fn tab_bar_status_area(area: Rect, width: u16) -> Option<Rect> {
    if width == 0 {
        return None;
    }
    let reserved = width.saturating_add(1);
    (area.width.saturating_sub(reserved) >= MIN_TAB_STRIP_WIDTH)
        .then(|| Rect::new(area.right().saturating_sub(width), area.y, width, 1))
}

pub(crate) fn tab_bar_content_area(area: Rect, status_width: u16) -> Rect {
    let reserved = tab_bar_status_area(area, status_width)
        .map(|status| status.width.saturating_add(1))
        .unwrap_or(0);
    Rect {
        width: area.width.saturating_sub(reserved),
        ..area
    }
}

pub(crate) fn render_tab_bar_status<'a>(
    buffer: &mut Buffer,
    area: Rect,
    segments: impl Iterator<Item = StatusSegment<'a>> + Clone,
    separator: &str,
    palette: &Palette,
) {
    let Some(status) = tab_bar_status_area(area, tab_bar_status_width(segments.clone(), separator))
    else {
        return;
    };
    let separator_width = display_width(separator);
    let mut x = status.x;
    for (index, segment) in segments.enumerate() {
        if index > 0 && separator_width > 0 {
            put_text(
                buffer,
                x,
                area.y,
                separator_width,
                separator,
                Style::default().fg(palette.overlay0).bg(palette.panel_bg),
            );
            x = x.saturating_add(separator_width);
        }
        let width = display_width(segment.text);
        let style = if segment.accent {
            Style::default()
                .fg(panel_contrast_fg(palette))
                .bg(palette.accent)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(palette.overlay1).bg(palette.panel_bg)
        };
        put_text(buffer, x, area.y, width, segment.text, style);
        x = x.saturating_add(width);
    }
}

fn tab_drop_indicator_x(
    visible: &[(usize, Rect)],
    scroll_left: Rect,
    scroll_right: Rect,
    tab_count: usize,
    insert_index: usize,
) -> Option<u16> {
    let (first_index, first_rect) = *visible.first()?;
    let (last_index, last_rect) = *visible.last()?;
    if insert_index == 0 {
        return Some(if first_index == 0 {
            first_rect.x
        } else {
            scroll_left.right()
        });
    }
    if let Some((_, rect)) = visible.iter().find(|(index, _)| *index == insert_index) {
        return Some(rect.x.saturating_sub(1));
    }
    if insert_index >= tab_count {
        return Some(if last_index + 1 >= tab_count {
            last_rect.right()
        } else {
            scroll_right.x.saturating_sub(1)
        });
    }
    None
}

fn centered_tab_scroll(focused: usize, widths: &[u16], available: u16) -> usize {
    let mut best = focused;
    let mut best_distance = u16::MAX;
    for start in 0..=focused {
        let before = widths
            .iter()
            .copied()
            .enumerate()
            .skip(start)
            .take(focused.saturating_sub(start))
            .fold(0u16, |width, (_, tab)| width.saturating_add(tab + 1));
        if before >= available {
            continue;
        }
        let focused_width = widths[focused].min(available.saturating_sub(before));
        let center = before.saturating_mul(2).saturating_add(focused_width);
        let distance = center.abs_diff(available);
        if distance <= best_distance {
            best_distance = distance;
            best = start;
        }
    }
    best
}

fn max_tab_scroll(widths: &[u16], available: u16) -> usize {
    let Some((&last, preceding)) = widths.split_last() else {
        return 0;
    };
    let mut start = preceding.len();
    let mut used = u32::from(last);
    for width in preceding.iter().rev() {
        let required = used + 1 + u32::from(*width);
        if required > u32::from(available) {
            break;
        }
        used = required;
        start -= 1;
    }
    start
}

fn tab_label(tab: &Tab<'_>) -> String {
    if tab.zoomed {
        format!("{} Z", tab.label)
    } else {
        tab.label.to_owned()
    }
}

#[cfg(test)]
mod tests;
