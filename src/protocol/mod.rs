//! =============================================================================
//! Protocol Handlers
//! =============================================================================
//!
//! Every LSP method will map to a Rust module inside this tree.  The Lua
//! implementation dynamically transformed method names into module paths; in
//! Rust we will keep a static registry for clarity and compile-time checking.

use serde_json::Value;

use crate::rpc::{Priority, Route};

pub mod diagnostics;
pub mod text_document;

#[derive(Debug)]
pub struct RequestSpec {
    pub route: Route,
    pub payload: Value,
    pub priority: Priority,
}

#[derive(Debug)]
pub struct NotificationSpec {
    pub route: Route,
    pub payload: Value,
    pub priority: Priority,
}

pub fn route_request(_method: &str, _params: Value) -> Option<RequestSpec> {
    None
}

pub fn route_notification(_method: &str, _params: Value) -> Option<NotificationSpec> {
    None
}
