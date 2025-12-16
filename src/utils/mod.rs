//! =============================================================================
//! Utility Helpers
//! =============================================================================
//!
//! Range conversions, throttling/debouncing, and other helper utilities land
//! here so both the protocol handlers and the RPC bridge can reuse them without
//! reimplementing the same glue each time.

use std::str::FromStr;

use lsp_types::Uri;
use serde_json::Value;
use url::Url;

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

pub fn uri_to_file_path(uri: &str) -> Option<String> {
    let parsed = Url::parse(uri).ok()?;
    parsed
        .to_file_path()
        .ok()
        .map(|p| p.to_string_lossy().into_owned())
}

pub fn file_path_to_uri(path: &str) -> Option<Uri> {
    let url = Url::from_file_path(path).ok()?;
    Uri::from_str(url.as_str()).ok()
}

pub fn lsp_text_doc_to_tsserver_entry(doc: &TextDocumentItem) -> serde_json::Value {
    let file = uri_to_file_path(&doc.uri).unwrap_or_else(|| doc.uri.clone());
    let script_kind = script_kind_from_language(doc.language_id.as_deref());
    serde_json::json!({
        "file": file,
        "fileContent": doc.text,
        "scriptKindName": script_kind,
    })
}

fn script_kind_from_language(lang: Option<&str>) -> &'static str {
    match lang {
        Some("javascript") => "JS",
        Some("javascriptreact") => "JSX",
        Some("typescriptreact") => "TSX",
        Some("json") => "JSON",
        _ => "TS",
    }
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

pub fn tsserver_position_value(value: &Value) -> Option<Position> {
    let line = value.get("line")?.as_u64()? as u32;
    let offset = value.get("offset")?.as_u64()? as u32;
    Some(Position {
        line: line.saturating_sub(1),
        character: offset.saturating_sub(1),
    })
}

pub fn tsserver_range_from_value(value: &Value) -> Option<Range> {
    let start = tsserver_position_value(value.get("start")?)?;
    let end = tsserver_position_value(value.get("end")?)?;
    Some(Range { start, end })
}

pub fn tsserver_position_value_lsp(value: &Value) -> Option<lsp_types::Position> {
    let pos = tsserver_position_value(value)?;
    Some(lsp_types::Position {
        line: pos.line,
        character: pos.character,
    })
}

pub fn tsserver_range_from_value_lsp(value: &Value) -> Option<lsp_types::Range> {
    let start = tsserver_position_value_lsp(value.get("start")?)?;
    let end = tsserver_position_value_lsp(value.get("end")?)?;
    Some(lsp_types::Range { start, end })
}
