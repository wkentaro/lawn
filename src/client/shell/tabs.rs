use super::*;
use crate::ui::chrome;

pub(crate) fn render_tab_bar(
    buffer: &mut Buffer,
    area: Rect,
    snapshot: &ClientShellSnapshot,
    config: &ClientShellConfig,
    parking: Option<&lawn::PaneParking>,
    online: bool,
    tab_scroll: &mut usize,
    reveal_focused_tab: &mut bool,
    tab_drag_insert_index: Option<usize>,
    hits: &mut ShellHitMap,
) {
    let palette = &config.palette;
    buffer.set_style(area, Style::default().bg(palette.panel_bg));
    let preview = parking.and_then(|parking| parking.restore_preview(snapshot));
    let temporary_tab = preview
        .as_ref()
        .filter(|preview| preview.temporary_tab)
        .map(|preview| ClientShellTab {
            tab_id: preview.target.tab_id.clone(),
            workspace_id: preview.pane.workspace_id.clone(),
            number: 0,
            label: format!("({})", preview.target.tab_label),
            custom_label: false,
            focused: true,
            zoomed: false,
            agent_status: crate::api::schema::AgentStatus::Unknown,
        });
    let statuses = if online {
        tab_statuses(snapshot, parking, preview.as_ref())
    } else {
        HashMap::new()
    };
    let tabs = snapshot
        .tabs
        .iter()
        .filter(|tab| Some(tab.workspace_id.as_str()) == snapshot.focused_workspace_id.as_deref())
        .chain(temporary_tab.as_ref())
        .map(|tab| chrome::Tab {
            key: &tab.tab_id,
            label: &tab.label,
            custom_label: tab.custom_label,
            focused: tab.focused,
            zoomed: tab.zoomed,
            interactive: temporary_tab
                .as_ref()
                .is_none_or(|temporary| temporary.tab_id != tab.tab_id),
            status: statuses.get(tab.tab_id.as_str()).copied(),
        })
        .collect::<Vec<_>>();
    let content = chrome::tab_bar_content_area(area, tab_bar_status_width(snapshot));
    let outcome = chrome::render_tab_strip(
        buffer,
        content,
        &tabs,
        chrome::TabBarStyle {
            palette,
            indicators: config.status_indicators,
            mouse: config.mouse_capture,
        },
        chrome::TabBarState {
            scroll: *tab_scroll,
            reveal_focused: *reveal_focused_tab,
        },
        tab_drag_insert_index,
        |rect, key| hits.tabs.push((rect, key.to_owned())),
    );
    *tab_scroll = outcome.state.scroll;
    *reveal_focused_tab = outcome.state.reveal_focused;
    hits.new_tab = outcome.new_tab;
    hits.tab_scroll_left = outcome.tab_scroll_left;
    hits.tab_scroll_right = outcome.tab_scroll_right;
    chrome::render_tab_bar_status(
        buffer,
        area,
        snapshot
            .tab_bar_right
            .iter()
            .map(|segment| chrome::StatusSegment {
                text: &segment.text,
                accent: segment.accent,
            }),
        &snapshot.tab_bar_right_separator,
        palette,
    );
}

pub(crate) fn tab_bar_status_width(snapshot: &ClientShellSnapshot) -> u16 {
    chrome::tab_bar_status_width(
        snapshot
            .tab_bar_right
            .iter()
            .map(|segment| chrome::StatusSegment {
                text: &segment.text,
                accent: segment.accent,
            }),
        &snapshot.tab_bar_right_separator,
    )
}

fn tab_statuses<'a>(
    snapshot: &'a ClientShellSnapshot,
    parking: Option<&lawn::PaneParking>,
    preview: Option<&lawn::RestorePreview<'a>>,
) -> HashMap<&'a str, (crate::api::schema::AgentStatus, usize)> {
    use crate::api::schema::AgentStatus;
    let panes = snapshot
        .panes
        .iter()
        .filter(|pane| {
            Some(pane.workspace_id.as_str()) == snapshot.focused_workspace_id.as_deref()
                && parking.is_none_or(|parking| !parking.hides_pane(&pane.pane_id))
        })
        .map(|pane| (pane.pane_id.as_str(), pane.tab_id.as_str()))
        .chain(preview.map(|preview| {
            (
                preview.pane.pane_id.as_str(),
                preview.target.tab_id.as_str(),
            )
        }))
        .collect::<HashMap<_, _>>();
    let rank = |status| match status {
        AgentStatus::Blocked => 4,
        AgentStatus::Working => 3,
        AgentStatus::Idle => 2,
        AgentStatus::Done => 1,
        AgentStatus::Unknown => 0,
    };
    let mut statuses = HashMap::new();
    for agent in &snapshot.agents {
        if agent
            .agent
            .as_ref()
            .or(agent.display_agent.as_ref())
            .is_none()
            || rank(agent.agent_status) == 0
        {
            continue;
        }
        let Some(tab) = panes.get(agent.pane_id.as_str()) else {
            continue;
        };
        let entry = statuses.entry(*tab).or_insert((AgentStatus::Unknown, 0));
        if rank(agent.agent_status) > rank(entry.0) {
            entry.0 = agent.agent_status;
        }
        entry.1 += usize::from(agent.agent_status == AgentStatus::Blocked);
    }
    statuses
}
