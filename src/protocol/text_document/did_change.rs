use serde_json::json;

use crate::protocol::NotificationSpec;
use crate::rpc::{Priority, Route};
use crate::types::DidChangeTextDocumentParams;
use crate::utils::{tsserver_text_changes_from_edits, uri_to_file_path};

pub fn handle(params: DidChangeTextDocumentParams) -> NotificationSpec {
    let text_changes = tsserver_text_changes_from_edits(&params.content_changes);
    let file_name = uri_to_file_path(&params.text_document.uri)
        .unwrap_or_else(|| params.text_document.uri.clone());
    let request = json!({
        "command": "updateOpen",
        "arguments": {
            "openFiles": [],
            "changedFiles": [{
                "fileName": file_name,
                "textChanges": text_changes,
            }],
            "closedFiles": [],
        }
    });

    NotificationSpec {
        route: Route::Syntax,
        payload: request,
        priority: Priority::Const,
    }
}
