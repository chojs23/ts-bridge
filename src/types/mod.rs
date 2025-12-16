//! =============================================================================
//! Shared Types
//! =============================================================================
//!
//! The Lua plugin keeps all ad-hoc type annotations in `types.lua`.  This Rust
//! module mirrors that role so the rest of the crate can share request/response
//! structs without a web of circular dependencies.

use serde::{Deserialize, Serialize};

/// Minimal LSP text document identifier; expanded as we add handlers.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextDocumentIdentifier {
    pub uri: String,
}

/// Basic LSP position.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Position {
    pub line: u32,
    pub character: u32,
}

/// LSP range (start/end positions).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Range {
    pub start: Position,
    pub end: Position,
}
