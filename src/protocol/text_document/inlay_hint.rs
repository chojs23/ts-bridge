//! =============================================================================
//! textDocument/inlayHint
//! =============================================================================
//!
//! Bridges LSP inlay hint requests to tsserver’s `provideInlayHints` command,
//! translating the returned metadata into `InlayHint` entries (respecting label
//! text, padding flags, and kind tagging).

use anyhow::{Context, Result};
use lsp_types::{InlayHint, InlayHintKind, InlayHintLabel, InlayHintParams};
use serde_json::{Value, json};

use crate::documents::TextSpan;
use crate::protocol::RequestSpec;
use crate::rpc::{Priority, Route};
use crate::utils::{tsserver_position_value_lsp, uri_to_file_path};

const CMD_PROVIDE_INLAY_HINTS: &str = "provideInlayHints";

pub fn handle(params: InlayHintParams, span: TextSpan) -> RequestSpec {
    let uri = params.text_document.uri;
    let file_path = uri_to_file_path(uri.as_str()).unwrap_or_else(|| uri.to_string());

    let request = json!({
        "command": CMD_PROVIDE_INLAY_HINTS,
        "arguments": {
            "file": file_path,
            "start": span.start,
            "length": span.length,
        }
    });

    RequestSpec {
        route: Route::Syntax,
        payload: request,
        priority: Priority::Low,
        on_response: Some(adapt_inlay_hints),
        response_context: None,
    }
}

fn adapt_inlay_hints(payload: &Value, _context: Option<&Value>) -> Result<Value> {
    let entries = payload
        .get("body")
        .and_then(|value| value.as_array())
        .context("tsserver provideInlayHints missing body array")?;

    let mut hints = Vec::with_capacity(entries.len());
    for entry in entries {
        if let Some(hint) = convert_hint(entry) {
            hints.push(hint);
        }
    }

    Ok(serde_json::to_value(hints)?)
}

fn convert_hint(entry: &Value) -> Option<InlayHint> {
    let position = entry
        .get("position")
        .and_then(tsserver_position_value_lsp)?;
    let label = render_label(entry)?;
    let kind = match entry.get("kind").and_then(|value| value.as_str()) {
        Some("Type") => Some(InlayHintKind::TYPE),
        Some("Parameter") => Some(InlayHintKind::PARAMETER),
        _ => None,
    };
    let padding_left = entry
        .get("whitespaceBefore")
        .and_then(|value| value.as_bool());
    let padding_right = entry
        .get("whitespaceAfter")
        .and_then(|value| value.as_bool());

    Some(InlayHint {
        position,
        label,
        kind,
        text_edits: None,
        tooltip: None,
        padding_left,
        padding_right,
        data: None,
    })
}

fn render_label(value: &Value) -> Option<InlayHintLabel> {
    if let Some(text) = value.get("text").and_then(|v| v.as_str()) {
        if !text.is_empty() {
            return Some(InlayHintLabel::String(text.to_string()));
        }
    }
    if let Some(parts) = value.get("displayParts").and_then(|v| v.as_array()) {
        let mut buffer = String::new();
        for part in parts {
            if let Some(text) = part.get("text").and_then(|v| v.as_str()) {
                buffer.push_str(text);
            }
        }
        if !buffer.is_empty() {
            return Some(InlayHintLabel::String(buffer));
        }
    }
    None
}

/// Builds the TypeScript `UserPreferences` slice we forward through the `configure`
/// command. Toggling inlay hints off funnels every boolean switch to `false`
/// (or `"none"` for enums) so tsserver stops emitting hint payloads entirely.
pub fn preferences(enabled: bool) -> Value {
    if enabled {
        json!({
            "includeInlayParameterNameHints": "literals",
            "includeInlayParameterNameHintsWhenArgumentMatchesName": false,
            "includeInlayFunctionParameterTypeHints": true,
            "includeInlayVariableTypeHints": true,
            "includeInlayPropertyDeclarationTypeHints": true,
            "includeInlayFunctionLikeReturnTypeHints": true,
            "includeInlayEnumMemberValueHints": true,
        })
    } else {
        json!({
            "includeInlayParameterNameHints": "none",
            "includeInlayParameterNameHintsWhenArgumentMatchesName": false,
            "includeInlayFunctionParameterTypeHints": false,
            "includeInlayVariableTypeHints": false,
            "includeInlayPropertyDeclarationTypeHints": false,
            "includeInlayFunctionLikeReturnTypeHints": false,
            "includeInlayEnumMemberValueHints": false,
        })
    }
}
