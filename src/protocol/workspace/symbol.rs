use anyhow::Result;
use lsp_types::{Location, SymbolKind, SymbolTag, WorkspaceSymbolParams};
use serde::Serialize;
use serde_json::{Value, json};

use crate::protocol::RequestSpec;
use crate::rpc::{Priority, Route};
use crate::utils::tsserver_span_to_location;

pub fn handle(params: WorkspaceSymbolParams) -> RequestSpec {
    let request = json!({
        "command": "navto",
        "arguments": {
            "searchValue": params.query,
            "maxResultCount": 256,
            "start": 0,
            "projectFileName": None::<String>
        }
    });

    RequestSpec {
        route: Route::Syntax,
        payload: request,
        priority: Priority::Normal,
        on_response: Some(adapt_workspace_symbols),
        response_context: None,
    }
}

fn adapt_workspace_symbols(payload: &Value, _context: Option<&Value>) -> Result<Value> {
    let items = payload
        .get("body")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    let mut symbols = Vec::new();
    for item in items {
        if let Some(symbol) = convert_navto_item(&item) {
            symbols.push(json!(symbol));
        }
    }

    Ok(Value::Array(symbols))
}

fn convert_navto_item(item: &Value) -> Option<WorkspaceSymbol> {
    let name = item.get("name")?.as_str()?.to_string();
    let kind = item
        .get("kind")
        .and_then(|k| k.as_str())
        .map(document_symbol_kind)
        .unwrap_or(SymbolKind::VARIABLE);
    let location = item.get("textSpan").and_then(tsserver_span_to_location)?;
    let modifiers = item
        .get("kindModifiers")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let container_name = item
        .get("containerName")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    Some(WorkspaceSymbol {
        name,
        kind,
        location,
        container_name,
        tags: workspace_symbol_tags(modifiers),
    })
}

fn workspace_symbol_tags(modifiers: &str) -> Option<Vec<SymbolTag>> {
    let contains_deprecated = modifiers
        .split(|c: char| matches!(c, ',' | ' ' | ';' | '\t'))
        .any(|token| token.eq_ignore_ascii_case("deprecated"));
    if contains_deprecated {
        Some(vec![SymbolTag::DEPRECATED])
    } else {
        None
    }
}

fn document_symbol_kind(kind: &str) -> SymbolKind {
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
struct WorkspaceSymbol {
    name: String,
    kind: SymbolKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    tags: Option<Vec<SymbolTag>>,
    location: Location,
    #[serde(skip_serializing_if = "Option::is_none")]
    container_name: Option<String>,
}
