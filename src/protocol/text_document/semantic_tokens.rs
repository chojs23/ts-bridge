//! =============================================================================
//! textDocument/semanticTokens (full + range)
//! =============================================================================
//!
//! Uses tsserver's `semanticClassifications-full` command to fetch classified
//! spans and converts them into LSP semantic tokens (relative encoding).

use anyhow::{Context, Result};
use lsp_types::{
    SemanticToken, SemanticTokenModifier, SemanticTokenType, SemanticTokens, SemanticTokensLegend,
    SemanticTokensParams,
};
use serde_json::{Value, json};

use crate::protocol::{AdapterResult, RequestSpec};
use crate::rpc::{Priority, Route};
use crate::utils::tsserver_range_from_value_lsp;

const TOKEN_TYPES: &[&str] = &[
    "namespace",
    "type",
    "class",
    "interface",
    "enum",
    "enumMember",
    "typeParameter",
    "function",
    "method",
    "property",
    "variable",
    "parameter",
    "keyword",
    "string",
    "number",
];

const TOKEN_MODIFIERS: &[&str] = &[
    "declaration",
    "definition",
    "readonly",
    "static",
    "async",
    "abstract",
    "deprecated",
    "defaultLibrary",
];

pub fn legend() -> SemanticTokensLegend {
    SemanticTokensLegend {
        token_types: TOKEN_TYPES
            .iter()
            .map(|ty| SemanticTokenType::from(*ty))
            .collect(),
        token_modifiers: TOKEN_MODIFIERS
            .iter()
            .map(|m| SemanticTokenModifier::from(*m))
            .collect(),
    }
}

pub fn handle_full(params: SemanticTokensParams) -> RequestSpec {
    let uri = params.text_document.uri;
    let file = crate::utils::uri_to_file_path(uri.as_str()).unwrap_or_else(|| uri.to_string());

    let request = json!({
        "command": "encodedSemanticClassifications-full",
        "arguments": {
            "file": file,
            "format": "2020",
        }
    });

    RequestSpec {
        route: Route::Syntax,
        payload: request,
        priority: Priority::Low,
        on_response: Some(adapt_semantic_tokens),
        response_context: None,
        work_done: None,
    }
}

pub fn handle_range(params: lsp_types::SemanticTokensRangeParams) -> RequestSpec {
    let uri = params.text_document.uri;
    let file = crate::utils::uri_to_file_path(uri.as_str()).unwrap_or_else(|| uri.to_string());
    let range = params.range;

    let request = json!({
        "command": "encodedSemanticClassifications-full",
        "arguments": {
            "file": file,
            "format": "2020",
            "start": {
                "line": range.start.line + 1,
                "offset": range.start.character + 1,
            },
            "length": clamp_range_length(&range),
        }
    });

    RequestSpec {
        route: Route::Syntax,
        payload: request,
        priority: Priority::Low,
        on_response: Some(adapt_semantic_tokens),
        response_context: None,
        work_done: None,
    }
}

fn clamp_range_length(range: &lsp_types::Range) -> u32 {
    let lines = range.end.line.saturating_sub(range.start.line) + 1;
    lines.saturating_mul(10_000).min(5_000_000)
}

