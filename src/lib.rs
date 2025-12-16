//! =============================================================================
//! Crate Entry Points
//! =============================================================================
//!
//! The Lua implementation structures the codebase around a handful of
//! high-level subsystems (configuration, tsserver discovery, process
//! management, RPC bridging, protocol translators, and user-facing APIs). To
//! mirror that architecture we expose matching Rust modules so each concern
//! can be reimplemented in isolation without losing parity with the original
//! plugin.

pub mod api;
pub mod config;
pub mod process;
pub mod protocol;
pub mod provider;
pub mod rpc;
pub mod server;
pub mod types;
pub mod utils;

pub use server::run_stdio_server;
