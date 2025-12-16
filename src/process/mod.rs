//! =============================================================================
//! Tsserver Process Management
//! =============================================================================
//!
//! Tracks child Node processes, implements the `Content-Length` framed protocol,
//! and exposes cancellation pipes just like the Lua `process.lua`.

use crate::provider::TsserverBinary;

/// Represents an owned tsserver instance (syntax or semantic).
pub struct TsserverProcess {
    kind: ServerKind,
    binary: TsserverBinary,
}

impl TsserverProcess {
    pub fn new(kind: ServerKind, binary: TsserverBinary) -> Self {
        Self { kind, binary }
    }

    /// TODO: spawn the child node process and attach stdio pipes.
    pub fn start(&mut self) {
        todo!("Implement spawning logic mirroring lua/process.lua");
    }

    /// TODO: send a JSON-RPC payload to tsserver.
    pub fn write(&mut self, _payload: &serde_json::Value) {
        todo!("Serialize request frames + flush to stdin");
    }

    /// TODO: signal cancellation via pipe files on platforms that support it.
    pub fn cancel(&self, _seq: u64) {
        todo!("Touch cancellation pipe for seq id");
    }
}

#[derive(Debug, Clone, Copy)]
pub enum ServerKind {
    Syntax,
    Semantic,
}
