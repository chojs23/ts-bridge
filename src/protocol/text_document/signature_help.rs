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

#[cfg(test)]
mod tests {
    use super::*;
    use lsp_types::{
        SignatureHelp as LspSignatureHelp, SignatureHelpTriggerKind, TextDocumentIdentifier,
        TextDocumentPositionParams, Uri,
    };
    use std::str::FromStr;

    fn params_with_context(
        trigger: SignatureHelpTriggerKind,
        is_retrigger: bool,
    ) -> SignatureHelpParams {
        SignatureHelpParams {
            text_document_position_params: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier {
                    uri: Uri::from_str("file:///workspace/main.ts").unwrap(),
                },
                position: lsp_types::Position {
                    line: 3,
                    character: 8,
                },
            },
            context: Some(SignatureHelpContext {
                trigger_kind: trigger,
                trigger_character: Some("(".into()),
                is_retrigger,
                active_signature_help: None,
            }),
            work_done_progress_params: Default::default(),
        }
    }

    #[test]
    fn handle_builds_signature_help_request_with_trigger_reason() {
        let params = params_with_context(SignatureHelpTriggerKind::TRIGGER_CHARACTER, false);
        let spec = handle(params);
        assert_eq!(spec.route, Route::Syntax);
        assert_eq!(spec.priority, Priority::Normal);
        assert_eq!(spec.payload.get("command"), Some(&json!("signatureHelp")));
        let args = spec.payload.get("arguments").expect("arguments missing");
        assert_eq!(
            args.get("file").and_then(|v| v.as_str()),
            Some("/workspace/main.ts")
        );
        assert_eq!(args.get("line").and_then(|v| v.as_u64()), Some(4));
        assert_eq!(args.get("offset").and_then(|v| v.as_u64()), Some(9));
        let trigger_reason = args.get("triggerReason").expect("trigger reason missing");
        assert_eq!(
            trigger_reason.get("kind").and_then(|v| v.as_str()),
            Some("characterTyped")
        );
        assert_eq!(
            trigger_reason
                .get("triggerCharacter")
                .and_then(|v| v.as_str()),
            Some("(")
        );
        assert_eq!(
            trigger_reason.get("isRetrigger").and_then(|v| v.as_bool()),
            Some(false)
        );
    }

    #[test]
    fn adapt_signature_help_converts_items() {
        let payload = json!({
            "body": {
                "selectedItemIndex": 1,
                "argumentIndex": 0,
                "items": [{
                    "prefixDisplayParts": [{ "text": "function foo" }, { "text": "(" }],
                    "parameters": [{
                        "displayParts": [{ "text": "bar: string" }],
                        "documentation": [{ "text": "name" }]
                    }],
                    "suffixDisplayParts": [{ "text": ")" }],
                    "documentation": [{ "text": "Docs" }],
                    "tags": [{
                        "name": "deprecated",
                        "text": [{ "text": "use other" }]
                    }]
                }]
            }
        });

        let value = adapt_signature_help(&payload, None).expect("signature help adapts");
        let parsed: LspSignatureHelp =
            serde_json::from_value(value).expect("signature help deserializes");
        assert_eq!(parsed.signatures.len(), 1);
        let sig = &parsed.signatures[0];
        assert_eq!(sig.label, "function foo(bar: string)");
        assert!(sig.documentation.is_some());
        let params = sig.parameters.as_ref().expect("parameters present");
        assert_eq!(params.len(), 1);
        assert_eq!(
            params[0].label,
            ParameterLabel::Simple("bar: string".into())
        );
        assert_eq!(parsed.active_signature, Some(1));
        assert_eq!(parsed.active_parameter, Some(0));
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
