use anyhow::Result;
use lsp_types::{SymbolInformation, WorkspaceSymbolParams};
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
            symbols.push(symbol);
        }
    }

    Ok(serde_json::to_value(symbols)?)
}

fn convert_navto_item(item: &Value) -> Option<SymbolInformation> {
    let name = item.get("name")?.as_str()?.to_string();
    let kind = item
        .get("kind")
        .and_then(|k| k.as_str())
        .map(document_symbol_kind)
        .unwrap_or(lsp_types::SymbolKind::VARIABLE);
    let location = item.get("textSpan").and_then(tsserver_span_to_location)?;
    let container_name = item
        .get("containerName")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    Some(SymbolInformation {
        name,
        kind,
        location,
        container_name,
        tags: None,
        deprecated: None,
    })
}

fn document_symbol_kind(kind: &str) -> lsp_types::SymbolKind {
    match kind {
        "class" => lsp_types::SymbolKind::CLASS,
        "interface" => lsp_types::SymbolKind::INTERFACE,
        "enum" => lsp_types::SymbolKind::ENUM,
        "method" => lsp_types::SymbolKind::METHOD,
        "function" => lsp_types::SymbolKind::FUNCTION,
        "member" | "property" | "getter" | "setter" => lsp_types::SymbolKind::PROPERTY,
        "var" | "let" | "const" => lsp_types::SymbolKind::VARIABLE,
        "module" => lsp_types::SymbolKind::MODULE,
        "namespace" => lsp_types::SymbolKind::NAMESPACE,
        "type" => lsp_types::SymbolKind::TYPE_PARAMETER,
        _ => lsp_types::SymbolKind::VARIABLE,
    }
}
