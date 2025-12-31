//! =============================================================================
//! textDocument/documentSymbol
//! =============================================================================
//!
//! Uses tsserver's `navtree` command to fetch a hierarchical view of symbols
//! inside a file and translates it into LSP `DocumentSymbol` entries.

use anyhow::{Context, Result};
use lsp_types::{DocumentSymbolParams, Range, SymbolKind, SymbolTag};
use serde::Serialize;
use serde_json::{Value, json};

use crate::protocol::{AdapterResult, RequestSpec};
use crate::rpc::{Priority, Route};
use crate::utils::{tsserver_range_from_value_lsp, uri_to_file_path};

pub fn handle(params: DocumentSymbolParams) -> RequestSpec {
    let uri = params.text_document.uri;
    let file = uri_to_file_path(uri.as_str()).unwrap_or_else(|| uri.to_string());

    let request = json!({
        "command": "navtree",
        "arguments": { "file": file }
    });

    RequestSpec {
        route: Route::Syntax,
        payload: request,
        priority: Priority::Normal,
        on_response: Some(adapt_document_symbols),
        response_context: None,
    }
}

fn adapt_document_symbols(payload: &Value, _context: Option<&Value>) -> Result<AdapterResult> {
    let body = payload
        .get("body")
        .context("tsserver navtree missing body")?;

    let mut symbols = Vec::new();
    if let Some(children) = body.get("childItems").and_then(|v| v.as_array()) {
        for child in children {
            if let Some(symbol) = build_symbol(child) {
                symbols.push(symbol);
            }
        }
    } else if let Some(symbol) = build_symbol(body) {
        if symbol.name == "<global>" {
            if let Some(children) = symbol.children {
                symbols.extend(children);
            }
        } else {
            symbols.push(symbol);
        }
    }

    let serialized = symbols.into_iter().map(|symbol| json!(symbol)).collect();
    Ok(AdapterResult::ready(Value::Array(serialized)))
}

fn build_symbol(node: &Value) -> Option<BridgeDocumentSymbol> {
    let name = node.get("text")?.as_str()?.to_string();
    let kind = node
        .get("kind")
        .and_then(|k| k.as_str())
        .map(symbol_kind_from_ts)
        .unwrap_or(SymbolKind::VARIABLE);
    let range = symbol_range(node)?;
    let modifiers = node
        .get("kindModifiers")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let detail = if modifiers.is_empty() {
        None
    } else {
        Some(modifiers.to_string())
    };

    let child_items = node
        .get("childItems")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let children = child_items
        .into_iter()
        .filter_map(|child| build_symbol(&child))
        .collect::<Vec<_>>();

    Some(BridgeDocumentSymbol {
        name,
        detail,
        kind,
        range: range.clone(),
        selection_range: range,
        children: if children.is_empty() {
            None
        } else {
            Some(children)
        },
        tags: document_symbol_tags(modifiers),
    })
}

fn document_symbol_tags(modifiers: &str) -> Option<Vec<SymbolTag>> {
    let contains_deprecated = modifiers
        .split(|c: char| matches!(c, ',' | ' ' | ';' | '\t'))
        .any(|token| token.eq_ignore_ascii_case("deprecated"));
    if contains_deprecated {
        Some(vec![SymbolTag::DEPRECATED])
    } else {
        None
    }
}

fn symbol_range(node: &Value) -> Option<lsp_types::Range> {
    if let Some(spans) = node.get("spans").and_then(|v| v.as_array()) {
        for span in spans {
            if let Some(range) = tsserver_range_from_value_lsp(span) {
                return Some(range);
            }
        }
    }
    if let Some(span) = node.get("textSpan") {
        return tsserver_range_from_value_lsp(span);
    }
    None
}

fn symbol_kind_from_ts(kind: &str) -> SymbolKind {
    match kind {
        "class" => SymbolKind::CLASS,
        "interface" => SymbolKind::INTERFACE,
        "enum" => SymbolKind::ENUM,
        "method" => SymbolKind::METHOD,
        "function" => SymbolKind::FUNCTION,
        "member" | "property" | "getter" | "setter" => SymbolKind::PROPERTY,
        "var" | "let" | "const" => SymbolKind::VARIABLE,
        "module" => SymbolKind::MODULE,
        "namespace" => SymbolKind::NAMESPACE,
        "type" => SymbolKind::TYPE_PARAMETER,
        _ => SymbolKind::VARIABLE,
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct BridgeDocumentSymbol {
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    detail: Option<String>,
    kind: SymbolKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    tags: Option<Vec<SymbolTag>>,
    range: Range,
    selection_range: Range,
    #[serde(skip_serializing_if = "Option::is_none")]
    children: Option<Vec<BridgeDocumentSymbol>>,
}
