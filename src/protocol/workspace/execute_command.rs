//! =============================================================================
//! workspace/executeCommand
//! =============================================================================
//!
//! Mirrors the user-facing commands provided by typescript-tools so Neovim user
//! commands (e.g. :TSBOrganizeImports) can be wired through
//! `workspace/executeCommand`. Each command maps to a concrete tsserver request
//! and returns ready-to-apply workspace edits or locations.

use std::collections::{HashMap, VecDeque};

use anyhow::{Context, Result};
use lsp_types::{
    ExecuteCommandParams, FileRename, GotoDefinitionParams, TextDocumentIdentifier,
    TextDocumentPositionParams, Uri, WorkspaceEdit,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::protocol::text_document::code_action::workspace_edit_from_tsserver_changes;
use crate::protocol::text_document::definition::{self, DefinitionContext, DefinitionParams};
use crate::protocol::{AdapterResult, RequestSpec};
use crate::rpc::{Priority, Route};
use crate::utils::{tsserver_span_to_location, uri_to_file_path};

const ORGANIZE_MODE_ALL: &str = "All";
const ORGANIZE_MODE_SORT_AND_COMBINE: &str = "SortAndCombine";
const ORGANIZE_MODE_REMOVE_UNUSED: &str = "RemoveUnused";

const FIX_UNUSED_IDENTIFIER: &str = "unusedIdentifier_delete";
const FIX_MISSING_IMPORT: &str = "fixMissingImport";
const FIX_ALL_CHAIN: &[&str] = &[
    "fixClassIncorrectlyImplementsInterface",
    "fixAwaitInSyncFunction",
    "fixUnreachableCode",
];

pub const USER_COMMANDS: &[&str] = &[
    "TSBOrganizeImports",
    "TSBSortImports",
    "TSBRemoveUnusedImports",
    "TSBRemoveUnused",
    "TSBAddMissingImports",
    "TSBFixAll",
    "TSBGoToSourceDefinition",
    "TSBRenameFile",
    "TSBFileReferences",
    "TSBRestartProject",
];

pub fn handle(params: ExecuteCommandParams) -> Option<RequestSpec> {
    let args = params.arguments;
    match params.command.as_str() {
        "TSBOrganizeImports" => organize_imports_command(&args, ORGANIZE_MODE_ALL),
        "TSBSortImports" => organize_imports_command(&args, ORGANIZE_MODE_SORT_AND_COMBINE),
        "TSBRemoveUnusedImports" => organize_imports_command(&args, ORGANIZE_MODE_REMOVE_UNUSED),
        "TSBRemoveUnused" => combined_code_fix_command(&args, FIX_UNUSED_IDENTIFIER),
        "TSBAddMissingImports" => combined_code_fix_command(&args, FIX_MISSING_IMPORT),
        "TSBFixAll" => fix_all_command(&args),
        "TSBGoToSourceDefinition" => goto_source_definition_command(&args),
        "TSBRenameFile" => rename_file_command(&args),
        "TSBFileReferences" => file_references_command(&args),
        _ => None,
    }
}

fn organize_imports_command(args: &[Value], mode: &str) -> Option<RequestSpec> {
    let target = parse_file_target(args)?;
    let request = json!({
        "command": "organizeImports",
        "arguments": {
            "scope": {
                "type": "file",
                "args": { "file": target.file },
            },
            "mode": mode,
        }
    });
    Some(RequestSpec {
        route: Route::Syntax,
        payload: request,
        priority: Priority::Low,
        on_response: Some(adapt_file_code_edits),
        response_context: None,
    })
}

fn combined_code_fix_command(args: &[Value], fix_id: &str) -> Option<RequestSpec> {
    let target = parse_file_target(args)?;
    Some(RequestSpec {
        route: Route::Syntax,
        payload: combined_code_fix_payload(&target.file, fix_id),
        priority: Priority::Low,
        on_response: Some(adapt_combined_code_fix),
        response_context: None,
    })
}

fn fix_all_command(args: &[Value]) -> Option<RequestSpec> {
    let target = parse_file_target(args)?;
    let mut pending: VecDeque<String> = FIX_ALL_CHAIN.iter().map(|id| id.to_string()).collect();
    let current = pending.pop_front()?;
    let context = serde_json::to_value(FixAllContext {
        file: target.file.clone(),
        pending_fix_ids: pending,
        accumulated: None,
    })
    .ok()?;

    Some(RequestSpec {
        route: Route::Syntax,
        payload: combined_code_fix_payload(&target.file, &current),
        priority: Priority::Low,
        on_response: Some(adapt_fix_all_chain),
        response_context: Some(context),
    })
}

fn goto_source_definition_command(args: &[Value]) -> Option<RequestSpec> {
    let params: TextDocumentPositionParams = serde_json::from_value(args.first()?.clone()).ok()?;
    let goto = GotoDefinitionParams {
        text_document_position_params: params,
        work_done_progress_params: Default::default(),
        partial_result_params: Default::default(),
    };
    let spec = definition::handle(DefinitionParams {
        base: goto,
        context: Some(DefinitionContext {
            source_definition: Some(true),
        }),
    });
    Some(spec)
}

fn rename_file_command(args: &[Value]) -> Option<RequestSpec> {
    let (old_path, new_path) = parse_rename_paths(args)?;
    let request = json!({
        "command": "getEditsForFileRename",
        "arguments": {
            "oldFilePath": old_path,
            "newFilePath": new_path,
        }
    });

    Some(RequestSpec {
        route: Route::Syntax,
        payload: request,
        priority: Priority::Low,
        on_response: Some(adapt_file_code_edits),
        response_context: None,
    })
}

fn file_references_command(args: &[Value]) -> Option<RequestSpec> {
    let target = parse_file_target(args)?;
    let request = json!({
        "command": "fileReferences",
        "arguments": { "file": target.file },
    });

    Some(RequestSpec {
        route: Route::Syntax,
        payload: request,
        priority: Priority::Normal,
        on_response: Some(adapt_file_references),
        response_context: None,
    })
}

fn combined_code_fix_payload(file: &str, fix_id: &str) -> Value {
    json!({
        "command": "getCombinedCodeFix",
        "arguments": {
            "scope": {
                "type": "file",
                "args": { "file": file },
            },
            "fixId": fix_id,
        }
    })
}

fn adapt_file_code_edits(payload: &Value, _context: Option<&Value>) -> Result<AdapterResult> {
    let changes = payload
        .get("body")
        .and_then(|value| value.as_array())
        .cloned()
        .unwrap_or_default();
    let edit = workspace_edit_from_tsserver_changes(&changes).unwrap_or_else(empty_workspace_edit);
    Ok(AdapterResult::ready(serde_json::to_value(edit)?))
}

fn adapt_combined_code_fix(payload: &Value, _context: Option<&Value>) -> Result<AdapterResult> {
    let combined = payload
        .get("body")
        .and_then(|body| {
            body.get("changes")
                .or_else(|| body.get("FileChanges"))
                .and_then(|value| value.as_array())
        })
        .cloned()
        .unwrap_or_default();
    let edit = workspace_edit_from_tsserver_changes(&combined).unwrap_or_else(empty_workspace_edit);
    Ok(AdapterResult::ready(serde_json::to_value(edit)?))
}

fn adapt_file_references(payload: &Value, _context: Option<&Value>) -> Result<AdapterResult> {
    let refs = payload
        .get("body")
        .and_then(|body| body.get("refs"))
        .and_then(|value| value.as_array())
        .cloned()
        .unwrap_or_default();

    let mut locations = Vec::new();
    for span in refs {
        if let Some(location) = tsserver_span_to_location(&span) {
            locations.push(location);
        }
    }

    Ok(AdapterResult::ready(serde_json::to_value(locations)?))
}

fn adapt_fix_all_chain(payload: &Value, context: Option<&Value>) -> Result<AdapterResult> {
    let mut state: FixAllContext =
        serde_json::from_value(context.cloned().context("missing fixAll context")?)?;
    let combined = payload
        .get("body")
        .and_then(|body| {
            body.get("changes")
                .or_else(|| body.get("FileChanges"))
                .and_then(|value| value.as_array())
        })
        .cloned()
        .unwrap_or_default();
    if let Some(edit) = workspace_edit_from_tsserver_changes(&combined) {
        state.merge_edit(edit);
    }

    if let Some(next_fix) = state.pending_fix_ids.pop_front() {
        let updated_context = serde_json::to_value(&state)?;
        return Ok(AdapterResult::Continue(RequestSpec {
            route: Route::Syntax,
            payload: combined_code_fix_payload(&state.file, &next_fix),
            priority: Priority::Low,
            on_response: Some(adapt_fix_all_chain),
            response_context: Some(updated_context),
        }));
    }

    let result = state.accumulated.unwrap_or_else(empty_workspace_edit);
    Ok(AdapterResult::ready(serde_json::to_value(result)?))
}

fn empty_workspace_edit() -> WorkspaceEdit {
    WorkspaceEdit {
        changes: Some(HashMap::new()),
        document_changes: None,
        change_annotations: None,
    }
}

fn parse_file_target(args: &[Value]) -> Option<CommandTarget> {
    let uri = extract_uri(args.first()?)?;
    let file = uri_to_file_path(uri.as_str()).unwrap_or_else(|| uri.to_string());
    Some(CommandTarget { uri, file })
}

fn extract_uri(value: &Value) -> Option<Uri> {
    if let Some(obj) = value.as_object() {
        if let Some(text_document) = obj.get("textDocument") {
            if let Ok(id) = serde_json::from_value::<TextDocumentIdentifier>(text_document.clone())
            {
                return Some(id.uri);
            }
        }
        if let Some(uri_value) = obj.get("uri").and_then(|v| v.as_str()) {
            return uri_value.parse().ok();
        }
    }
    if let Ok(id) = serde_json::from_value::<TextDocumentIdentifier>(value.clone()) {
        return Some(id.uri);
    }
    if let Some(uri_str) = value.as_str() {
        return uri_str.parse().ok();
    }
    None
}

fn parse_rename_paths(args: &[Value]) -> Option<(String, String)> {
    let first = args.first()?.clone();
    if let Ok(rename) = serde_json::from_value::<FileRename>(first.clone()) {
        return Some((
            uri_to_file_path(rename.old_uri.as_str()).unwrap_or_else(|| rename.old_uri.to_string()),
            uri_to_file_path(rename.new_uri.as_str()).unwrap_or_else(|| rename.new_uri.to_string()),
        ));
    }
    if let Some(files) = first.get("files").and_then(|v| v.as_array()) {
        if let Some(entry) = files.first() {
            if let Ok(rename) = serde_json::from_value::<FileRename>(entry.clone()) {
                return Some((
                    uri_to_file_path(rename.old_uri.as_str())
                        .unwrap_or_else(|| rename.old_uri.to_string()),
                    uri_to_file_path(rename.new_uri.as_str())
                        .unwrap_or_else(|| rename.new_uri.to_string()),
                ));
            }
        }
    }
    if let Ok(args) = serde_json::from_value::<RenameArgs>(first) {
        let old_path = uri_to_file_path(&args.old_uri).unwrap_or_else(|| args.old_uri.clone());
        let new_path = uri_to_file_path(&args.new_uri).unwrap_or_else(|| args.new_uri.clone());
        return Some((old_path, new_path));
    }
    None
}

fn merge_workspace_edits(target: &mut WorkspaceEdit, source: WorkspaceEdit) {
    let target_changes = target.changes.get_or_insert_with(HashMap::new);
    if let Some(changes) = source.changes {
        for (uri, mut edits) in changes.into_iter() {
            target_changes.entry(uri).or_default().append(&mut edits);
        }
    }
}

#[derive(Debug)]
struct CommandTarget {
    #[allow(dead_code)]
    uri: Uri,
    file: String,
}

#[derive(Deserialize)]
struct RenameArgs {
    #[serde(alias = "oldUri")]
    old_uri: String,
    #[serde(alias = "newUri")]
    new_uri: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct FixAllContext {
    file: String,
    #[serde(default)]
    pending_fix_ids: VecDeque<String>,
    #[serde(default)]
    accumulated: Option<WorkspaceEdit>,
}

impl FixAllContext {
    fn merge_edit(&mut self, edit: WorkspaceEdit) {
        match &mut self.accumulated {
            Some(accumulated) => merge_workspace_edits(accumulated, edit),
            None => self.accumulated = Some(edit),
        }
    }
}
