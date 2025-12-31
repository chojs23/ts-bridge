//! =============================================================================
//! textDocument/documentHighlight
//! =============================================================================
//!
//! Surfaces tsserver’s `documentHighlights` command so clients like Neovim can
//! show same-buffer highlight spans (read vs write).  Tsserver reports every
//! file touched by the symbol, but the LSP request expects highlights scoped to
//! the current document, so we filter on the originating file.

use anyhow::{Context, Result};
use lsp_types::{DocumentHighlight, DocumentHighlightKind, DocumentHighlightParams};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::protocol::{AdapterResult, RequestSpec};
use crate::rpc::{Priority, Route};
use crate::utils::{tsserver_range_from_value_lsp, uri_to_file_path};

const CMD_DOCUMENT_HIGHLIGHTS: &str = "documentHighlights";

#[derive(Debug, Deserialize)]
struct HighlightContext {
    file: String,
}

pub fn handle(params: DocumentHighlightParams) -> RequestSpec {
    let text_document = params.text_document_position_params.text_document;
    let position = params.text_document_position_params.position;
    let uri_string = text_document.uri.to_string();
    let file = uri_to_file_path(text_document.uri.as_str()).unwrap_or(uri_string);

    let request = json!({
        "command": CMD_DOCUMENT_HIGHLIGHTS,
        "arguments": {
            "file": file,
            "line": position.line + 1,
            "offset": position.character + 1,
        }
    });

    let context = json!({ "file": file });

    RequestSpec {
        route: Route::Syntax,
        payload: request,
        priority: Priority::Normal,
        on_response: Some(adapt_document_highlights),
        response_context: Some(context),
        work_done: None,
    }
}

fn adapt_document_highlights(payload: &Value, context: Option<&Value>) -> Result<AdapterResult> {
    let ctx: HighlightContext = serde_json::from_value(
        context
            .cloned()
            .context("missing documentHighlight context")?,
    )?;
    let items = payload
        .get("body")
        .context("tsserver documentHighlights missing body")?
        .as_array()
        .cloned()
        .unwrap_or_default();

    let mut highlights = Vec::new();
    for item in items {
        let file = item
            .get("file")
            .or_else(|| item.get("fileName"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if !file.is_empty() && file != ctx.file {
            continue;
        }
        let spans = item
            .get("highlightSpans")
            .and_then(|value| value.as_array())
            .cloned()
            .unwrap_or_default();
        for span in spans {
            if let Some(range) = tsserver_range_from_value_lsp(&span) {
                let kind = span
                    .get("kind")
                    .and_then(|value| value.as_str())
                    .and_then(highlight_kind_from_ts_kind);
                highlights.push(DocumentHighlight { range, kind });
            }
        }
    }

    Ok(AdapterResult::ready(serde_json::to_value(highlights)?))
}

fn highlight_kind_from_ts_kind(kind: &str) -> Option<DocumentHighlightKind> {
    match kind {
        "writtenReference" => Some(DocumentHighlightKind::WRITE),
        "definition" => Some(DocumentHighlightKind::WRITE),
        "reference" => Some(DocumentHighlightKind::READ),
        "none" => Some(DocumentHighlightKind::TEXT),
        _ => None,
    }
}
