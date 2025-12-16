//! =============================================================================
//! Utility Helpers
//! =============================================================================
//!
//! Range conversions, throttling/debouncing, and other helper utilities land
//! here so both the protocol handlers and the RPC bridge can reuse them without
//! reimplementing the same glue each time.

use crate::types::{Position, Range, TextDocumentContentChangeEvent, TextDocumentItem};

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

pub fn lsp_text_doc_to_tsserver_entry(doc: &TextDocumentItem) -> serde_json::Value {
    serde_json::json!({
        "file": doc.uri,
        "fileContent": doc.text,
        "scriptKindName": doc.language_id.clone().unwrap_or_else(|| "TS".to_string()),
    })
}

pub fn tsserver_text_changes_from_edits(
    edits: &[TextDocumentContentChangeEvent],
) -> Vec<serde_json::Value> {
    edits
        .iter()
        .map(|change| {
            if let Some(range) = &change.range {
                let ts_range = lsp_range_to_tsserver(range);
                serde_json::json!({
                    "start": { "line": ts_range.start.line, "offset": ts_range.start.offset },
                    "end": { "line": ts_range.end.line, "offset": ts_range.end.offset },
                    "newText": change.text,
                })
            } else {
                serde_json::json!({
                    "newText": change.text,
                })
            }
        })
        .collect()
}
