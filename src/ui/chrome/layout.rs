use crate::config::{SidebarCollapsedModeConfig, TabBarPositionConfig};
use ratatui::layout::Rect;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Layout {
    pub sidebar: Rect,
    pub tab_bar: Rect,
    pub mobile_header: Rect,
    pub pane_surface: Rect,
    pub parking_header: Rect,
}

pub(crate) struct LayoutSettings {
    pub(crate) sidebar_min_width: u16,
    pub(crate) sidebar_max_width: u16,
    pub(crate) mobile_width_threshold: u16,
    pub(crate) tab_bar_position: TabBarPositionConfig,
    pub(crate) hide_tab_bar_when_single_tab: bool,
    pub(crate) sidebar_collapsed_mode: SidebarCollapsedModeConfig,
}

impl LayoutSettings {
    pub(crate) fn layout(
        &self,
        cols: u16,
        rows: u16,
        sidebar_collapsed: bool,
        single_tab: bool,
        sidebar_width: u16,
    ) -> Layout {
        if cols <= self.mobile_width_threshold {
            let header_height = rows.min(2);
            return Layout {
                parking_header: Rect::default(),
                sidebar: Rect::default(),
                tab_bar: Rect::default(),
                mobile_header: Rect::new(0, 0, cols, header_height),
                pane_surface: Rect::new(0, header_height, cols, rows.saturating_sub(header_height)),
            };
        }

        let sidebar_width = if sidebar_collapsed {
            match self.sidebar_collapsed_mode {
                SidebarCollapsedModeConfig::Compact => 4,
                SidebarCollapsedModeConfig::Hidden => 0,
            }
        } else {
            let (min, max) = crate::config::validated_sidebar_bounds(
                self.sidebar_min_width,
                self.sidebar_max_width,
            )
            .unwrap_or((18, 36));
            sidebar_width.clamp(min, max)
        }
        .min(cols.saturating_sub(1));
        let main = Rect::new(sidebar_width, 0, cols.saturating_sub(sidebar_width), rows);
        let show_tab_bar = rows > 1 && !(self.hide_tab_bar_when_single_tab && single_tab);
        let tab_height = u16::from(show_tab_bar);
        let (tab_bar, pane_surface) = match self.tab_bar_position {
            TabBarPositionConfig::Top => (
                Rect::new(main.x, 0, main.width, tab_height),
                Rect::new(
                    main.x,
                    tab_height,
                    main.width,
                    rows.saturating_sub(tab_height),
                ),
            ),
            TabBarPositionConfig::Bottom => (
                Rect::new(
                    main.x,
                    rows.saturating_sub(tab_height),
                    main.width,
                    tab_height,
                ),
                Rect::new(main.x, 0, main.width, rows.saturating_sub(tab_height)),
            ),
        };

        Layout {
            parking_header: Rect::default(),
            sidebar: Rect::new(0, 0, sidebar_width, rows),
            tab_bar,
            mobile_header: Rect::default(),
            pane_surface,
        }
    }
}
