use super::*;

pub(super) fn capture(
    core: &GhosttyPaneCore,
    max_cols: u16,
    max_rows: u16,
) -> Result<Option<crate::terminal::TerminalCellSnapshot>, crate::ghostty::Error> {
    if core
        .terminal
        .mode_get(crate::ghostty::MODE_SYNCHRONIZED_OUTPUT)?
    {
        return Ok(None);
    }
    let source_cols = core.terminal.cols()?;
    let source_rows = core.terminal.rows()?;
    let Some((_, mut end, _)) =
        ghostty_recent_read_range(&core.terminal, usize::from(max_rows.min(source_rows)))?
    else {
        return Ok(None);
    };
    // Text reads trim painted blank rows; preserve their cells in styled snapshots.
    for row in (end + 1..core.terminal.total_rows()?).rev() {
        let Ok(screen_row) = u32::try_from(row) else {
            return Ok(None);
        };
        let mut painted = false;
        core.terminal.for_each_screen_row_styled_cell(
            screen_row,
            source_cols,
            |_, _, style, content_bg| {
                painted |= content_bg.is_some() || style != crate::ghostty::CellStyle::default();
            },
        )?;
        if painted {
            end = row;
            break;
        }
    }
    let start = end
        .saturating_add(1)
        .saturating_sub(usize::from(max_rows.min(source_rows)));
    let width = source_cols.min(max_cols);
    let (Ok(height), Ok(start_row), Ok(end_row)) = (
        u16::try_from(end - start + 1),
        u32::try_from(start),
        u32::try_from(end),
    ) else {
        return Ok(None);
    };
    let colors = core.terminal.screen_colors(core.render_state.colors()?)?;
    let default_fg = ghostty_default_fg(
        colors.foreground,
        core.host_terminal_theme,
        core.initial_default_foreground,
    );
    let default_bg = ghostty_default_bg(
        colors.background,
        core.host_terminal_theme,
        core.initial_default_background,
    );
    let palette = core.terminal.default_palette()?;
    let overrides = PaletteOverrides::new(&colors.palette, &palette);
    let mut cells = Vec::with_capacity(usize::from(width) * usize::from(height));
    let hide_placeholders = crate::kitty_graphics::is_enabled();
    for row in start_row..=end_row {
        core.terminal.for_each_screen_row_styled_cell(
            row,
            width,
            |wide, graphemes, style, content_bg| {
                let fg = style
                    .fg_color
                    .map(|color| ghostty_cell_color(color, overrides.as_ref()))
                    .or(default_fg);
                let bg = content_bg
                    .or(style.bg_color)
                    .map(|color| ghostty_cell_color(color, overrides.as_ref()))
                    .or(default_bg);
                let style = ghostty_resolved_cell_style(
                    &style,
                    fg,
                    bg,
                    Some(ghostty_color(colors.foreground)),
                    Some(ghostty_color(colors.background)),
                    overrides.as_ref(),
                );
                let mut symbol = match wide {
                    crate::ghostty::CellWide::SpacerHead => " ".into(),
                    crate::ghostty::CellWide::SpacerTail => String::new(),
                    _ => graphemes
                        .iter()
                        .filter_map(|cp| char::from_u32(*cp))
                        .collect(),
                };
                if symbol.is_empty() && wide != crate::ghostty::CellWide::SpacerTail
                    || hide_placeholders
                        && symbol.chars().next().map(u32::from)
                            == Some(crate::ghostty::KITTY_UNICODE_PLACEHOLDER)
                {
                    symbol.clear();
                    symbol.push(' ');
                }
                ghostty_normalize_buffer_symbol_in_place(&mut symbol, wide);
                if cells.len() % usize::from(width) + 1 == usize::from(width)
                    && (wide == crate::ghostty::CellWide::Wide || symbol.width() > 1)
                {
                    symbol.clear();
                    symbol.push(' ');
                }
                cells.push(cell_data_from_style(symbol, style));
            },
        )?;
    }
    Ok(Some(crate::terminal::TerminalCellSnapshot {
        source_cols,
        source_rows,
        default_fg: crate::protocol::color_to_u32(default_fg.unwrap_or(Color::Reset)),
        default_bg: crate::protocol::color_to_u32(default_bg.unwrap_or(Color::Reset)),
        start_row,
        width,
        height,
        cells,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pane(cols: u16, rows: u16, output: &str) -> GhosttyPaneTerminal {
        let (tx, _rx) = mpsc::channel(4);
        let terminal = crate::ghostty::Terminal::new(cols, rows, 100).unwrap();
        let pane = GhosttyPaneTerminal::new(terminal, tx).unwrap();
        pane.core.lock().unwrap().terminal.write(output.as_bytes());
        pane
    }

    #[test]
    fn cell_snapshot_matches_normal_frame_styles_and_graphemes() {
        for output in [
            "\x1b[31;1mred\x1b[0m \x1b[38;5;171midx\x1b[0m\r\n\x1b[38;2;1;2;3;48;5;4mtrue\x1b[0m 界 e\u{301}\r\n\x1b[2;3;4mstyles\x1b[0m \x1b[7minverse\x1b[0m",
            "\x1b]4;18;rgb:11/22/33\x07\x1b[38;5;18mcustom\x1b[0m\r\n\x1b[48;5;4m\x1b[K\x1b[0m\r\n\x1b[48;2;17;34;51m\x1b[K\x1b[0mend",
            "\x1b]10;#abcdef\x07\x1b]11;#123456\x07default\r\n\x1b[8mhidden\x1b[0m\r\n\x1b[7mreverse\x1b[0m",
            "\x1b[?1049h\x1b[?5h\x1b[31mreverse screen\x1b[0m\r\n\x1b[48;5;4m\x1b[K\x1b[0m\r\nlast",
            "\x1b[44m\x1b[2J\x1b[Hhi\x1b[0m",
            "\x1b[48;2;17;34;51m\x1b[2J\x1b[Hhi\x1b[0m",
        ] {
            let pane = pane(24, 3, output);
            let snapshot = pane.cell_snapshot(24, 3).unwrap();
            assert_eq!((snapshot.width, snapshot.height), (24, 3));
            let backend = ratatui::backend::TestBackend::new(24, 3);
            let mut terminal = ratatui::Terminal::new(backend).unwrap();
            let mut expected = Vec::new();
            terminal
                .draw(|frame| {
                    pane.render(frame, Rect::new(0, 0, 24, 3), false);
                    expected = crate::protocol::FrameData::from_ratatui_buffer(
                        frame.buffer_mut(),
                        None,
                    )
                    .cells;
                })
                .unwrap();
            for (index, (actual, expected)) in snapshot.cells.iter().zip(&expected).enumerate() {
                assert_eq!(actual, expected, "cell {index}, output {output:?}");
            }
        }
    }

    #[test]
    fn cell_snapshot_keeps_latest_output_anchor_for_default_blank_rows() {
        let pane = pane(24, 3, "hi");
        let snapshot = pane.cell_snapshot(24, 3).unwrap();
        assert_eq!(
            (snapshot.start_row, snapshot.width, snapshot.height),
            (0, 24, 1)
        );
        assert_eq!(snapshot.cells[0].symbol, "h");
        assert_eq!(snapshot.cells[1].symbol, "i");
        assert_eq!(
            snapshot.default_fg,
            crate::protocol::color_to_u32(Color::Reset)
        );
        assert_eq!(
            snapshot.default_bg,
            crate::protocol::color_to_u32(Color::Reset)
        );
    }

    #[test]
    fn cell_snapshot_exports_native_default_colors_for_uncovered_cells() {
        let pane = pane(24, 3, "\x1b]10;#abcdef\x07\x1b]11;#123456\x07hi");
        let snapshot = pane.cell_snapshot(24, 3).unwrap();
        assert_eq!((snapshot.width, snapshot.height), (24, 1));
        assert_eq!(
            snapshot.default_fg,
            crate::protocol::color_to_u32(Color::Rgb(0xab, 0xcd, 0xef))
        );
        assert_eq!(
            snapshot.default_bg,
            crate::protocol::color_to_u32(Color::Rgb(0x12, 0x34, 0x56))
        );
        assert_eq!(snapshot.cells[23].fg, snapshot.default_fg);
        assert_eq!(snapshot.cells[23].bg, snapshot.default_bg);
        let backend = ratatui::backend::TestBackend::new(24, 3);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| pane.render(frame, Rect::new(0, 0, 24, 3), false))
            .unwrap();
        assert_eq!(
            snapshot.default_bg,
            crate::protocol::color_to_u32(terminal.backend().buffer()[(23, 2)].bg)
        );
    }

    #[test]
    fn cell_snapshot_leaves_viewport_and_pending_render_updates_unchanged() {
        for scrolled in [false, true] {
            let output = "old\r\n".repeat(12);
            let baseline = pane(16, 4, &output);
            let observed = pane(16, 4, &output);
            for pane in [&baseline, &observed] {
                assert!(matches!(
                    pane.collect_dirty_patch(16, 4),
                    TerminalDirtyPatchOutcome::Patch(_)
                ));
                pane.core
                    .lock()
                    .unwrap()
                    .terminal
                    .write(b"\x1b[31mLATEST\x1b[0m");
                if scrolled {
                    pane.set_scroll_offset_from_bottom(8);
                }
            }
            let before = observed.scroll_metrics().unwrap();
            let snapshot = observed.cell_snapshot(16, 4).unwrap();
            let after = observed.scroll_metrics().unwrap();
            assert_eq!(before.offset_from_bottom, after.offset_from_bottom);
            assert_eq!(before.max_offset_from_bottom, after.max_offset_from_bottom);
            assert_eq!(before.viewport_rows, after.viewport_rows);
            assert!(snapshot
                .cells
                .iter()
                .map(|cell| cell.symbol.as_str())
                .collect::<String>()
                .contains("LATEST"));
            match (
                baseline.collect_dirty_patch(16, 4),
                observed.collect_dirty_patch(16, 4),
            ) {
                (
                    TerminalDirtyPatchOutcome::Patch(expected),
                    TerminalDirtyPatchOutcome::Patch(actual),
                ) => {
                    assert_eq!(actual.rows, expected.rows);
                }
                (expected, actual) => {
                    panic!("expected matching pending patches: {expected:?}, {actual:?}")
                }
            }
            assert!(matches!(
                observed.collect_dirty_patch(16, 4),
                TerminalDirtyPatchOutcome::Clean
            ));
        }
    }

    #[test]
    fn cell_snapshot_bounds_capture_and_blanks_partial_wide_cells() {
        let pane = pane(8, 3, "first\r\nsecond\r\n\x1b[31m界e\u{301}");
        let snapshot = pane.cell_snapshot(1, 1).unwrap();
        assert_eq!((snapshot.source_cols, snapshot.source_rows), (8, 3));
        assert_eq!(
            (snapshot.start_row, snapshot.width, snapshot.height),
            (2, 1, 1)
        );
        assert_eq!(snapshot.cells.len(), 1);
        assert_eq!(snapshot.cells[0].symbol, " ");
        assert_eq!(
            snapshot.cells[0].fg,
            crate::protocol::color_to_u32(Color::Indexed(1))
        );
        for (cols, rows) in [(0, 1), (1, 0), (513, 1), (1, 257), (512, 256)] {
            assert!(pane.cell_snapshot(cols, rows).is_none());
        }
    }

    #[test]
    fn cell_snapshot_defers_synchronized_output_and_busy_terminal() {
        let pane = pane(8, 3, "before");
        {
            let mut core = pane.core.lock().unwrap();
            assert!(pane.cell_snapshot(8, 3).is_none());
            core.terminal.write(b"\x1b[?2026h\rnew");
        }
        assert!(pane.cell_snapshot(8, 3).is_none());
        pane.core.lock().unwrap().terminal.write(b"\x1b[?2026l");
        assert!(pane.cell_snapshot(8, 3).is_some());
    }
}
