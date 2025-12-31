//! =============================================================================
//! textDocument/formatting
//! =============================================================================
//!
//! Proxies whole-document formatting requests to tsserver's `format` command.
//! We request the full file range and let tsserver provide the minimal text
//! edits, translating them into standard LSP `TextEdit`s.

use anyhow::{Context, Result};
use lsp_types::{DocumentFormattingParams, TextEdit};
use serde_json::{Value, json};

use crate::protocol::{AdapterResult, RequestSpec};
use crate::rpc::{Priority, Route};
use crate::utils::{tsserver_range_from_value_lsp, uri_to_file_path};

const CMD_FORMAT: &str = "format";

pub fn handle(params: DocumentFormattingParams) -> RequestSpec {
    let uri = params.text_document.uri;
    let file = uri_to_file_path(uri.as_str()).unwrap_or_else(|| uri.to_string());
    let options = format_code_settings(&params.options);

    let request = json!({
        "command": CMD_FORMAT,
        "arguments": {
            "file": file,
            "line": 1,
            "offset": 1,
            "endLine": 10_000_000,
            "endOffset": 1,
            "options": options,
        }
    });

    RequestSpec {
        route: Route::Syntax,
        payload: request,
        priority: Priority::Normal,
        on_response: Some(adapt_formatting),
        response_context: None,
    }
}

fn adapt_formatting(payload: &Value, _context: Option<&Value>) -> Result<AdapterResult> {
    let edits = payload
        .get("body")
        .context("tsserver format missing body")?
        .as_array()
        .cloned()
        .unwrap_or_default();

    let mut lsp_edits = Vec::with_capacity(edits.len());
    for edit in edits {
        let range = tsserver_range_from_value_lsp(&edit).context("format edit missing span")?;
        let new_text = edit
            .get("newText")
            .and_then(|value| value.as_str())
            .unwrap_or_default()
            .to_string();
        lsp_edits.push(TextEdit { range, new_text });
    }

    Ok(AdapterResult::ready(serde_json::to_value(lsp_edits)?))
}

fn format_code_settings(options: &lsp_types::FormattingOptions) -> Value {
    let mut payload = serde_json::Map::new();
    payload.insert(
        "convertTabsToSpaces".into(),
        Value::Bool(options.insert_spaces),
    );
    payload.insert("tabSize".into(), json!(options.tab_size));
    payload.insert("indentSize".into(), json!(options.tab_size));

    for (key, value) in &options.properties {
        let val = match value {
            lsp_types::FormattingProperty::Bool(b) => Value::Bool(*b),
            lsp_types::FormattingProperty::Number(n) => json!(*n),
            lsp_types::FormattingProperty::String(s) => Value::String(s.clone()),
        };
        payload.insert(key.clone(), val);
    }

    if let Some(val) = options.trim_trailing_whitespace {
        payload.insert("trimTrailingWhitespace".into(), Value::Bool(val));
    }
    if let Some(val) = options.trim_final_newlines {
        payload.insert("trimFinalNewlines".into(), Value::Bool(val));
    }
    if let Some(val) = options.insert_final_newline {
        payload.insert("insertFinalNewline".into(), Value::Bool(val));
    }

    Value::Object(payload)
}
