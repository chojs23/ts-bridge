//! =============================================================================
//! RPC Bridge
//! =============================================================================
//!
//! This layer glues Neovim’s LSP transport to the tsserver processes.  In Lua
//! this logic lived inside `rpc.lua` and `tsserver.lua`; here we split the
//! responsibilities into:
//! * request routing (syntax vs semantic)
//! * request queue/priorities/cancellation
//! * handler dispatch into the protocol module tree

use crate::config::Config;
use crate::process::{ServerKind, TsserverProcess};
use crate::provider::{Provider, TsserverBinary};

/// Public facade invoked by Neovim (or any embedding host).  Eventually this
/// type will implement whatever trait the chosen RPC runtime expects.
pub struct Service {
    config: Config,
    provider: Provider,
    syntax: Option<TsserverProcess>,
    semantic: Option<TsserverProcess>,
}

impl Service {
    pub fn new(config: Config, provider: Provider) -> Self {
        Self {
            config,
            provider,
            syntax: None,
            semantic: None,
        }
    }

    /// Bootstraps tsserver processes once (mirrors Lua’s `TsserverProvider.init`
    /// + `Tsserver.new` calls).
    pub fn start(&mut self) -> Result<(), ServiceError> {
        let binary = self.provider.resolve().map_err(ServiceError::Provider)?;
        let syntax = TsserverProcess::new(ServerKind::Syntax, binary.clone());
        self.syntax = Some(syntax);

        if self.config.plugin().separate_diagnostic_server {
            let semantic = TsserverProcess::new(ServerKind::Semantic, binary);
            self.semantic = Some(semantic);
        }

        Ok(())
    }
}

#[derive(thiserror::Error, Debug)]
pub enum ServiceError {
    #[error(transparent)]
    Provider(#[from] crate::provider::ProviderError),
}
