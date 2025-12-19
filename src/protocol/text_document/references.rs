//! =============================================================================
//! textDocument/references
//! =============================================================================
//!
//! Using tsserver’s `references` command and translating the resulting spans into standard LSP `Location`
//! values. We lean on tsserver’s optional `includeDefinition` flag so the
//! server itself filters declaration hits whenever the client did not request
//! them.

use anyhow::{Context, Result};
use lsp_types::{Location, ReferenceParams};
use serde_json::{Value, json};

use crate::protocol::RequestSpec;
use crate::rpc::{Priority, Route};
use crate::utils::{tsserver_span_to_location, uri_to_file_path};

const CMD_REFERENCES: &str = "references";

pub fn handle(params: ReferenceParams) -> RequestSpec {
    let text_document = params.text_document_position.text_document;
    let uri_string = text_document.uri.to_string();
    let file_name = uri_to_file_path(text_document.uri.as_str()).unwrap_or(uri_string);
    let position = params.text_document_position.position;
    let include_definition = params.context.include_declaration;

    let request = json!({
        "command": CMD_REFERENCES,
        "arguments": {
            "file": file_name,
            "line": position.line + 1,
            "offset": position.character + 1,
            "includeDefinition": include_definition,
        }
    });

    RequestSpec {
        route: Route::Syntax,
        payload: request,
        priority: Priority::Normal,
        on_response: Some(adapt_references),
        response_context: None,
    }
}

fn adapt_references(payload: &Value, _context: Option<&Value>) -> Result<Value> {
    let refs = payload
        .get("body")
        .context("tsserver references missing body")?
        .get("refs")
        .and_then(|value| value.as_array())
        .context("tsserver references missing refs array")?;

    let mut locations: Vec<Location> = Vec::with_capacity(refs.len());
    for span in refs {
        if let Some(location) = tsserver_span_to_location(span) {
            locations.push(location);
        }
    }

    Ok(serde_json::to_value(locations)?)
}
