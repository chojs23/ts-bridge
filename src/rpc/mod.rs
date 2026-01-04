//! =============================================================================
//! RPC Bridge
//! =============================================================================
//!
//! This layer glues Neovim’s LSP transport to the tsserver processes.
//! * request routing (syntax vs semantic)
//! * request queue/priorities/cancellation
//! * handler dispatch into the protocol module tree

mod queue;
pub use queue::{Priority, Request, RequestQueue};

use crossbeam_channel::{Receiver, TryRecvError};
use serde_json::Value;

use crate::config::Config;
use crate::process::{ServerKind, TsserverProcess};
use crate::provider::Provider;
/// Public facade invoked by Neovim (or any embedding host).  Eventually this
/// type will implement whatever trait the chosen RPC runtime expects.
pub struct Service {
    config: Config,
    provider: Provider,
    syntax: Option<TsserverProcess>,
    semantic: Option<TsserverProcess>,
    syntax_rx: Option<Receiver<Value>>,
    semantic_rx: Option<Receiver<Value>>,
    syntax_queue: RequestQueue,
    semantic_queue: RequestQueue,
}

impl Service {
    pub fn new(config: Config, provider: Provider) -> Self {
        Self {
            config,
            provider,
            syntax: None,
            semantic: None,
            syntax_rx: None,
            semantic_rx: None,
            syntax_queue: RequestQueue::default(),
            semantic_queue: RequestQueue::default(),
        }
    }

    /// Bootstraps tsserver processes once
    pub fn start(&mut self) -> Result<(), ServiceError> {
        let binary = self.provider.resolve().map_err(ServiceError::Provider)?;
        let launch = self.config.plugin().tsserver.clone();
        let mut syntax = TsserverProcess::new(ServerKind::Syntax, binary.clone(), launch.clone());
        syntax.start().map_err(ServiceError::Process)?;
        self.syntax_rx = syntax.response_rx();
        self.syntax = Some(syntax);

        if self.config.plugin().separate_diagnostic_server {
            let mut semantic = TsserverProcess::new(ServerKind::Semantic, binary, launch);
            semantic.start().map_err(ServiceError::Process)?;
            self.semantic_rx = semantic.response_rx();
            self.semantic = Some(semantic);
        }

        Ok(())
    }

    fn syntax_mut(&mut self) -> Result<&mut TsserverProcess, ServiceError> {
        if self.syntax.is_none() {
            self.start()?;
        }
        self.syntax.as_mut().ok_or(ServiceError::ProcessNotStarted)
    }

    fn semantic_mut(&mut self) -> Option<&mut TsserverProcess> {
        self.semantic.as_mut()
    }

    /// Queues a request for the given route and returns the syntax seq (when applicable).
    pub fn dispatch_request(
        &mut self,
        route: Route,
        payload: Value,
        priority: Priority,
    ) -> Result<Vec<DispatchReceipt>, ServiceError> {
        let mut receipts = Vec::new();
        match route {
            Route::Syntax => {
                let seq = self.syntax_queue.enqueue(payload, priority);
                self.flush_queue(ServerKind::Syntax)?;
                receipts.push(DispatchReceipt {
                    server: ServerKind::Syntax,
                    seq,
                });
            }
            Route::Semantic => {
                if self.semantic.is_some() {
                    let seq = self.semantic_queue.enqueue(payload, priority);
                    self.flush_queue(ServerKind::Semantic)?;
                    receipts.push(DispatchReceipt {
                        server: ServerKind::Semantic,
                        seq,
                    });
                }
            }
            Route::Both => {
                let seq = self.syntax_queue.enqueue(payload.clone(), priority);
                self.flush_queue(ServerKind::Syntax)?;
                receipts.push(DispatchReceipt {
                    server: ServerKind::Syntax,
                    seq,
                });
                if self.semantic.is_some() {
                    let semantic_seq = self.semantic_queue.enqueue(payload, priority);
                    self.flush_queue(ServerKind::Semantic)?;
                    receipts.push(DispatchReceipt {
                        server: ServerKind::Semantic,
                        seq: semantic_seq,
                    });
                }
            }
        }

        Ok(receipts)
    }

