//! =============================================================================
//! User-Facing API Helpers
//! =============================================================================
//!
//! Functions exposed to end users (organize imports, fix-all, rename file,
//! diagnostics throttling, etc.) will live here.  This structure mirrors
//! `lua/typescript-tools/api.lua`.

pub fn organize_imports_sync() {
    todo!("Call protocol::text_document::organize_imports handler once implemented");
}

pub fn request_diagnostics() {
    todo!("Bridge to custom diagnostic request");
}
