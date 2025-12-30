//! =============================================================================
//! completionItem/resolve
//! =============================================================================
//!
//! Enriches completion items by calling tsserver’s `completionEntryDetails`
//! command. The handler expects items produced by our completion adapter so it
//! can reuse the stored metadata (`data.file`, `data.position`,
//! `data.entryNames`).

use anyhow::{Context, Result};
use lsp_types::{
    CompletionItem, CompletionItemKind, CompletionTextEdit, Documentation, InsertTextFormat,
    MarkupContent, MarkupKind, Position, TextEdit,
};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::protocol::RequestSpec;
use crate::rpc::{Priority, Route};
use crate::utils::tsserver_range_from_value_lsp;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CompletionResolveData {
    file: String,
    position: Position,
    #[serde(default)]
    entry_names: Vec<Value>,
}

pub fn handle(mut item: CompletionItem) -> Option<RequestSpec> {
    let data = item.data.take()?;
    let data: CompletionResolveData = serde_json::from_value(data).ok()?;
    let request = json!({
        "command": "completionEntryDetails",
        "arguments": {
            "file": data.file,
            "line": data.position.line + 1,
            "offset": data.position.character + 1,
            "entryNames": data.entry_names,
        }
    });

    let context = serde_json::to_value(item).ok()?;

    Some(RequestSpec {
        route: Route::Syntax,
        payload: request,
        priority: Priority::Normal,
        on_response: Some(adapt_completion_resolve),
        response_context: Some(context),
    })
}

fn adapt_completion_resolve(payload: &Value, context: Option<&Value>) -> Result<Value> {
    let mut item: CompletionItem =
        serde_json::from_value(context.cloned().context("missing completion item")?)?;
    let details = payload
        .get("body")
        .and_then(|value| value.as_array())
        .and_then(|array| array.first())
        .context("tsserver completion details missing body")?;

    if let Some(display) = render_display_parts(details.get("displayParts")) {
        item.detail = Some(display);
    }

    if let Some(documentation) = render_documentation(details) {
        item.documentation = Some(Documentation::MarkupContent(MarkupContent {
            kind: MarkupKind::Markdown,
            value: documentation,
        }));
    }

    if let Some(edits) = build_additional_text_edits(details.get("codeActions")) {
        item.additional_text_edits = Some(edits);
    }

    if should_create_function_snippet(&item, details) {
        inject_snippet(&mut item, details);
    }

    Ok(serde_json::to_value(item)?)
}

fn render_display_parts(parts: Option<&Value>) -> Option<String> {
    let parts = parts?.as_array()?;
    let mut buffer = String::new();
    for part in parts {
        if let Some(text) = part.get("text").and_then(|v| v.as_str()) {
            buffer.push_str(text);
        }
    }
    if buffer.is_empty() {
        None
    } else {
        Some(buffer)
    }
}

fn render_documentation(details: &Value) -> Option<String> {
    let mut sections = Vec::new();
    if let Some(docs) = details
        .get("documentation")
        .and_then(|value| value.as_array())
    {
        let mut buffer = String::new();
        for entry in docs {
            if let Some(text) = entry.get("text").and_then(|v| v.as_str()) {
                if !buffer.is_empty() {
                    buffer.push('\n');
                }
                buffer.push_str(text);
            }
        }
        if !buffer.is_empty() {
            sections.push(buffer);
        }
    }
    if let Some(tags) = details.get("tags").and_then(|value| value.as_array()) {
        for tag in tags {
            if let Some(name) = tag.get("name").and_then(|v| v.as_str()) {
                let text = tag
                    .get("text")
                    .and_then(|t| render_display_parts(Some(t)))
                    .unwrap_or_default();
                if text.is_empty() {
                    sections.push(format!("_@{}_", name));
                } else {
                    sections.push(format!("_@{}_ — {}", name, text));
                }
            }
        }
    }
    if sections.is_empty() {
        None
    } else {
        Some(sections.join("\n\n"))
    }
}

fn build_additional_text_edits(actions_value: Option<&Value>) -> Option<Vec<TextEdit>> {
    let actions = actions_value?.as_array()?;
    let mut edits = Vec::new();
    for action in actions {
        let changes = action.get("changes").and_then(|value| value.as_array());
        if let Some(changes) = changes {
            for change in changes {
                if let Some(text_changes) = change.get("textChanges").and_then(|v| v.as_array()) {
                    for text_change in text_changes {
                        if let Some(range) = tsserver_range_from_value_lsp(text_change) {
                            if let Some(new_text) =
                                text_change.get("newText").and_then(|v| v.as_str())
                            {
                                edits.push(TextEdit {
                                    range,
                                    new_text: new_text.to_string(),
                                });
                            }
                        }
                    }
                }
            }
        }
    }
    if edits.is_empty() { None } else { Some(edits) }
}

fn should_create_function_snippet(item: &CompletionItem, details: &Value) -> bool {
    matches!(
        item.kind.unwrap_or(CompletionItemKind::TEXT),
        CompletionItemKind::FUNCTION | CompletionItemKind::METHOD | CompletionItemKind::CONSTRUCTOR
    ) && details.get("displayParts").is_some()
}

fn inject_snippet(item: &mut CompletionItem, details: &Value) {
    if let Some(parts) = details
        .get("displayParts")
        .and_then(|value| value.as_array())
    {
        let mut snippet = String::new();
        snippet.push_str(
            item.insert_text
                .as_deref()
                .or_else(|| {
                    item.text_edit.as_ref().map(|edit| match edit {
                        CompletionTextEdit::Edit(edit) => edit.new_text.as_str(),
                        CompletionTextEdit::InsertAndReplace(edit) => edit.new_text.as_str(),
                    })
                })
                .unwrap_or(item.label.as_str()),
        );
        snippet.push('(');

        let mut param_index = 1;
        let mut first = true;
        for part in parts {
            let kind = part.get("kind").and_then(|v| v.as_str());
            if kind == Some("parameterName") {
                if let Some(text) = part.get("text").and_then(|v| v.as_str()) {
                    if !first {
                        snippet.push_str(", ");
                    }
                    snippet.push_str(&format!("${{{}:{}}}", param_index, escape_snippet(text)));
                    param_index += 1;
                    first = false;
                }
            } else if kind == Some("punctuation") {
                if let Some(text) = part.get("text").and_then(|v| v.as_str()) {
                    if text == ")" {
                        break;
                    }
                }
            }
        }

        if first {
            snippet.push_str("$0");
        }
        snippet.push(')');

        item.insert_text = Some(snippet.clone());
        item.insert_text_format = Some(InsertTextFormat::SNIPPET);
        if let Some(CompletionTextEdit::Edit(edit)) = item.text_edit.as_mut() {
            edit.new_text = snippet;
        }
    }
}

fn escape_snippet(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('$', "\\$")
        .replace('}', "\\}")
}
