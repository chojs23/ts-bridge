//! =============================================================================
//! Protocol Handlers
//! =============================================================================
//!
//! Every LSP method will map to a Rust module inside this tree.  The Lua
//! implementation dynamically transformed method names into module paths; in
//! Rust we will keep a static registry for clarity and compile-time checking.

use lsp_types::{GotoDefinitionParams, request::Request};
use serde_json::Value;

use crate::rpc::{Priority, Route};

pub mod diagnostics;
pub mod text_document;

pub type ResponseAdapter = fn(&Value, Option<&Value>) -> anyhow::Result<Value>;

#[derive(Debug)]
pub struct RequestSpec {
    pub route: Route,
    pub payload: Value,
    pub priority: Priority,
    pub on_response: Option<ResponseAdapter>,
    pub response_context: Option<Value>,
}

#[derive(Debug)]
pub struct NotificationSpec {
    pub route: Route,
    pub payload: Value,
    pub priority: Priority,
}

pub fn route_request(method: &str, params: Value) -> Option<RequestSpec> {
    match method {
        lsp_types::request::HoverRequest::METHOD => {
            let params: lsp_types::HoverParams = serde_json::from_value(params).ok()?;
            Some(text_document::hover::handle(params))
        }
        lsp_types::request::Completion::METHOD => {
            let params: lsp_types::CompletionParams = serde_json::from_value(params).ok()?;
            Some(text_document::completion::handle(params))
        }
        lsp_types::request::ResolveCompletionItem::METHOD => {
            let item: lsp_types::CompletionItem = serde_json::from_value(params).ok()?;
            text_document::completion_resolve::handle(item)
        }
        lsp_types::request::GotoDefinition::METHOD => {
            let params: text_document::definition::DefinitionParams =
                serde_json::from_value(params).ok()?;
            Some(text_document::definition::handle(params))
        }
        lsp_types::request::SignatureHelpRequest::METHOD => {
            let params: lsp_types::SignatureHelpParams = serde_json::from_value(params).ok()?;
            Some(text_document::signature_help::handle(params))
        }
        lsp_types::request::References::METHOD => {
            let params: lsp_types::ReferenceParams = serde_json::from_value(params).ok()?;
            Some(text_document::references::handle(params))
        }
        lsp_types::request::GotoTypeDefinition::METHOD => {
            let params: GotoDefinitionParams = serde_json::from_value(params).ok()?;
            Some(text_document::type_definition::handle(params))
        }
        lsp_types::request::CodeActionRequest::METHOD => {
            let params: lsp_types::CodeActionParams = serde_json::from_value(params).ok()?;
            Some(text_document::code_action::handle(params))
        }
        lsp_types::request::CodeActionResolveRequest::METHOD => {
            let action: lsp_types::CodeAction = serde_json::from_value(params).ok()?;
            text_document::code_action_resolve::handle(action)
        }
        _ => None,
    }
}

pub fn route_notification(_method: &str, _params: Value) -> Option<NotificationSpec> {
    None
}
