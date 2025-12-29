//! =============================================================================
//! textDocument/* Handlers
//! =============================================================================
//!
//! Houses handlers for open/change/close, hover, completion, diagnostics, etc.

pub mod code_action;
pub mod code_action_resolve;
pub mod completion;
pub mod completion_resolve;
pub mod definition;
pub mod did_change;
pub mod did_close;
pub mod did_open;
pub mod document_highlight;
pub mod document_symbol;
pub mod formatting;
pub mod hover;
pub mod implementation;
pub mod inlay_hint;
pub mod references;
pub mod rename;
pub mod semantic_tokens;
pub mod signature_help;
pub mod type_definition;
