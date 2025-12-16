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

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TextDocumentItem {
    pub uri: String,
    #[serde(rename = "languageId")]
    pub language_id: Option<String>,
    pub version: i32,
    pub text: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DidOpenTextDocumentParams {
    #[serde(rename = "textDocument")]
    pub text_document: TextDocumentItem,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct VersionedTextDocumentIdentifier {
    pub uri: String,
    pub version: Option<i32>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TextDocumentContentChangeEvent {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub range: Option<Range>,
    pub text: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DidChangeTextDocumentParams {
    #[serde(rename = "textDocument")]
    pub text_document: VersionedTextDocumentIdentifier,
    #[serde(rename = "contentChanges")]
    pub content_changes: Vec<TextDocumentContentChangeEvent>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DidCloseTextDocumentParams {
    #[serde(rename = "textDocument")]
    pub text_document: TextDocumentIdentifier,
}
