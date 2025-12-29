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
        route: Route::Both,
        payload: request,
        priority: Priority::Const,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{DidCloseTextDocumentParams, TextDocumentIdentifier};

    #[test]
    fn did_close_routes_to_both_servers() {
        let params = DidCloseTextDocumentParams {
            text_document: TextDocumentIdentifier {
                uri: "file:///workspace/foo.ts".into(),
            },
        };
        let root = Path::new("/workspace");
        let spec = handle(params, root);

        assert_eq!(spec.route, Route::Both);
        let closed = spec
            .payload
            .get("arguments")
            .and_then(|args| args.get("closedFiles"))
            .and_then(|value| value.as_array())
            .and_then(|arr| arr.first())
            .and_then(|value| value.as_str())
            .expect("closed file missing");
        assert_eq!(closed, "/workspace/foo.ts");
    }
}
