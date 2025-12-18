//! =============================================================================
//! textDocument/* Handlers
//! =============================================================================
//!
//! Houses handlers for open/change/close, hover, completion, diagnostics, etc.
//! Only skeletons exist right now to mirror the Lua layout.

pub mod code_action;
pub mod code_action_resolve;
pub mod completion;
pub mod completion_resolve;
pub mod definition;
pub mod did_change;
pub mod did_close;
pub mod did_open;
pub mod hover;
pub mod references;
pub mod signature_help;
pub mod type_definition;
