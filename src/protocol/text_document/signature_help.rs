//! =============================================================================
//! textDocument/signatureHelp
//! =============================================================================
//!
//! Calls tsserver’s `signatureHelp` command and reshapes the response into
//! LSP `SignatureHelp`, including documentation and parameter metadata.

use anyhow::{Context, Result};
use lsp_types::{
    Documentation, MarkupContent, MarkupKind, ParameterInformation, ParameterLabel, SignatureHelp,
    SignatureHelpContext, SignatureHelpParams, SignatureInformation,
};
use serde_json::{Value, json};

use crate::protocol::RequestSpec;
use crate::rpc::{Priority, Route};
use crate::utils::uri_to_file_path;

pub const TRIGGER_CHARACTERS: &[&str] = &["(", ",", "<"];

pub fn handle(params: SignatureHelpParams) -> RequestSpec {
    let text_document = params.text_document_position_params.text_document;
    let position = params.text_document_position_params.position;
    let uri_string = text_document.uri.to_string();
    let file_name = uri_to_file_path(text_document.uri.as_str()).unwrap_or(uri_string);

    let trigger_reason = params
        .context
        .as_ref()
        .and_then(|ctx| signature_trigger_reason(ctx));

    let mut arguments = json!({
        "file": file_name,
        "line": position.line + 1,
        "offset": position.character + 1,
    });
    if let Some(reason) = trigger_reason {
        arguments
            .as_object_mut()
            .unwrap()
            .insert("triggerReason".into(), reason);
    }

    let request = json!({
        "command": "signatureHelp",
        "arguments": arguments,
    });

    RequestSpec {
        route: Route::Syntax,
        payload: request,
        priority: Priority::Normal,
        on_response: Some(adapt_signature_help),
        response_context: None,
    }
}

fn signature_trigger_reason(context: &SignatureHelpContext) -> Option<Value> {
    use lsp_types::SignatureHelpTriggerKind;
    let kind = match context.trigger_kind {
        SignatureHelpTriggerKind::INVOKED => "invoked",
        SignatureHelpTriggerKind::TRIGGER_CHARACTER => {
            if context.is_retrigger {
                "retrigger"
            } else {
                "characterTyped"
            }
        }
        SignatureHelpTriggerKind::CONTENT_CHANGE => {
            if context.is_retrigger {
                "retrigger"
            } else {
                "characterTyped"
            }
        }
        _ => "invoked",
    };

    let mut obj = serde_json::Map::new();
    obj.insert("kind".into(), json!(kind));
    if let Some(ch) = &context.trigger_character {
        obj.insert("triggerCharacter".into(), json!(ch));
    }
    obj.insert("isRetrigger".into(), json!(context.is_retrigger));
    Some(Value::Object(obj))
}

fn adapt_signature_help(payload: &Value, _context: Option<&Value>) -> Result<Value> {
    let body = payload
        .get("body")
        .context("tsserver signatureHelp missing body")?;
    let items = body
        .get("items")
        .and_then(|value| value.as_array())
        .cloned()
        .unwrap_or_default();

    let signatures = items
        .into_iter()
        .filter_map(convert_signature)
        .collect::<Vec<_>>();

    let help = SignatureHelp {
        signatures,
        active_signature: body
            .get("selectedItemIndex")
            .and_then(|v| v.as_u64())
            .map(|v| v as u32),
        active_parameter: body
            .get("argumentIndex")
            .and_then(|v| v.as_u64())
            .map(|v| v as u32),
    };

    Ok(serde_json::to_value(help)?)
}

fn convert_signature(item: Value) -> Option<SignatureInformation> {
    let label = format_signature_label(
        item.get("prefixDisplayParts"),
        item.get("parameters"),
        item.get("suffixDisplayParts"),
    );

    let documentation = render_documentation(item.get("documentation"), item.get("tags"));

    let parameters = item
        .get("parameters")
        .and_then(|value| value.as_array())
        .map(|params| {
            params
                .iter()
                .filter_map(|param| {
                    let label =
                        display_parts_to_string(param.get("displayParts")).unwrap_or_default();
                    let documentation = render_documentation(param.get("documentation"), None);
                    Some(ParameterInformation {
                        label: ParameterLabel::Simple(label),
                        documentation: documentation.and_then(markdown_documentation),
                    })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    Some(SignatureInformation {
        label,
        documentation: documentation.and_then(markdown_documentation),
        parameters: if parameters.is_empty() {
            None
        } else {
            Some(parameters)
        },
        active_parameter: None,
    })
}

fn format_signature_label(
    prefix: Option<&Value>,
    parameters: Option<&Value>,
    suffix: Option<&Value>,
) -> String {
    let prefix = display_parts_to_string(prefix).unwrap_or_default();
    let suffix = display_parts_to_string(suffix).unwrap_or_default();
    let params = parameters
        .and_then(|value| value.as_array())
        .map(|params| {
            params
                .iter()
                .filter_map(|param| display_parts_to_string(param.get("displayParts")))
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default();

    format!("{prefix}{params}{suffix}")
}

fn display_parts_to_string(parts: Option<&Value>) -> Option<String> {
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

fn render_documentation(docs: Option<&Value>, tags: Option<&Value>) -> Option<String> {
    let mut sections = Vec::new();
    if let Some(parts) = docs.and_then(|value| value.as_array()) {
        let mut buf = String::new();
        for part in parts {
            if let Some(text) = part.get("text").and_then(|v| v.as_str()) {
                if !buf.is_empty() {
                    buf.push('\n');
                }
                buf.push_str(text);
            }
        }
        if !buf.is_empty() {
            sections.push(buf);
        }
    }
    if let Some(tags) = tags.and_then(|value| value.as_array()) {
        for tag in tags {
            if let Some(name) = tag.get("name").and_then(|v| v.as_str()) {
                let text = display_parts_to_string(tag.get("text")).unwrap_or_default();
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

fn markdown_documentation(doc: String) -> Option<Documentation> {
    if doc.is_empty() {
        None
    } else {
        Some(Documentation::MarkupContent(MarkupContent {
            kind: MarkupKind::Markdown,
            value: doc,
        }))
    }
}
