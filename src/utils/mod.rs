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
    let mut changes = Vec::with_capacity(edits.len());
    for change in edits.iter().rev() {
        let Some(range) = &change.range else {
            log::warn!(
                "dropping textDocument/didChange edit without range; incremental sync is required"
            );
            continue;
        };

        let mut payload = serde_json::json!({ "newText": change.text });
        let ts_range = lsp_range_to_tsserver(range);
        if let Some(obj) = payload.as_object_mut() {
            obj.insert(
                "start".to_string(),
                serde_json::json!({
                    "line": ts_range.start.line,
                    "offset": ts_range.start.offset
                }),
            );
            obj.insert(
                "end".to_string(),
                serde_json::json!({
                    "line": ts_range.end.line,
                    "offset": ts_range.end.offset
                }),
            );
        }
        changes.push(payload);
    }
    changes
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Position, Range, TextDocumentContentChangeEvent, TextDocumentItem};
    use serde_json::json;

    fn range(start_line: u32, start_char: u32, end_line: u32, end_char: u32) -> Range {
        Range {
            start: Position {
                line: start_line,
                character: start_char,
            },
            end: Position {
                line: end_line,
                character: end_char,
            },
        }
    }

    #[test]
    fn lsp_range_to_tsserver_is_one_based() {
        let input = range(0, 0, 4, 15); // first line/column in LSP space
        let converted = lsp_range_to_tsserver(&input);
        assert_eq!(converted.start.line, 1);
        assert_eq!(converted.start.offset, 1);
        assert_eq!(converted.end.line, 5);
        assert_eq!(converted.end.offset, 16);
    }

    #[test]
    fn tsserver_text_changes_from_edits_skips_full_sync_edits() {
        let edits = vec![
            TextDocumentContentChangeEvent {
                range: Some(range(1, 2, 1, 5)),
                text: "foo".to_string(),
            },
            TextDocumentContentChangeEvent {
                range: None,
                text: "dropped".to_string(),
            },
        ];

        let changes = tsserver_text_changes_from_edits(&edits);
        assert_eq!(changes.len(), 1);
        assert_eq!(
            changes[0],
            json!({
                "newText": "foo",
                "start": {"line": 2, "offset": 3},
                "end": {"line": 2, "offset": 6}
            })
        );
    }

    #[test]
    fn file_path_uri_roundtrip() {
        let path = std::env::temp_dir().join("ts-bridge-test.ts");
        let path_str = path.to_str().expect("temp path is valid utf-8");
        let uri = file_path_to_uri(path_str).expect("path converts to URI");
        let roundtrip = uri_to_file_path(uri.as_str()).expect("URI converts back to path");
        assert_eq!(Path::new(&roundtrip), path);
    }

    #[test]
    fn lsp_text_doc_to_tsserver_entry_sets_project_root() {
        let doc = TextDocumentItem {
            uri: "file:///tmp/sample.ts".to_string(),
            language_id: Some("typescript".to_string()),
            version: 1,
            text: "const x = 1;".to_string(),
        };
        let root = Path::new("/tmp/project-root");
        let entry = lsp_text_doc_to_tsserver_entry(&doc, Some(root));
        assert_eq!(entry["file"], json!("/tmp/sample.ts"));
        assert_eq!(entry["fileContent"], json!("const x = 1;"));
        assert_eq!(entry["scriptKindName"], json!("TS"));
        assert_eq!(
            entry["projectRootPath"],
            json!(root.to_string_lossy().to_string())
        );
    }
}
