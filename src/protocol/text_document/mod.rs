//! =============================================================================
//! textDocument/* Handlers
//! =============================================================================
//!
//! Houses handlers for open/change/close, hover, completion, diagnostics, etc.
//! Only skeletons exist right now to mirror the Lua layout.

pub mod completion;
pub mod definition;
pub mod did_change;
pub mod did_close;
pub mod did_open;
pub mod hover;
pub mod references;
pub mod type_definition;
