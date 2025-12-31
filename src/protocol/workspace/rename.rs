//! =============================================================================
//! workspace/willRenameFiles
//! =============================================================================
//!
//! Chains tsserver `getEditsForFileRename` requests for each file rename so
//! LSP clients can preview edits before they rename files on disk.

use std::collections::{HashMap, VecDeque};

use anyhow::{Context, Result};
use lsp_types::{FileRename, RenameFilesParams, WorkspaceEdit};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::protocol::text_document::code_action::workspace_edit_from_tsserver_changes;
use crate::protocol::{AdapterResult, RequestSpec};
use crate::rpc::{Priority, Route};
use crate::utils::uri_to_file_path;

pub fn handle(params: RenameFilesParams) -> Option<RequestSpec> {
    let mut queue: VecDeque<FileRenameSpec> =
        params.files.into_iter().map(FileRenameSpec::from).collect();
    let first = queue.pop_front()?;
    let context = serde_json::to_value(RenameChainState {
        pending: queue,
        accumulated: None,
    })
    .ok()?;

    Some(RequestSpec {
        route: Route::Syntax,
        payload: rename_payload(&first),
        priority: Priority::Low,
        on_response: Some(adapt_rename_chain),
        response_context: Some(context),
    })
}

fn adapt_rename_chain(payload: &Value, context: Option<&Value>) -> Result<AdapterResult> {
    let mut state: RenameChainState = serde_json::from_value(
        context
            .cloned()
            .context("missing willRenameFiles context")?,
    )?;
    let changes = payload
        .get("body")
        .and_then(|value| value.as_array())
        .cloned()
        .unwrap_or_default();
    if let Some(edit) = workspace_edit_from_tsserver_changes(&changes) {
        state.merge_edit(edit);
    }

    if let Some(next) = state.pending.pop_front() {
        let context = serde_json::to_value(&state)?;
        return Ok(AdapterResult::Continue(RequestSpec {
            route: Route::Syntax,
            payload: rename_payload(&next),
            priority: Priority::Low,
            on_response: Some(adapt_rename_chain),
            response_context: Some(context),
        }));
    }

    let result = state.accumulated.unwrap_or_else(empty_workspace_edit);
    Ok(AdapterResult::ready(serde_json::to_value(result)?))
}

fn rename_payload(spec: &FileRenameSpec) -> Value {
    Value::from(serde_json::json!({
        "command": "getEditsForFileRename",
        "arguments": {
            "oldFilePath": uri_to_file_path(&spec.old_uri).unwrap_or_else(|| spec.old_uri.clone()),
            "newFilePath": uri_to_file_path(&spec.new_uri).unwrap_or_else(|| spec.new_uri.clone()),
        }
    }))
}

fn empty_workspace_edit() -> WorkspaceEdit {
    WorkspaceEdit {
        changes: Some(HashMap::new()),
        document_changes: None,
        change_annotations: None,
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RenameChainState {
    pending: VecDeque<FileRenameSpec>,
    #[serde(default)]
    accumulated: Option<WorkspaceEdit>,
}

impl RenameChainState {
    fn merge_edit(&mut self, edit: WorkspaceEdit) {
        match &mut self.accumulated {
            Some(existing) => {
                let map = existing.changes.get_or_insert_with(HashMap::new);
                if let Some(changes) = edit.changes {
                    for (uri, mut edits) in changes {
                        map.entry(uri).or_default().append(&mut edits);
                    }
                }
            }
            None => self.accumulated = Some(edit),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct FileRenameSpec {
    old_uri: String,
    new_uri: String,
}

impl From<FileRename> for FileRenameSpec {
    fn from(rename: FileRename) -> Self {
        Self {
            old_uri: rename.old_uri.to_string(),
            new_uri: rename.new_uri.to_string(),
        }
    }
}
