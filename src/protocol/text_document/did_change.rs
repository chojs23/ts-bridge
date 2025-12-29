use std::path::Path;

use serde_json::json;

use crate::protocol::NotificationSpec;
use crate::rpc::{Priority, Route};
use crate::types::DidChangeTextDocumentParams;
use crate::utils::{tsserver_text_changes_from_edits, uri_to_file_path};

pub fn handle(params: DidChangeTextDocumentParams, workspace_root: &Path) -> NotificationSpec {
    let text_changes = tsserver_text_changes_from_edits(&params.content_changes);
    let file_name = uri_to_file_path(&params.text_document.uri)
        .unwrap_or_else(|| params.text_document.uri.clone());
    let request = json!({
        "command": "updateOpen",
        "arguments": {
            "projectRootPath": workspace_root.to_string_lossy(),
            "openFiles": [],
            "changedFiles": [{
                "fileName": file_name,
                "textChanges": text_changes,
            }],
            "closedFiles": [],
        }
    });

    NotificationSpec {
        route: Route::Both,
        payload: request,
        priority: Priority::Const,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{
        DidChangeTextDocumentParams, Position, Range, TextDocumentContentChangeEvent,
        VersionedTextDocumentIdentifier,
    };

    #[test]
    fn did_change_routes_to_both_servers() {
        let params = DidChangeTextDocumentParams {
            text_document: VersionedTextDocumentIdentifier {
                uri: "file:///workspace/foo.ts".into(),
                version: Some(2),
            },
            content_changes: vec![TextDocumentContentChangeEvent {
                range: Some(Range {
                    start: Position {
                        line: 0,
                        character: 0,
                    },
                    end: Position {
                        line: 0,
                        character: 3,
                    },
                }),
                text: "let".into(),
            }],
        };
        let root = Path::new("/workspace");
        let spec = handle(params, root);

        assert_eq!(spec.route, Route::Both);
        let file_name = spec
            .payload
            .get("arguments")
            .and_then(|args| args.get("changedFiles"))
            .and_then(|arr| arr.as_array())
            .and_then(|arr| arr.first())
            .and_then(|entry| entry.get("fileName"))
            .and_then(|value| value.as_str())
            .expect("missing changed file");
        assert_eq!(file_name, "/workspace/foo.ts");
    }
}
