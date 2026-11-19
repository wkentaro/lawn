mod layout;
mod sidebar;
mod style;
mod tabs;

pub(crate) use layout::{Layout, LayoutSettings};
pub(crate) use sidebar::{
    render_agent_row, render_workspace_rows, workspace_active_background,
    workspace_selection_background, AgentRow, WorkspaceRow,
};
pub(crate) use style::{
    display_width, panel_contrast_fg, put_right_text, put_text, render_sidebar_background,
    status_color, status_dot, status_icon, status_text,
};
pub(crate) use tabs::{
    render_tab_bar_status, render_tab_strip, tab_bar_content_area, tab_bar_status_width,
    StatusSegment, Tab, TabBarState, TabBarStyle,
};
