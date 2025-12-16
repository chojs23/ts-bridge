use serde_json::json;

use crate::protocol::NotificationSpec;
use crate::rpc::{Priority, Route};
use crate::types::DidCloseTextDocumentParams;
use crate::utils::uri_to_file_path;

pub fn handle(params: DidCloseTextDocumentParams) -> NotificationSpec {
    let file = uri_to_file_path(&params.text_document.uri)
        .unwrap_or_else(|| params.text_document.uri.clone());
    let request = json!({
        "command": "updateOpen",
        "arguments": {
            "openFiles": [],
            "changedFiles": [],
            "closedFiles": [file],
        }
    });

    NotificationSpec {
        route: Route::Syntax,
        payload: request,
        priority: Priority::Const,
    }
}
