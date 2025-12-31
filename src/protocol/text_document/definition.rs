//! =============================================================================
//! textDocument/definition
//! =============================================================================
//!
//! Tsserver powers definition requests through the `definitionAndBoundSpan`
//! command (plus `findSourceDefinition` for source preference).
//! Converting each returned `FileSpanWithContext`
//! into an LSP `LocationLink` so the client can show peek-definition previews
//! with context.

use anyhow::{Context, Result};
use lsp_types::{GotoDefinitionParams, GotoDefinitionResponse};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::protocol::{AdapterResult, RequestSpec};
use crate::rpc::{Priority, Route};
use crate::utils::{
    tsserver_range_from_value_lsp, tsserver_span_to_location_link, uri_to_file_path,
};

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
#[serde(rename_all = "camelCase")]
pub struct DefinitionContext {
    pub source_definition: Option<bool>,
}

pub fn handle(params: DefinitionParams) -> RequestSpec {
    let text_document = params.base.text_document_position_params.text_document;
    let uri_string = text_document.uri.to_string();
    let file_name = uri_to_file_path(text_document.uri.as_str()).unwrap_or(uri_string);

    let position = params.base.text_document_position_params.position;
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
        response_context: None,
        work_done: None,
    }
}

fn adapt_definition(payload: &Value, _context: Option<&Value>) -> Result<AdapterResult> {
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
        if let Some(link) = tsserver_span_to_location_link(def, origin_selection.clone()) {
            links.push(link);
        }
    }

    let response = GotoDefinitionResponse::Link(links);
    Ok(AdapterResult::ready(serde_json::to_value(response)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use lsp_types::{
        GotoDefinitionParams, GotoDefinitionResponse, LocationLink, Position,
        TextDocumentIdentifier, TextDocumentPositionParams, Uri,
    };
    use std::str::FromStr;

    fn params_with_context(source_definition: bool) -> DefinitionParams {
        let base = GotoDefinitionParams {
            text_document_position_params: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier {
                    uri: Uri::from_str("file:///workspace/app.ts").unwrap(),
                },
                position: Position {
                    line: 2,
                    character: 10,
                },
            },
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        };
        DefinitionParams {
            base,
            context: Some(DefinitionContext {
                source_definition: Some(source_definition),
            }),
        }
    }

    #[test]
    fn handle_builds_definition_request() {
        let params = DefinitionParams {
            base: GotoDefinitionParams {
                text_document_position_params: TextDocumentPositionParams {
                    text_document: TextDocumentIdentifier {
                        uri: Uri::from_str("file:///workspace/app.ts").unwrap(),
                    },
                    position: Position {
                        line: 1,
                        character: 4,
                    },
                },
                work_done_progress_params: Default::default(),
                partial_result_params: Default::default(),
            },
            context: None,
        };
        let spec = handle(params);
        assert_eq!(spec.route, Route::Syntax);
        assert_eq!(spec.priority, Priority::Normal);
        assert_eq!(spec.payload.get("command"), Some(&json!(CMD_DEFINITION)));
        let args = spec.payload.get("arguments").expect("missing args");
        assert_eq!(
            args.get("file").and_then(|v| v.as_str()),
            Some("/workspace/app.ts")
        );
        assert_eq!(args.get("line").and_then(|v| v.as_u64()), Some(2));
        assert_eq!(args.get("offset").and_then(|v| v.as_u64()), Some(5));
    }

    #[test]
    fn handle_uses_source_definition_command_when_context_requests_it() {
        let spec = handle(params_with_context(true));
        assert_eq!(
            spec.payload.get("command"),
            Some(&json!(CMD_SOURCE_DEFINITION))
        );
    }

    #[test]
    fn source_definition_flag_deserializes_from_camel_case_context() {
        let raw = json!({
            "textDocument": { "uri": "file:///workspace/app.ts" },
            "position": { "line": 4, "character": 2 },
            "context": { "sourceDefinition": true }
        });
        let params: DefinitionParams =
            serde_json::from_value(raw).expect("definition params should deserialize");
        let spec = handle(params);
        assert_eq!(
            spec.payload.get("command"),
            Some(&json!(CMD_SOURCE_DEFINITION))
        );
    }

    #[test]
    fn adapt_definition_converts_standard_payload() {
        let payload = json!({
            "command": CMD_DEFINITION,
            "body": {
                "textSpan": {
                    "start": { "line": 3, "offset": 1 },
                    "end": { "line": 3, "offset": 6 }
                },
                "definitions": [{
                    "file": "/workspace/foo.ts",
                    "start": { "line": 10, "offset": 2 },
                    "end": { "line": 10, "offset": 12 },
                    "contextStart": { "line": 9, "offset": 1 },
                    "contextEnd": { "line": 11, "offset": 1 }
                }]
            }
        });

        let value = adapt_definition(&payload, None).expect("definition should adapt");
        match serde_json::from_value::<GotoDefinitionResponse>(value)
            .expect("response should deserialize")
        {
            GotoDefinitionResponse::Link(links) => {
                assert_eq!(links.len(), 1);
                let LocationLink {
                    target_uri,
                    target_selection_range,
                    target_range,
                    origin_selection_range,
                } = &links[0];
                assert_eq!(target_uri.to_string(), "file:///workspace/foo.ts");
                assert_eq!(target_selection_range.start.line, 9);
                assert_eq!(target_selection_range.start.character, 1);
                assert_eq!(target_range.start.line, 8);
                assert_eq!(
                    origin_selection_range.as_ref().map(|r| r.start.line),
                    Some(2)
                );
            }
            _ => panic!("expected link response"),
        }
    }

    #[test]
    fn adapt_definition_handles_source_definition_shape() {
        let payload = json!({
            "command": CMD_SOURCE_DEFINITION,
            "body": [{
                "file": "/workspace/src.ts",
                "start": { "line": 5, "offset": 3 },
                "end": { "line": 5, "offset": 7 }
            }]
        });
        let value = adapt_definition(&payload, None).expect("source definition adapts");
        match serde_json::from_value::<GotoDefinitionResponse>(value)
            .expect("response should deserialize")
        {
            GotoDefinitionResponse::Link(links) => {
                assert_eq!(links.len(), 1);
                let LocationLink {
                    target_uri,
                    origin_selection_range,
                    ..
                } = &links[0];
                assert_eq!(target_uri.to_string(), "file:///workspace/src.ts");
                assert!(origin_selection_range.is_none());
            }
            _ => panic!("expected link response"),
        }
    }
}
