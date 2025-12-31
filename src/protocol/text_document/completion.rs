//! =============================================================================
//! textDocument/completion
//! =============================================================================
//!
//! Bridges LSP completion requests to tsserver’s `completionInfo` command and
//! reshapes the entries into `CompletionList` items.

use anyhow::{Context, Result};
use lsp_types::{
    CompletionItem, CompletionItemTag, CompletionList, CompletionParams, CompletionResponse,
    CompletionTextEdit, InsertTextFormat, Position, TextEdit,
};
use serde_json::{Value, json};

use crate::protocol::{AdapterResult, RequestSpec};
use crate::rpc::{Priority, Route};
use crate::utils::{
    completion_commit_characters, completion_item_kind_from_tsserver,
    tsserver_range_from_value_lsp, uri_to_file_path,
};

pub const TRIGGER_CHARACTERS: &[&str] = &[".", "\"", "'", "`", "/", "@", "<", "#", " "];

pub fn handle(params: CompletionParams) -> RequestSpec {
    let CompletionParams {
        text_document_position,
        work_done_progress_params: _,
        partial_result_params: _,
        context,
    } = params;
    let text_document = text_document_position.text_document;
    let position = text_document_position.position;
    let uri_string = text_document.uri.to_string();
    let file_name = uri_to_file_path(text_document.uri.as_str()).unwrap_or(uri_string);

    let trigger_kind = context.as_ref().map(|ctx| ctx.trigger_kind);
    let trigger_character = context
        .as_ref()
        .and_then(|ctx| ctx.trigger_character.clone())
        .filter(|ch| TRIGGER_CHARACTERS.contains(&ch.as_str()));

    let mut arguments = json!({
        "file": file_name,
        "line": position.line + 1,
        "offset": position.character + 1,
        "includeExternalModuleExports": true,
        "includeInsertTextCompletions": true,
    });
    if let Some(kind) = trigger_kind {
        arguments
            .as_object_mut()
            .unwrap()
            .insert("triggerKind".into(), json!(kind));
    }
    if let Some(character) = trigger_character {
        arguments
            .as_object_mut()
            .unwrap()
            .insert("triggerCharacter".into(), json!(character));
    }

    let request = json!({
        "command": "completionInfo",
        "arguments": arguments,
    });

    RequestSpec {
        route: Route::Syntax,
        payload: request,
        priority: Priority::Normal,
        on_response: Some(adapt_completion),
        response_context: Some(json!({
            "file": file_name,
            "position": {
                "line": position.line,
                "character": position.character,
            }
        })),
    }
}

fn adapt_completion(payload: &Value, context: Option<&Value>) -> Result<AdapterResult> {
    let ctx = context.context("completion context missing")?;
    let file = ctx
        .get("file")
        .and_then(|v| v.as_str())
        .context("completion context missing file")?;
    let position: Position = ctx
        .get("position")
        .cloned()
        .map(|value| serde_json::from_value(value))
        .transpose()?
        .unwrap_or(Position {
            line: 0,
            character: 0,
        });

    let body = payload
        .get("body")
        .context("tsserver completion missing body")?;
    let entries = body
        .get("entries")
        .and_then(|value| value.as_array())
        .cloned()
        .unwrap_or_default();
    let is_incomplete = body
        .get("isIncomplete")
        .and_then(|value| value.as_bool())
        .unwrap_or(false);

    let mut items = Vec::with_capacity(entries.len());
    for entry in entries {
        if let Some(item) = convert_entry(&entry, file, &position) {
            items.push(item);
        }
    }

    let list = CompletionList {
        is_incomplete,
        items,
    };
    Ok(AdapterResult::ready(serde_json::to_value(
        CompletionResponse::List(list),
    )?))
}

fn convert_entry(entry: &Value, file: &str, position: &Position) -> Option<CompletionItem> {
    let name = entry.get("name")?.as_str()?.to_string();
    let mut label = name.clone();
    let kind_modifiers = entry.get("kindModifiers").and_then(|v| v.as_str());
    if is_optional(kind_modifiers) {
        label.push('?');
    }

    let insert_text = entry
        .get("insertText")
        .and_then(|v| v.as_str())
        .unwrap_or(&name)
        .to_string();

    let kind = completion_item_kind_from_tsserver(entry.get("kind").and_then(|v| v.as_str()));

    let mut item = CompletionItem {
        label,
        kind: Some(kind),
        sort_text: entry
            .get("sortText")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        filter_text: Some(insert_text.clone()),
        insert_text: Some(insert_text.clone()),
        insert_text_format: Some(
            if entry
                .get("isSnippet")
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
            {
                InsertTextFormat::SNIPPET
            } else {
                InsertTextFormat::PLAIN_TEXT
            },
        ),
        ..CompletionItem::default()
    };

    if let Some(range_value) = entry.get("replacementSpan") {
        if let Some(range) = tsserver_range_from_value_lsp(range_value) {
            item.text_edit = Some(CompletionTextEdit::Edit(TextEdit {
                range,
                new_text: insert_text.clone(),
            }));
        }
    }

    if is_deprecated(kind_modifiers) {
        item.tags = Some(vec![CompletionItemTag::DEPRECATED]);
    }

    if let Some(chars) = completion_commit_characters(kind) {
        item.commit_characters = Some(chars);
    }

    if entry
        .get("hasAction")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
        && entry.get("source").is_some()
    {
        let sort = item
            .sort_text
            .clone()
            .unwrap_or_else(|| insert_text.clone());
        item.sort_text = Some(format!("\u{FFFF}{}", sort));
    }

    item.data = Some(json!({
        "file": file,
        "position": {
            "line": position.line,
            "character": position.character,
        },
        "entryNames": [build_entry_name(entry, &name)],
    }));

    Some(item)
}

fn is_deprecated(modifiers: Option<&str>) -> bool {
    modifiers
        .map(|mods| mods.contains("deprecated"))
        .unwrap_or(false)
}

fn is_optional(modifiers: Option<&str>) -> bool {
    modifiers
        .map(|mods| mods.contains("optional"))
        .unwrap_or(false)
}

fn build_entry_name(entry: &Value, name: &str) -> Value {
    let mut map = serde_json::Map::new();
    map.insert("name".to_string(), json!(name));
    if let Some(source) = entry.get("source") {
        map.insert("source".to_string(), source.clone());
    }
    if let Some(data) = entry.get("data") {
        map.insert("data".to_string(), data.clone());
    }
    Value::Object(map)
}
