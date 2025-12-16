//! =============================================================================
//! Utility Helpers
//! =============================================================================
//!
//! Range conversions, throttling/debouncing, and other helper utilities land
//! here so both the protocol handlers and the RPC bridge can reuse them without
//! reimplementing the same glue each time.

use std::path::Path;
use std::str::FromStr;

use lsp_types::{CompletionItemKind, Location, LocationLink, Uri};
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

pub fn lsp_text_doc_to_tsserver_entry(
    doc: &TextDocumentItem,
    workspace_root: Option<&Path>,
) -> serde_json::Value {
    let file = uri_to_file_path(&doc.uri).unwrap_or_else(|| doc.uri.clone());
    let script_kind = script_kind_from_language(doc.language_id.as_deref());
    let mut entry = serde_json::json!({
        "file": file,
        "fileContent": doc.text,
        "scriptKindName": script_kind,
    });

    if let Some(root) = workspace_root {
        if let Some(obj) = entry.as_object_mut() {
            obj.insert(
                "projectRootPath".to_string(),
                serde_json::json!(root.to_string_lossy().into_owned()),
            );
        }
    }

    entry
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

pub fn tsserver_file_to_uri(path: &str) -> Option<Uri> {
    if path.starts_with("zipfile://") {
        Uri::from_str(path).ok()
    } else {
        file_path_to_uri(path)
    }
}

pub fn tsserver_span_to_location(value: &Value) -> Option<Location> {
    let file = value.get("file")?.as_str()?;
    let uri = tsserver_file_to_uri(file)?;
    let range = tsserver_range_from_value_lsp(value)?;
    Some(Location { uri, range })
}

pub fn tsserver_span_to_location_link(
    value: &Value,
    origin: Option<lsp_types::Range>,
) -> Option<LocationLink> {
    let file = value.get("file")?.as_str()?;
    let target_uri = tsserver_file_to_uri(file)?;
    let target_selection_range = tsserver_range_from_value_lsp(value)?;
    let target_range =
        if let (Some(start), Some(end)) = (value.get("contextStart"), value.get("contextEnd")) {
            tsserver_range_from_value_lsp(&serde_json::json!({ "start": start, "end": end }))
                .unwrap_or_else(|| target_selection_range.clone())
        } else {
            target_selection_range.clone()
        };

    Some(LocationLink {
        origin_selection_range: origin,
        target_range,
        target_selection_range,
        target_uri,
    })
}

pub fn completion_item_kind_from_tsserver(kind: Option<&str>) -> CompletionItemKind {
    match kind {
        Some("keyword") => CompletionItemKind::KEYWORD,
        Some("script") | Some("module") | Some("external module name") => {
            CompletionItemKind::MODULE
        }
        Some("class") | Some("local class") => CompletionItemKind::CLASS,
        Some("interface") => CompletionItemKind::INTERFACE,
        Some("type") | Some("type parameter") => CompletionItemKind::TYPE_PARAMETER,
        Some("enum") => CompletionItemKind::ENUM,
        Some("enum member") => CompletionItemKind::ENUM_MEMBER,
        Some("var") | Some("local var") | Some("let") => CompletionItemKind::VARIABLE,
        Some("function") | Some("local function") => CompletionItemKind::FUNCTION,
        Some("method") => CompletionItemKind::METHOD,
        Some("getter") | Some("setter") | Some("property") => CompletionItemKind::PROPERTY,
        Some("constructor") => CompletionItemKind::CONSTRUCTOR,
        Some("call") | Some("index") | Some("construct") => CompletionItemKind::METHOD,
        Some("parameter") => CompletionItemKind::FIELD,
        Some("primitive type") | Some("label") => CompletionItemKind::KEYWORD,
        Some("alias") => CompletionItemKind::VARIABLE,
        Some("const") => CompletionItemKind::CONSTANT,
        Some("directory") => CompletionItemKind::FILE,
        Some("string") => CompletionItemKind::CONSTANT,
        _ => CompletionItemKind::TEXT,
    }
}

pub fn completion_commit_characters(kind: CompletionItemKind) -> Option<Vec<String>> {
    match kind {
        CompletionItemKind::CLASS => Some(vec![".".into(), ",".into(), "(".into()]),
        CompletionItemKind::CONSTANT => Some(vec![".".into(), "?".into()]),
        CompletionItemKind::CONSTRUCTOR => Some(vec!["(".into()]),
        CompletionItemKind::ENUM => Some(vec![".".into()]),
        CompletionItemKind::FIELD => Some(vec![".".into(), "(".into()]),
        CompletionItemKind::FUNCTION => Some(vec![".".into(), "(".into()]),
        CompletionItemKind::INTERFACE => Some(vec![":".into(), ".".into()]),
        CompletionItemKind::METHOD => Some(vec!["(".into()]),
        CompletionItemKind::MODULE => Some(vec![".".into(), "?".into()]),
        CompletionItemKind::PROPERTY => Some(vec![".".into(), "?".into()]),
        CompletionItemKind::VARIABLE => Some(vec![".".into(), "?".into()]),
        _ => None,
    }
}
