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
        route: Route::Both,
        payload: request,
        priority: Priority::Const,
        on_response: None,
        response_context: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{DidOpenTextDocumentParams, TextDocumentItem};

    #[test]
    fn did_open_routes_to_both_servers() {
        let params = DidOpenTextDocumentParams {
            text_document: TextDocumentItem {
                uri: "file:///workspace/foo.ts".into(),
                language_id: Some("typescript".into()),
                version: 1,
                text: "const a = 1;".into(),
            },
        };
        let root = Path::new("/workspace");
        let spec = handle(params, root);

        assert_eq!(spec.route, Route::Both);
        let open_files = spec
            .payload
            .get("arguments")
            .and_then(|args| args.get("openFiles"))
            .and_then(|value| value.as_array())
            .expect("openFiles array missing");
        assert_eq!(
            open_files[0]
                .get("file")
                .and_then(|v| v.as_str())
                .expect("file path missing"),
            "/workspace/foo.ts"
        );
    }
}
