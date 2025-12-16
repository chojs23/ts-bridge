use serde_json::json;

use crate::protocol::NotificationSpec;
use crate::rpc::{Priority, Route};
use crate::types::DidCloseTextDocumentParams;

pub fn handle(params: DidCloseTextDocumentParams) -> NotificationSpec {
    let request = json!({
        "command": "updateOpen",
        "arguments": {
            "openFiles": [],
            "changedFiles": [],
            "closedFiles": [params.text_document.uri],
        }
    });

    NotificationSpec {
        route: Route::Syntax,
        payload: request,
        priority: Priority::Const,
    }
}
