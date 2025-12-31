//! =============================================================================
//! codeAction/resolve
//! =============================================================================
//!
//! Resolves lazily-evaluated code actions, currently focusing on "fix all".
//! When a code action stores `CodeActionData::FixAll`, we reissue tsserver’s
//! `getCombinedCodeFix` to materialize the edits.

use anyhow::{Context, Result};
use lsp_types::{CodeAction, CodeActionKind};
use serde_json::{Value, json};

use crate::protocol::text_document::code_action::{
    CodeActionData, FixAllData, OrganizeImportsData, organize_imports_payload,
    workspace_edit_from_tsserver_changes,
};
use crate::protocol::{AdapterResult, RequestSpec};
use crate::rpc::{Priority, Route};

pub fn handle(mut action: CodeAction) -> Option<RequestSpec> {
    let data = action.data.take()?;
    let data: CodeActionData = serde_json::from_value(data).ok()?;

    match data {
        CodeActionData::FixAll(fix_all) => build_fix_all_request(action, fix_all),
        CodeActionData::OrganizeImports(data) => build_organize_imports_request(action, data),
    }
}

fn build_fix_all_request(action: CodeAction, fix_all: FixAllData) -> Option<RequestSpec> {
    let request = json!({
        "command": "getCombinedCodeFix",
        "arguments": {
            "scope": {
                "type": "file",
                "args": {
                    "file": fix_all.file,
                }
            },
            "fixId": fix_all.fix_id,
        }
    });

    let context = serde_json::to_value(action).ok()?;

    Some(RequestSpec {
        route: Route::Syntax,
        payload: request,
        priority: Priority::Low,
        on_response: Some(adapt_fix_all_response),
        response_context: Some(context),
        work_done: None,
    })
}

fn build_organize_imports_request(
    action: CodeAction,
    data: OrganizeImportsData,
) -> Option<RequestSpec> {
    let request = organize_imports_payload(&data.file);
    let context = serde_json::to_value(action).ok()?;

    Some(RequestSpec {
        route: Route::Syntax,
        payload: request,
        priority: Priority::Low,
        on_response: Some(adapt_organize_imports_response),
        response_context: Some(context),
        work_done: None,
    })
}

fn adapt_fix_all_response(payload: &Value, context: Option<&Value>) -> Result<AdapterResult> {
    let mut action: CodeAction =
        serde_json::from_value(context.cloned().context("missing code action context")?)?;
    let body = payload
        .get("body")
        .context("tsserver fix-all missing body")?;
    let combined = body
        .get("changes")
        .or_else(|| body.get("FileChanges"))
        .and_then(|value| value.as_array())
        .cloned()
        .unwrap_or_default();

    if let Some(edit) = workspace_edit_from_tsserver_changes(&combined) {
        action.edit = Some(edit);
    }

    if action.kind.is_none() {
        action.kind = Some(CodeActionKind::SOURCE_FIX_ALL);
    }

    Ok(AdapterResult::ready(serde_json::to_value(action)?))
}

fn adapt_organize_imports_response(
    payload: &Value,
    context: Option<&Value>,
) -> Result<AdapterResult> {
    let mut action: CodeAction =
        serde_json::from_value(context.cloned().context("missing code action context")?)?;
    let changes = payload
        .get("body")
        .and_then(|value| value.as_array())
        .cloned()
        .unwrap_or_default();

    if let Some(edit) = workspace_edit_from_tsserver_changes(&changes) {
        action.edit = Some(edit);
    }

    if action.kind.is_none() {
        action.kind = Some(CodeActionKind::SOURCE_ORGANIZE_IMPORTS);
    }

    Ok(AdapterResult::ready(serde_json::to_value(action)?))
}
