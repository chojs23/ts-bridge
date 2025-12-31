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

use crate::protocol::{AdapterResult, RequestSpec};
use crate::rpc::{Priority, Route};
use crate::utils::{tsserver_file_to_uri, tsserver_range_from_value_lsp, uri_to_file_path};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum CodeActionData {
    #[serde(rename = "fixAll")]
    FixAll(FixAllData),
    #[serde(rename = "organizeImports")]
    OrganizeImports(OrganizeImportsData),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FixAllData {
    pub file: String,
    pub fix_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrganizeImportsData {
    pub file: String,
}

#[derive(Debug, Deserialize)]
struct AdapterContext {
    file: String,
    context: CodeActionContext,
    #[serde(default, rename = "includeOrganize")]
    include_organize: bool,
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
    let context_only = context.only.clone();
    let wants_organize = context_only
        .as_ref()
        .map(|list| {
            list.iter()
                .any(|kind| matches_kind(kind, CodeActionKind::SOURCE_ORGANIZE_IMPORTS.as_str()))
        })
        .unwrap_or(false);
    let wants_quickfix = context_only
        .as_ref()
        .map(|list| {
            list.iter()
                .any(|kind| matches_kind(kind, CodeActionKind::QUICKFIX.as_str()))
        })
        .unwrap_or(true);

    let has_filter = context_only
        .as_ref()
        .map(|list| !list.is_empty())
        .unwrap_or(false);

    if wants_organize && !wants_quickfix {
        return organize_imports_request(file);
    }

    // When the client didn't filter (`only` empty/missing), include organize imports alongside
    // quick fixes so the default picker shows it.
    let include_organize = wants_organize || !has_filter;

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
        "includeOrganize": include_organize,
    });

    RequestSpec {
        route: Route::Syntax,
        payload: request,
        priority: Priority::Normal,
        on_response: Some(adapt_code_actions),
        response_context: Some(adapter_context),
    }
}

fn organize_imports_request(file: String) -> RequestSpec {
    let request = organize_imports_payload(&file);

    RequestSpec {
        route: Route::Syntax,
        payload: request,
        priority: Priority::Low,
        on_response: Some(adapt_organize_imports),
        response_context: None,
    }
}

