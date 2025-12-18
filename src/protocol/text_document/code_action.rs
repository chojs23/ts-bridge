//! =============================================================================
//! textDocument/codeAction
//! =============================================================================
//!
//! Bridges LSP code actions to tsserver’s `getCodeFixes` command.  Every
//! diagnostic supplied by the client is forwarded so tsserver can suggest quick
//! fixes (missing imports, unreachable code, etc.).  Results are converted into
//! `CodeAction` entries with ready-to-apply workspace edits.  When tsserver also
//! reports a `fixId`, we surface a companion "fix all" action that is resolved
//! lazily via `codeAction/resolve`.

use std::collections::HashMap;

use anyhow::{Context, Result};
use lsp_types::{
    CodeAction, CodeActionContext, CodeActionKind, CodeActionOrCommand, CodeActionParams,
    CodeActionResponse, Diagnostic, NumberOrString, TextEdit, Uri, WorkspaceEdit,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::protocol::RequestSpec;
use crate::rpc::{Priority, Route};
use crate::utils::{tsserver_file_to_uri, tsserver_range_from_value_lsp, uri_to_file_path};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum CodeActionData {
    #[serde(rename = "fixAll")]
    FixAll(FixAllData),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FixAllData {
    pub file: String,
    pub fix_id: String,
}

#[derive(Debug, Deserialize)]
struct AdapterContext {
    file: String,
    context: CodeActionContext,
}

pub fn handle(params: CodeActionParams) -> RequestSpec {
    let CodeActionParams {
        text_document,
        range,
        context,
        work_done_progress_params: _,
        partial_result_params: _,
    } = params;
    let uri = text_document.uri;
    let file = uri_to_file_path(uri.as_str()).unwrap_or_else(|| uri.to_string());
    let error_codes = collect_error_codes(&context);

    let request = json!({
        "command": "getCodeFixes",
        "arguments": {
            "file": file,
            "startLine": range.start.line + 1,
            "startOffset": range.start.character + 1,
            "endLine": range.end.line + 1,
            "endOffset": range.end.character + 1,
            "errorCodes": error_codes,
        }
    });

    let adapter_context = json!({
        "file": file,
        "context": context,
    });

    RequestSpec {
        route: Route::Syntax,
        payload: request,
        priority: Priority::Normal,
        on_response: Some(adapt_code_actions),
        response_context: Some(adapter_context),
    }
}

fn adapt_code_actions(payload: &Value, context: Option<&Value>) -> Result<Value> {
    let adapter_ctx: AdapterContext =
        serde_json::from_value(context.cloned().context("code action context missing")?)?;
    let fixes = payload
        .get("body")
        .and_then(|value| value.as_array())
        .cloned()
        .unwrap_or_default();

    let mut actions: Vec<CodeActionOrCommand> = Vec::new();
    for fix in fixes {
        if let Some(action) = build_quick_fix(&fix, &adapter_ctx) {
            actions.push(CodeActionOrCommand::CodeAction(action));
        }
        if let Some(action) = build_fix_all_action(&fix, &adapter_ctx) {
            actions.push(CodeActionOrCommand::CodeAction(action));
        }
    }

    Ok(serde_json::to_value(CodeActionResponse::from(actions))?)
}

fn build_quick_fix(fix: &Value, ctx: &AdapterContext) -> Option<CodeAction> {
    let title = fix.get("description")?.as_str()?.to_string();
    let changes = fix.get("changes")?.as_array()?;
    let edit = workspace_edit_from_tsserver_changes(changes)?;
    let diagnostics = diagnostics_for_action(&ctx.context);

    let mut action = CodeAction {
        title,
        kind: Some(CodeActionKind::QUICKFIX),
        diagnostics,
        edit: Some(edit),
        ..CodeAction::default()
    };

    if let Some(preferred) = fix.get("isPreferred").and_then(|v| v.as_bool()) {
        action.is_preferred = Some(preferred);
    }

    Some(action)
}

fn build_fix_all_action(fix: &Value, ctx: &AdapterContext) -> Option<CodeAction> {
    let fix_id = fix.get("fixId")?.as_str()?;
    let description = fix.get("fixAllDescription")?.as_str()?;
    let diagnostics = diagnostics_for_action(&ctx.context);

    let mut action = CodeAction {
        title: description.to_string(),
        kind: Some(CodeActionKind::QUICKFIX),
        diagnostics,
        ..CodeAction::default()
    };

    let data = CodeActionData::FixAll(FixAllData {
        file: ctx.file.clone(),
        fix_id: fix_id.to_string(),
    });
    action.data = Some(serde_json::to_value(data).ok()?);

    Some(action)
}

fn diagnostics_for_action(context: &CodeActionContext) -> Option<Vec<Diagnostic>> {
    if context.diagnostics.is_empty() {
        None
    } else {
        Some(context.diagnostics.clone())
    }
}

fn collect_error_codes(context: &CodeActionContext) -> Vec<i32> {
    let mut codes = Vec::new();
    for diagnostic in &context.diagnostics {
        if let Some(NumberOrString::Number(value)) = diagnostic.code.clone() {
            codes.push(value as i32);
        }
    }
    codes
}

pub(crate) fn workspace_edit_from_tsserver_changes(changes: &[Value]) -> Option<WorkspaceEdit> {
    let mut map: HashMap<Uri, Vec<TextEdit>> = HashMap::new();
    for change in changes {
        let file_name = change
            .get("fileName")
            .or_else(|| change.get("file"))?
            .as_str()?;
        let uri = tsserver_file_to_uri(file_name)?;
        let text_changes = change.get("textChanges")?.as_array()?;
        let entry = map.entry(uri).or_default();
        for text_change in text_changes {
            let range = tsserver_range_from_value_lsp(text_change)?;
            let new_text = text_change.get("newText")?.as_str()?.to_string();
            entry.push(TextEdit { range, new_text });
        }
    }

    if map.is_empty() {
        None
    } else {
        Some(WorkspaceEdit {
            changes: Some(map),
            document_changes: None,
            change_annotations: None,
        })
    }
}
