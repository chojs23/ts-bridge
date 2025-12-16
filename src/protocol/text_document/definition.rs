//! =============================================================================
//! textDocument/definition
//! =============================================================================
//!
//! Tsserver powers definition requests through the `definitionAndBoundSpan`
//! command (plus `findSourceDefinition` for source preference).  This handler
//! mirrors the Lua implementation by converting each returned `FileSpanWithContext`
//! into an LSP `LocationLink` so the client can show peek-definition previews
//! with context.

use std::str::FromStr;

use anyhow::{Context, Result};
use lsp_types::{GotoDefinitionParams, GotoDefinitionResponse, LocationLink, Uri};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::protocol::RequestSpec;
use crate::rpc::{Priority, Route};
use crate::utils::{file_path_to_uri, tsserver_range_from_value_lsp, uri_to_file_path};

const CMD_DEFINITION: &str = "definitionAndBoundSpan";
const CMD_SOURCE_DEFINITION: &str = "findSourceDefinition";

#[derive(Deserialize)]
pub struct DefinitionParams {
    #[serde(flatten)]
    pub base: GotoDefinitionParams,
    #[serde(default)]
    pub context: Option<DefinitionContext>,
}

#[derive(Debug, Deserialize, Default)]
pub struct DefinitionContext {
    pub source_definition: Option<bool>,
}

pub fn handle(params: DefinitionParams) -> RequestSpec {
    let text_document = params
        .base
        .text_document_position_params
        .text_document;
    let uri_string = text_document.uri.to_string();
    let file_name = uri_to_file_path(text_document.uri.as_str()).unwrap_or(uri_string);

    let position = params
        .base
        .text_document_position_params
        .position;
    let use_source_definition = params
        .context
        .and_then(|ctx| ctx.source_definition)
        .unwrap_or(false);
    let command = if use_source_definition {
        CMD_SOURCE_DEFINITION
    } else {
        CMD_DEFINITION
    };
    let request = json!({
        "command": command,
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
        on_response: Some(adapt_definition),
    }
}

fn adapt_definition(payload: &Value) -> Result<Value> {
    let command = payload
        .get("command")
        .and_then(|cmd| cmd.as_str())
        .unwrap_or(CMD_DEFINITION);
    let body = payload
        .get("body")
        .context("tsserver definition missing body")?;

    let origin_selection = body.get("textSpan").and_then(tsserver_range_from_value_lsp);

    let defs: Box<dyn Iterator<Item = &Value> + '_> = if command == CMD_SOURCE_DEFINITION {
        let array = body
            .as_array()
            .context("source definition body must be array")?;
        Box::new(array.iter())
    } else {
        let array = body
            .get("definitions")
            .and_then(|value| value.as_array())
            .context("definition body missing definitions array")?;
        Box::new(array.iter())
    };

    let mut links = Vec::new();
    for def in defs {
        if let Some(link) = file_span_to_location_link(def, origin_selection.clone())? {
            links.push(link);
        }
    }

    let response = GotoDefinitionResponse::Link(links);
    Ok(serde_json::to_value(response)?)
}

fn file_span_to_location_link(
    span_value: &Value,
    origin_selection: Option<lsp_types::Range>,
) -> Result<Option<LocationLink>> {
    let file = match span_value.get("file").and_then(|f| f.as_str()) {
        Some(file) => file,
        None => return Ok(None),
    };
    let target_uri = resolve_span_uri(file).context("invalid definition uri")?;

    let target_selection = match tsserver_range_from_value_lsp(span_value) {
        Some(range) => range,
        None => return Ok(None),
    };

    let target_range = if let (Some(start), Some(end)) =
        (span_value.get("contextStart"), span_value.get("contextEnd"))
    {
        tsserver_range_from_value_lsp(&json!({
            "start": start,
            "end": end
        }))
        .unwrap_or(target_selection.clone())
    } else {
        target_selection.clone()
    };

    Ok(Some(LocationLink {
        origin_selection_range: origin_selection,
        target_range,
        target_selection_range: target_selection,
        target_uri,
    }))
}

fn resolve_span_uri(path: &str) -> Result<Uri> {
    if path.starts_with("zipfile://") {
        Uri::from_str(path).context("invalid zipfile uri")
    } else {
        file_path_to_uri(path).context("failed to convert path to uri")
    }
}
