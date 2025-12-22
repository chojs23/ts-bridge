//! =============================================================================
//! textDocument/implementation
//! =============================================================================
//!
//! Mirrors the definition/typeDefinition handlers but targets tsserver’s
//! `implementation` command so users can jump to interface/class/abstract
//! implementations directly from Neovim.

use anyhow::{Context, Result};
use lsp_types::{GotoDefinitionParams, GotoDefinitionResponse};
use serde_json::{Value, json};

use crate::protocol::RequestSpec;
use crate::rpc::{Priority, Route};
use crate::utils::{tsserver_span_to_location_link, uri_to_file_path};

const CMD_IMPLEMENTATION: &str = "implementation";

pub fn handle(params: GotoDefinitionParams) -> RequestSpec {
    let text_document = params.text_document_position_params.text_document;
    let uri_string = text_document.uri.to_string();
    let file_name = uri_to_file_path(text_document.uri.as_str()).unwrap_or(uri_string);
    let position = params.text_document_position_params.position;

    let request = json!({
        "command": CMD_IMPLEMENTATION,
        "arguments": {
            "file": file_name,
            "line": position.line + 1,
            "offset": position.character + 1,
        }
    });

    RequestSpec {
        route: Route::Syntax,
        payload: request,
        priority: Priority::Normal,
        on_response: Some(adapt_implementation),
        response_context: None,
    }
}

fn adapt_implementation(payload: &Value, _context: Option<&Value>) -> Result<Value> {
    let body = payload
        .get("body")
        .context("tsserver implementation missing body")?;
    let locations = body
        .as_array()
        .context("tsserver implementation body must be array")?;

    let mut links = Vec::new();
    for span in locations {
        if let Some(link) = tsserver_span_to_location_link(span, None) {
            links.push(link);
        }
    }

    let response = GotoDefinitionResponse::Link(links);
    Ok(serde_json::to_value(response)?)
}