    /// Cancels a pending request on both servers.
    pub fn cancel(&self, seq: u64) -> Result<(), ServiceError> {
        if let Some(server) = &self.syntax {
            server.cancel(seq).map_err(ServiceError::Process)?;
        }
        if let Some(server) = &self.semantic {
            server.cancel(seq).map_err(ServiceError::Process)?;
        }
        Ok(())
    }

    /// Drains any ready responses from syntax/semantic readers without blocking.
    pub fn poll_responses(&self) -> Vec<ServerEvent> {
        let mut events = Vec::new();
        if let Some(rx) = &self.syntax_rx {
            collect_events(ServerKind::Syntax, rx, &mut events);
        }
        if let Some(rx) = &self.semantic_rx {
            collect_events(ServerKind::Semantic, rx, &mut events);
        }
        events
    }

    pub fn workspace_root(&self) -> &std::path::Path {
        self.provider.workspace_root()
    }

    fn flush_queue(&mut self, kind: ServerKind) -> Result<(), ServiceError> {
        match kind {
            ServerKind::Syntax => {
                while let Some(request) = self.syntax_queue.dequeue() {
                    let server = self.syntax_mut()?;
                    server
                        .write(&request.payload)
                        .map_err(ServiceError::Process)?;
                }
            }
            ServerKind::Semantic => {
                while let Some(request) = self.semantic_queue.dequeue() {
                    if let Some(server) = self.semantic_mut() {
                        server
                            .write(&request.payload)
                            .map_err(ServiceError::Process)?;
                    }
                }
            }
        }
        Ok(())
    }

    pub fn update_config(&mut self, new_config: Config) {
        self.config = new_config;
    }

    pub fn restart(
        &mut self,
        restart_syntax: bool,
        restart_semantic: bool,
    ) -> Result<(), ServiceError> {
        if restart_syntax {
            self.syntax = None;
            self.syntax_rx = None;
            self.syntax_queue.reset();
        }
        if restart_semantic {
            self.semantic = None;
            self.semantic_rx = None;
            self.semantic_queue.reset();
        }
        Ok(())
    }

    pub fn config(&self) -> &Config {
        &self.config
    }

    pub fn config_mut(&mut self) -> &mut Config {
        &mut self.config
    }

    pub fn tsserver_status(&self) -> TsserverStatus {
        TsserverStatus {
            syntax_pid: self.syntax.as_ref().and_then(|process| process.pid()),
            semantic_pid: self.semantic.as_ref().and_then(|process| process.pid()),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct TsserverStatus {
    pub syntax_pid: Option<u32>,
    pub semantic_pid: Option<u32>,
}

#[derive(thiserror::Error, Debug)]
pub enum ServiceError {
    #[error(transparent)]
    Provider(#[from] crate::provider::ProviderError),
    #[error("failed interaction with tsserver process: {0}")]
    Process(#[from] crate::process::ProcessError),
    #[error("syntax process not started yet")]
    ProcessNotStarted,
}

#[derive(Debug, Clone)]
pub struct ServerEvent {
    pub server: ServerKind,
    pub payload: Value,
}

#[derive(Debug, Clone, Copy)]
pub struct DispatchReceipt {
    pub server: ServerKind,
    pub seq: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    Syntax,
    Semantic,
    Both,
}

fn collect_events(kind: ServerKind, rx: &Receiver<Value>, out: &mut Vec<ServerEvent>) {
    loop {
        match rx.try_recv() {
            Ok(payload) => out.push(ServerEvent {
                server: kind,
                payload,
            }),
            Err(TryRecvError::Empty) => break,
            Err(TryRecvError::Disconnected) => break,
        }
    }
}
