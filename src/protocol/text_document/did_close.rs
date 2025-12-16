use std::path::Path;

use serde_json::json;

use crate::protocol::NotificationSpec;
use crate::rpc::{Priority, Route};
use crate::types::DidCloseTextDocumentParams;
use crate::utils::uri_to_file_path;

pub fn handle(params: DidCloseTextDocumentParams, workspace_root: &Path) -> NotificationSpec {
    let file = uri_to_file_path(&params.text_document.uri)
        .unwrap_or_else(|| params.text_document.uri.clone());
    let request = json!({
        "command": "updateOpen",
        "arguments": {
            "projectRootPath": workspace_root.to_string_lossy(),
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
