use serde::{Deserialize, Serialize};

/// A bounded rectangle of the latest terminal screen at its native dimensions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct TerminalCellSnapshot {
    pub source_cols: u16,
    pub source_rows: u16,
    pub default_fg: u32,
    pub default_bg: u32,
    pub start_row: u32,
    pub width: u16,
    pub height: u16,
    pub cells: Vec<crate::protocol::CellData>,
}
