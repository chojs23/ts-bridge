use std::path::Path;

use serde_json::json;

use crate::protocol::RequestSpec;
use crate::rpc::{Priority, Route};
use crate::types::DidOpenTextDocumentParams;
use crate::utils::lsp_text_doc_to_tsserver_entry;

pub fn handle(params: DidOpenTextDocumentParams, workspace_root: &Path) -> RequestSpec {
    let entry = lsp_text_doc_to_tsserver_entry(&params.text_document, Some(workspace_root));
    let request = json!({
        "command": "updateOpen",
        "arguments": {
            "projectRootPath": workspace_root.to_string_lossy(),
            "openFiles": [entry],
            "changedFiles": [],
            "closedFiles": [],
        }
    });

    RequestSpec {
        route: Route::Syntax,
        payload: request,
        priority: Priority::Const,
        on_response: None,
        response_context: None,
    }
}
