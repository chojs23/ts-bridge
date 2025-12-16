//! =============================================================================
//! Protocol Handlers
//! =============================================================================
//!
//! Every LSP method will map to a Rust module inside this tree.  The Lua
//! implementation dynamically transformed method names into module paths; in
//! Rust we will keep a static registry for clarity and compile-time checking.

use lsp_types::request::Request;
use serde_json::Value;

use crate::rpc::{Priority, Route};

pub mod diagnostics;
pub mod text_document;

pub type ResponseAdapter = fn(&Value) -> anyhow::Result<Value>;

#[derive(Debug)]
pub struct RequestSpec {
    pub route: Route,
    pub payload: Value,
    pub priority: Priority,
    pub on_response: Option<ResponseAdapter>,
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
        _ => None,
    }
}

pub fn route_notification(_method: &str, _params: Value) -> Option<NotificationSpec> {
    None
}