fn adapt_code_actions(payload: &Value, context: Option<&Value>) -> Result<AdapterResult> {
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

    if adapter_ctx.include_organize {
        if let Some(action) = organize_imports_placeholder(&adapter_ctx.file) {
            actions.push(CodeActionOrCommand::CodeAction(action));
        }
    }

    Ok(AdapterResult::ready(serde_json::to_value(
        CodeActionResponse::from(actions),
    )?))
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

    // "Fix all" belongs to the source action family so clients can filter by
    // `source.fixAll` in picker UIs. Classifying it as a quick fix hides the
    // entry whenever a client explicitly requests source actions only.
    let mut action = CodeAction {
        title: description.to_string(),
        kind: Some(CodeActionKind::SOURCE_FIX_ALL),
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

fn organize_imports_placeholder(file: &str) -> Option<CodeAction> {
    let data = CodeActionData::OrganizeImports(OrganizeImportsData {
        file: file.to_string(),
    });
    Some(CodeAction {
        title: "Organize Imports".to_string(),
        kind: Some(CodeActionKind::SOURCE_ORGANIZE_IMPORTS),
        data: Some(serde_json::to_value(data).ok()?),
        ..CodeAction::default()
    })
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

fn adapt_organize_imports(payload: &Value, _context: Option<&Value>) -> Result<AdapterResult> {
    let changes = payload
        .get("body")
        .and_then(|value| value.as_array())
        .cloned()
        .unwrap_or_default();

    let mut actions: Vec<CodeActionOrCommand> = Vec::new();
    if let Some(edit) = workspace_edit_from_tsserver_changes(&changes) {
        let action = CodeAction {
            title: "Organize Imports".to_string(),
            kind: Some(CodeActionKind::SOURCE_ORGANIZE_IMPORTS),
            edit: Some(edit),
            ..CodeAction::default()
        };
        actions.push(CodeActionOrCommand::CodeAction(action));
    }

    Ok(AdapterResult::ready(serde_json::to_value(
        CodeActionResponse::from(actions),
    )?))
}

fn matches_kind(kind: &CodeActionKind, needle: &str) -> bool {
    let value = kind.as_str();
    value == needle || value.starts_with(&(needle.to_string() + "."))
}

pub(crate) fn organize_imports_payload(file: &str) -> Value {
    json!({
        "command": "organizeImports",
        "arguments": {
            "scope": {
                "type": "file",
                "args": {
                    "file": file,
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use lsp_types::{
        CodeActionContext, CodeActionKind, Diagnostic, Position, Range, TextDocumentIdentifier, Uri,
    };
    use serde_json::json;
    use std::str::FromStr;

    const FILE_URI: &str = "file:///workspace/app.ts";
    const FILE_PATH: &str = "/workspace/app.ts";

    fn sample_diagnostic(code: i32) -> Diagnostic {
        Diagnostic {
            range: Range {
                start: Position {
                    line: 0,
                    character: 0,
                },
                end: Position {
                    line: 0,
                    character: 1,
                },
            },
            code: Some(NumberOrString::Number(code)),
            ..Diagnostic::default()
        }
    }

    fn sample_context() -> CodeActionContext {
        CodeActionContext {
            diagnostics: vec![sample_diagnostic(6133)],
            only: None,
            trigger_kind: None,
        }
    }

    fn adapter_context(include_organize: bool) -> Value {
        json!({
            "file": FILE_PATH,
            "context": sample_context(),
            "includeOrganize": include_organize,
        })
    }

    #[test]
    fn build_fix_all_action_sets_source_kind() {
        let ctx = AdapterContext {
            file: FILE_PATH.to_string(),
            context: sample_context(),
            include_organize: false,
        };
        let fix = json!({
            "fixId": "fixAllMissingImports",
            "fixAllDescription": "Fix all missing imports",
        });

        let action = build_fix_all_action(&fix, &ctx).expect("fix all action");
        assert_eq!(action.kind, Some(CodeActionKind::SOURCE_FIX_ALL));
        let data: CodeActionData =
            serde_json::from_value(action.data.expect("data")).expect("code action data");
        match data {
            CodeActionData::FixAll(fix_all) => {
                assert_eq!(fix_all.file, FILE_PATH);
                assert_eq!(fix_all.fix_id, "fixAllMissingImports");
            }
            _ => panic!("expected fix all data"),
        }
    }

    #[test]
    fn adapt_code_actions_emits_quick_fix_fix_all_and_organize() {
        let payload = json!({
            "body": [{
                "description": "Add missing import",
                "changes": [{
                    "fileName": FILE_PATH,
                    "textChanges": [{
                        "start": { "line": 1, "offset": 1 },
                        "end": { "line": 1, "offset": 1 },
                        "newText": "import { foo } from 'foo';\n"
                    }]
                }],
                "fixId": "fixAllMissingImports",
                "fixAllDescription": "Fix all missing imports",
                "isPreferred": true,
            }]
        });
        let ctx_value = adapter_context(true);

        let adapted = adapt_code_actions(&payload, Some(&ctx_value)).expect("adapt");
        let value = match adapted {
            AdapterResult::Ready(value) => value,
            AdapterResult::Continue(_) => panic!("expected ready code action response"),
        };
        let actions: Vec<_> = match serde_json::from_value::<CodeActionResponse>(value) {
            Ok(actions) => actions,
            Err(err) => panic!("failed to deserialize code action response: {err}"),
        };
        assert_eq!(actions.len(), 3, "quick fix, fix all, organize placeholder");

        match &actions[0] {
            CodeActionOrCommand::CodeAction(action) => {
                assert_eq!(action.kind, Some(CodeActionKind::QUICKFIX));
                assert!(action.edit.is_some(), "quick fix should have edit");
                assert_eq!(action.is_preferred, Some(true));
            }
            _ => panic!("expected code action"),
        }

        match &actions[1] {
            CodeActionOrCommand::CodeAction(action) => {
                assert_eq!(action.kind, Some(CodeActionKind::SOURCE_FIX_ALL));
                let data: CodeActionData =
                    serde_json::from_value(action.data.clone().unwrap()).expect("fix all data");
                match data {
                    CodeActionData::FixAll(fix_all) => {
                        assert_eq!(fix_all.file, FILE_PATH);
                    }
                    _ => panic!("expected fix all data"),
                }
            }
            _ => panic!("expected fix all code action"),
        }

        match &actions[2] {
            CodeActionOrCommand::CodeAction(action) => {
                assert_eq!(action.kind, Some(CodeActionKind::SOURCE_ORGANIZE_IMPORTS));
                assert!(action.data.is_some());
            }
            _ => panic!("expected organize imports action"),
        }
    }

    #[test]
    fn handle_collects_error_codes_from_context() {
        let params = CodeActionParams {
            text_document: TextDocumentIdentifier {
                uri: Uri::from_str(FILE_URI).expect("uri"),
            },
            range: lsp_types::Range {
                start: lsp_types::Position {
                    line: 0,
                    character: 0,
                },
                end: lsp_types::Position {
                    line: 0,
                    character: 1,
                },
            },
            context: CodeActionContext {
                diagnostics: vec![sample_diagnostic(1234)],
                only: None,
                trigger_kind: None,
            },
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        };

        let spec = handle(params);
        let args = spec
            .payload
            .get("arguments")
            .and_then(|value| value.as_object())
            .expect("arguments");
        let error_codes = args
            .get("errorCodes")
            .and_then(|v| v.as_array())
            .expect("error codes array");
        assert_eq!(error_codes, &[json!(1234)]);
    }
}
