use std::io::{self, Write};

use crate::protocol::render_ansi::BlitEncoder;
use crate::protocol::{CellData, CursorState, FrameData, PaneSurfacePatchRow};

/// The host terminal as last written: each present writes only the cells
/// that differ from it.
#[derive(Default)]
pub(crate) struct Screen {
    encoder: BlitEncoder,
    size: Option<(u16, u16)>,
}

impl Screen {
    /// Writes `rows` and `cursor` to `out` at `size` (`cols`, `rows`). A new
    /// size redraws the whole screen, so `rows` must then cover all of it.
    pub(crate) fn present(
        &mut self,
        out: &mut impl Write,
        size: (u16, u16),
        rows: Vec<PaneSurfacePatchRow>,
        cursor: Option<CursorState>,
    ) -> io::Result<()> {
        let rows = clip(rows, size);
        if self.size != Some(size) {
            if rows.is_empty() {
                return Ok(());
            }
            let frame = frame(size, &rows, cursor);
            let encoded = self.encoder.encode(&frame, true);
            write(out, &encoded.bytes)?;
            self.encoder.commit(frame, encoded);
            self.size = Some(size);
            return Ok(());
        }
        let Some(encoded) = self.encoder.encode_patch(&rows, cursor.clone(), false) else {
            tracing::warn!("view rows do not fit the screen; waiting for a full redraw");
            self.size = None;
            return Ok(());
        };
        write(out, &encoded.bytes)?;
        self.encoder.commit_patch(&rows, cursor, encoded);
        Ok(())
    }
}

fn write(out: &mut impl Write, bytes: &[u8]) -> io::Result<()> {
    if bytes.is_empty() {
        return Ok(());
    }
    out.write_all(bytes)?;
    out.flush()
}

fn clip(
    mut rows: Vec<PaneSurfacePatchRow>,
    (cols, height): (u16, u16),
) -> Vec<PaneSurfacePatchRow> {
    rows.retain(|row| row.y < height && row.x < cols);
    for row in &mut rows {
        row.cells.truncate(usize::from(cols - row.x));
    }
    rows
}

fn frame(
    (cols, rows): (u16, u16),
    patch: &[PaneSurfacePatchRow],
    cursor: Option<CursorState>,
) -> FrameData {
    let blank = CellData::from_ratatui_cell(&ratatui::buffer::Cell::default());
    let mut cells = vec![blank; usize::from(cols) * usize::from(rows)];
    crate::protocol::surface_delta::apply_rows(&mut cells, cols, patch);
    FrameData {
        cells,
        width: cols,
        height: rows,
        cursor,
        hyperlinks: Vec::new(),
        graphics: Vec::new(),
    }
}

#[cfg(test)]
mod tests;
