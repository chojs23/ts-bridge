//! =============================================================================
//! textDocument/prepareRename & textDocument/rename
//! =============================================================================
//!
//! Both LSP entry points reuse the tsserver `rename` command.  Calling the same
//! backend for prepare + execute guarantees that Neovim sees identical gating
//! heuristics (`canRename`, placeholder text, etc.) and that we do not have to
//! special-case `getRenameInfo` responses.

use std::collections::HashMap;

use anyhow::{Context, Result, anyhow};
use lsp_types::{
    PrepareRenameResponse, RenameParams, TextDocumentPositionParams, TextEdit, Uri, WorkspaceEdit,
};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::protocol::RequestSpec;
use crate::rpc::{Priority, Route};
use crate::utils::{tsserver_file_to_uri, tsserver_range_from_value_lsp, uri_to_file_path};

#[derive(Debug, Deserialize)]
struct RenameContext {
    new_text: String,
}

pub fn handle_prepare(params: TextDocumentPositionParams) -> RequestSpec {
    let uri = params.text_document.uri;
    let position = params.position;
    let file = uri_to_file_path(uri.as_str()).unwrap_or_else(|| uri.to_string());

    // Run tsserver's `rename` command without `newName` so we get renameability
    // metadata (trigger span, placeholder) straight from the source of truth.
    let request = json!({
        "command": "rename",
        "arguments": {
            "file": file,
            "line": position.line + 1,
            "offset": position.character + 1,
            "findInStrings": false,
            "findInComments": false,
        }
    });

    RequestSpec {
        route: Route::Syntax,
        payload: request,
        priority: Priority::Normal,
        on_response: Some(adapt_prepare_rename),
        response_context: None,
    }
}

pub fn handle(params: RenameParams) -> RequestSpec {
    let RenameParams {
        text_document_position,
        new_name,
        work_done_progress_params: _,
    } = params;
    let uri = text_document_position.text_document.uri;
    let position = text_document_position.position;
    let file = uri_to_file_path(uri.as_str()).unwrap_or_else(|| uri.to_string());

    // Same command path as prepare, but now we feed tsserver the desired name
    // so we can translate the `locs` array into a WorkspaceEdit.
    let request = json!({
        "command": "rename",
        "arguments": {
            "file": file,
            "line": position.line + 1,
            "offset": position.character + 1,
            "newName": new_name,
            "findInStrings": false,
            "findInComments": false,
        }
    });

    let context = json!({ "new_text": new_name });

    RequestSpec {
        route: Route::Syntax,
        payload: request,
        priority: Priority::Normal,
        on_response: Some(adapt_rename),
        response_context: Some(context),
    }
}

fn adapt_prepare_rename(payload: &Value, _context: Option<&Value>) -> Result<Value> {
    let body = payload.get("body").context("rename info missing body")?;
    let info = body.get("info").unwrap_or(body);
    let can_rename = info
        .get("canRename")
        .or_else(|| info.get("canRenameResult"))
        .and_then(|value| value.as_bool())
        .unwrap_or(false);
    if !can_rename {
        let message = info
            .get("localizedErrorMessage")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| "rename not allowed here".to_string());
        return Err(anyhow!(message));
    }

    let span = info
        .get("triggerSpan")
        .or_else(|| info.get("textSpan"))
        .or_else(|| info.get("span"))
        .context("rename info missing span")?;
    let range = tsserver_range_from_value_lsp(span).context("invalid rename span")?;
    let placeholder = info
        .get("displayName")
        .or_else(|| info.get("fullDisplayName"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let response = PrepareRenameResponse::RangeWithPlaceholder { range, placeholder };
    Ok(serde_json::to_value(response)?)
}

fn adapt_rename(payload: &Value, context: Option<&Value>) -> Result<Value> {
    let ctx: RenameContext =
        serde_json::from_value(context.cloned().context("missing rename context")?)?;
    let body = payload.get("body").context("rename missing body")?;
    let info = body.get("info").context("rename missing info")?;
    if info.get("canRename").and_then(|v| v.as_bool()) == Some(false) {
        let message = info
            .get("localizedErrorMessage")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| "rename not allowed here".to_string());
        return Err(anyhow!(message));
    }

    let mut map: HashMap<Uri, Vec<TextEdit>> = HashMap::new();
    let locs = body
        .get("locs")
        .and_then(|value| value.as_array())
        .cloned()
        .unwrap_or_default();
    for entry in locs {
        let file = entry
            .get("file")
            .and_then(|v| v.as_str())
            .context("rename result missing file")?;
        let uri = tsserver_file_to_uri(file).context("invalid rename file uri")?;
        let spans = entry
            .get("locs")
            .and_then(|value| value.as_array())
            .cloned()
            .unwrap_or_default();
        let bucket = map.entry(uri).or_default();
        for span in spans {
            if let Some(range) = rename_span_to_range(&span) {
                bucket.push(TextEdit {
                    range,
                    new_text: ctx.new_text.clone(),
                });
            }
        }
    }

    let edit = WorkspaceEdit {
        changes: if map.is_empty() { None } else { Some(map) },
        document_changes: None,
        change_annotations: None,
    };
    Ok(serde_json::to_value(edit)?)
}

fn rename_span_to_range(value: &Value) -> Option<lsp_types::Range> {
    if let Some(span) = value.get("textSpan") {
        return tsserver_range_from_value_lsp(span);
    }
    if value.get("start").is_some() && value.get("end").is_some() {
        return tsserver_range_from_value_lsp(&json!({
            "start": value.get("start"),
            "end": value.get("end"),
        }));
    }
    None
}