fn adapt_semantic_tokens(payload: &Value, _context: Option<&Value>) -> Result<AdapterResult> {
    let body = payload
        .get("body")
        .context("tsserver semanticClassifications missing body")?;
    let spans = body
        .get("spans")
        .and_then(|value| value.as_array())
        .cloned()
        .unwrap_or_default();

    let mut tokens = Vec::new();
    for span in spans {
        let classification = span
            .get("classificationType")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let Some(token_type) = token_type_index(classification) else {
            continue;
        };
        let modifiers_mask = modifier_mask(
            span.get("classificationModifier")
                .or_else(|| span.get("classificationModifiers")),
        );
        let range = match span.get("textSpan").and_then(tsserver_range_from_value_lsp) {
            Some(range) => range,
            None => continue,
        };
        if range.start.line != range.end.line {
            continue;
        }
        let length = range.end.character.saturating_sub(range.start.character);
        if length == 0 {
            continue;
        }
        tokens.push(SemanticTokenRow {
            line: range.start.line,
            start: range.start.character,
            length,
            token_type,
            modifiers: modifiers_mask,
        });
    }

    tokens.sort_by(|a, b| match a.line.cmp(&b.line) {
        std::cmp::Ordering::Equal => a.start.cmp(&b.start),
        other => other,
    });

    let mut data = Vec::with_capacity(tokens.len());
    let mut prev_line = 0;
    let mut prev_start = 0;
    for token in tokens {
        let delta_line = token.line - prev_line;
        let delta_start = if delta_line == 0 {
            token.start.saturating_sub(prev_start)
        } else {
            token.start
        };
        data.push(SemanticToken {
            delta_line,
            delta_start,
            length: token.length,
            token_type: token.token_type,
            token_modifiers_bitset: token.modifiers,
        });
        prev_line = token.line;
        prev_start = token.start;
    }

    let response = SemanticTokens {
        result_id: None,
        data,
    };
    Ok(AdapterResult::ready(serde_json::to_value(response)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use lsp_types::{Position, Range, SemanticTokensParams, TextDocumentIdentifier, Uri};
    use std::str::FromStr;

    fn uri() -> Uri {
        Uri::from_str("file:///workspace/src/lib.ts").unwrap()
    }

    #[test]
    fn handle_full_builds_request() {
        let params = SemanticTokensParams {
            text_document: TextDocumentIdentifier { uri: uri() },
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        };
        let spec = handle_full(params);
        assert_eq!(spec.route, Route::Syntax);
        assert_eq!(spec.priority, Priority::Low);
        assert_eq!(
            spec.payload.get("command"),
            Some(&json!("encodedSemanticClassifications-full"))
        );
        let args = spec.payload.get("arguments").expect("arguments missing");
        assert_eq!(
            args.get("file").and_then(|v| v.as_str()),
            Some("/workspace/src/lib.ts")
        );
        assert_eq!(args.get("format").and_then(|v| v.as_str()), Some("2020"));
        assert!(
            args.get("length").is_none(),
            "full requests should not set range length"
        );
    }

    #[test]
    fn handle_range_builds_request_with_offsets() {
        let params = lsp_types::SemanticTokensRangeParams {
            text_document: TextDocumentIdentifier { uri: uri() },
            range: Range {
                start: Position {
                    line: 5,
                    character: 2,
                },
                end: Position {
                    line: 7,
                    character: 0,
                },
            },
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        };
        let spec = handle_range(params);
        assert_eq!(spec.route, Route::Syntax);
        let args = spec.payload.get("arguments").expect("arguments missing");
        assert_eq!(
            args.get("start")
                .and_then(|s| s.get("line"))
                .and_then(|v| v.as_u64()),
            Some(6)
        );
        assert_eq!(
            args.get("start")
                .and_then(|s| s.get("offset"))
                .and_then(|v| v.as_u64()),
            Some(3)
        );
        assert!(args.get("length").and_then(|v| v.as_u64()).is_some());
    }

    #[test]
    fn adapt_semantic_tokens_converts_spans() {
        let payload = json!({
            "body": {
                "spans": [{
                    "classificationType": "class",
                    "classificationModifier": "declaration",
                    "textSpan": {
                        "start": { "line": 1, "offset": 1 },
                        "end": { "line": 1, "offset": 5 }
                    }
                }, {
                    "classificationType": "method",
                    "classificationModifiers": "static",
                    "textSpan": {
                        "start": { "line": 2, "offset": 3 },
                        "end": { "line": 2, "offset": 7 }
                    }
                }]
            }
        });

        let value = adapt_semantic_tokens(&payload, None).expect("semantic tokens adapt");
        let tokens: SemanticTokens =
            serde_json::from_value(value).expect("semantic tokens deserialize");
        assert_eq!(tokens.data.len(), 2);
        // Tokens should be sorted and use relative encoding
        assert_eq!(tokens.data[0].delta_line, 0);
        assert_eq!(tokens.data[0].delta_start, 0);
        assert!(tokens.data[0].token_type < TOKEN_TYPES.len() as u32);
    }
}

#[derive(Debug)]
struct SemanticTokenRow {
    line: u32,
    start: u32,
    length: u32,
    token_type: u32,
    modifiers: u32,
}

fn token_type_index(classification: &str) -> Option<u32> {
    let normalized = match classification {
        "module" | "namespace" => "namespace",
        "class" | "class name" | "local class name" => "class",
        "enum" | "enum name" | "local enum name" => "enum",
        "interface" | "interface name" => "interface",
        "type" | "type alias" => "type",
        "type parameter name" => "typeParameter",
        "enum member name" => "enumMember",
        "parameter" | "parameter name" => "parameter",
        "function" | "function name" => "function",
        "member function name" | "member accessor name" | "method" => "method",
        "property" | "property declaration" | "property name" | "member" => "property",
        "var" | "let" | "const" | "variable" | "variable name" | "local variable name" => {
            "variable"
        }
        "keyword" => "keyword",
        "string" | "string literal" => "string",
        "numeric literal" | "number" => "number",
        _ => return None,
    };
    TOKEN_TYPES
        .iter()
        .position(|ty| ty == &normalized)
        .map(|idx| idx as u32)
}

fn modifier_mask(raw: Option<&Value>) -> u32 {
    let mut mask = 0u32;
    let Some(text) = raw.and_then(|v| v.as_str()) else {
        return mask;
    };
    for modifier in text.split(|ch| ch == ',' || ch == ' ') {
        if modifier.is_empty() {
            continue;
        }
        let normalized = match modifier {
            "declare" | "declaration" => "declaration",
            "definition" => "definition",
            "readonly" => "readonly",
            "static" => "static",
            "async" => "async",
            "abstract" => "abstract",
            "deprecated" => "deprecated",
            "defaultLibrary" => "defaultLibrary",
            _ => continue,
        };
        if let Some(idx) = TOKEN_MODIFIERS.iter().position(|m| m == &normalized) {
            mask |= 1 << idx;
        }
    }
    mask
}
