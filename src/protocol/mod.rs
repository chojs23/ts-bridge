//! =============================================================================
//! Protocol Handlers
//! =============================================================================
//!
//! Every LSP method will map to a Rust module inside this tree.  The Lua
//! implementation dynamically transformed method names into module paths; in
//! Rust we will keep a static registry for clarity and compile-time checking.

pub mod text_document;

/// Lookup table from LSP method names to handler functions.
pub struct Router;

impl Router {
    pub fn resolve(_method: &str) -> Option<Handler> {
        // TODO: implement method → handler dispatch
        None
    }
}

/// Handler signature; actual implementation will likely be async.
pub type Handler = fn();
