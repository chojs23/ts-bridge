//! =============================================================================
//! Utility Helpers
//! =============================================================================
//!
//! Range conversions, throttling/debouncing, and other helper utilities land
//! here so both the protocol handlers and the RPC bridge can reuse them without
//! reimplementing the same glue each time.

use crate::types::{Position, Range};

/// Converts an LSP `Range` into the tsserver 1-based coordinate space.
pub fn lsp_range_to_tsserver(range: &Range) -> TsserverRange {
    TsserverRange {
        start: lsp_position_to_tsserver(&range.start),
        end: lsp_position_to_tsserver(&range.end),
    }
}

pub fn lsp_position_to_tsserver(position: &Position) -> TsserverPosition {
    TsserverPosition {
        line: position.line + 1,
        offset: position.character + 1,
    }
}

/// Tsserver understands 1-based line/offset coordinates.
#[derive(Debug, Clone, Copy)]
pub struct TsserverPosition {
    pub line: u32,
    pub offset: u32,
}

#[derive(Debug, Clone, Copy)]
pub struct TsserverRange {
    pub start: TsserverPosition,
    pub end: TsserverPosition,
}
