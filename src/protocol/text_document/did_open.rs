use serde_json::json;

use crate::protocol::RequestSpec;
use crate::rpc::{Priority, Route};
use crate::types::DidOpenTextDocumentParams;
use crate::utils::lsp_text_doc_to_tsserver_entry;

pub fn handle(params: DidOpenTextDocumentParams) -> RequestSpec {
    let entry = lsp_text_doc_to_tsserver_entry(&params.text_document);
    let request = json!({
        "command": "updateOpen",
        "arguments": {
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
    }
}
