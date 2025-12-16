use serde_json::json;

use crate::protocol::NotificationSpec;
use crate::rpc::{Priority, Route};
use crate::types::DidChangeTextDocumentParams;
use crate::utils::tsserver_text_changes_from_edits;

pub fn handle(params: DidChangeTextDocumentParams) -> NotificationSpec {
    let text_changes = tsserver_text_changes_from_edits(&params.content_changes);
    let request = json!({
        "command": "updateOpen",
        "arguments": {
            "openFiles": [],
            "changedFiles": [{
                "fileName": params.text_document.uri,
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
