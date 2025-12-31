//! =============================================================================
//! textDocument/hover
//! =============================================================================
//!
//! Translating the LSP request into a tsserver `quickinfo`
//! command and shaping the resulting response into an LSP
//! `Hover`. The handler keeps the formatting decisions (code fence for the
//! `displayString`, plain-text docs, and `_@tag_` renders) so the Neovim UX
//! matches the original plugin.

use anyhow::{Context, Result};
use lsp_types::{Hover, HoverContents, MarkupContent, MarkupKind};
use serde_json::{Value, json};

use crate::protocol::{AdapterResult, RequestSpec};
use crate::rpc::{Priority, Route};
use crate::utils::{tsserver_range_from_value_lsp, uri_to_file_path};

pub fn handle(params: lsp_types::HoverParams) -> RequestSpec {
    let text_document = params.text_document_position_params.text_document;
    let uri_string = text_document.uri.to_string();
    let file_name = uri_to_file_path(text_document.uri.as_str()).unwrap_or(uri_string);
    let position = params.text_document_position_params.position;
    let request = json!({
        "command": "quickinfo",
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
        on_response: Some(adapt_quickinfo),
        response_context: None,
    }
}

fn adapt_quickinfo(payload: &Value, _context: Option<&Value>) -> Result<AdapterResult> {
    let body = payload
        .get("body")
        .context("tsserver quickinfo missing body")?;
    let mut sections = Vec::new();

    if let Some(display) = body.get("displayString").and_then(|v| v.as_str()) {
        if !display.is_empty() {
            sections.push(format!("```typescript\n{}\n```", display));
        }
    }

    if let Some(documentation) = body
        .get("documentation")
        .and_then(|doc| flatten_symbol_display(doc, "", false).filter(|text| !text.is_empty()))
    {
        sections.push(documentation);
    }

    if let Some(tags) = body
        .get("tags")
        .and_then(|tags| render_tags(tags).filter(|text| !text.is_empty()))
    {
        sections.push(tags);
    }

    let hover = Hover {
        contents: HoverContents::Markup(MarkupContent {
            kind: MarkupKind::Markdown,
            value: sections.join("\n\n"),
        }),
        range: tsserver_range_from_value_lsp(body),
    };

    Ok(AdapterResult::ready(serde_json::to_value(hover)?))
}

fn flatten_symbol_display(
    value: &Value,
    delimiter: &str,
    format_parameter: bool,
) -> Option<String> {
    if let Some(s) = value.as_str() {
        return Some(s.to_string());
    }

    let parts = value.as_array()?;
    let mut buffer = Vec::new();
    for part in parts {
        let text = part.get("text").and_then(|v| v.as_str())?;
        if format_parameter && part.get("kind").and_then(|k| k.as_str()) == Some("parameterName") {
            buffer.push(format!("`{}`", text));
        } else {
            buffer.push(text.to_string());
        }
    }

    if buffer.is_empty() {
        None
    } else {
        Some(buffer.join(delimiter))
    }
}

fn render_tags(tags_value: &Value) -> Option<String> {
    let tags = tags_value.as_array()?;
    let mut lines = Vec::new();
    for tag in tags {
        let name = match tag.get("name").and_then(|v| v.as_str()) {
            Some(name) if !name.is_empty() => name,
            _ => continue,
        };

        let mut line = format!("_@{}_", name);
        if let Some(text_value) = tag.get("text") {
            if let Some(text) = flatten_symbol_display(text_value, "", true) {
                if !text.is_empty() {
                    line.push_str(" — ");
                    line.push_str(&text);
                }
            }
        }
        lines.push(line);
    }

    if lines.is_empty() {
        None
    } else {
        Some(lines.join("\n"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lsp_types::{
        Hover as LspHover, HoverParams, Position, TextDocumentIdentifier,
        TextDocumentPositionParams, Uri,
    };
    use std::str::FromStr;

    #[test]
    fn handle_builds_quickinfo_request() {
        let params = HoverParams {
            text_document_position_params: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier {
                    uri: Uri::from_str("file:///workspace/foo.ts").unwrap(),
                },
                position: Position {
                    line: 4,
                    character: 2,
                },
            },
            work_done_progress_params: Default::default(),
        };

        let spec = handle(params);
        assert_eq!(spec.route, Route::Syntax);
        assert_eq!(spec.priority, Priority::Normal);
        let args = spec.payload.get("arguments").expect("arguments missing");
        assert_eq!(
            args.get("file").and_then(|v| v.as_str()),
            Some("/workspace/foo.ts")
        );
        assert_eq!(args.get("line").and_then(|v| v.as_u64()), Some(5));
        assert_eq!(args.get("offset").and_then(|v| v.as_u64()), Some(3));
    }

    #[test]
    fn adapt_quickinfo_formats_markdown() {
        let payload = json!({
            "body": {
                "displayString": "const greet: () => void",
                "documentation": [{ "text": "Greets the user." }],
                "tags": [{
                    "name": "deprecated",
                    "text": [{ "text": "Use greetAsync instead." }]
                }],
                "start": { "line": 1, "offset": 1 },
                "end": { "line": 1, "offset": 6 }
            }
        });

        let hover_value = adapt_quickinfo(&payload, None).expect("hover should adapt");
        let hover: LspHover = serde_json::from_value(hover_value).expect("hover deserializes");
        let HoverContents::Markup(content) = hover.contents else {
            panic!("expected markup hover");
        };
        assert_eq!(
            content.value,
            "```typescript\nconst greet: () => void\n```\n\nGreets the user.\n\n_@deprecated_ — Use greetAsync instead."
        );
        let range = hover.range.expect("hover should include range");
        assert_eq!(range.start.line, 0);
        assert_eq!(range.start.character, 0);
        assert_eq!(range.end.line, 0);
        assert_eq!(range.end.character, 5);
    }
}
