use std::path::{Path, PathBuf};

use anyhow::Context;
use lsp_server::{
    Connection, ErrorCode, Message, Notification as ServerNotification, Request, Response,
};
use lsp_types::{
    InitializeParams, InitializeResult, PublishDiagnosticsParams, ServerCapabilities,
    notification::{
        DidChangeTextDocument, DidCloseTextDocument, DidOpenTextDocument,
        Notification as LspNotification, PublishDiagnostics,
    },
};
use serde_json::{self, Value};

use crate::config::{Config, PluginSettings};
use crate::protocol;
use crate::provider::Provider;
use crate::rpc::Service;

/// Runs the LSP server over stdio. This is the entry-point Neovim (or any LSP
/// client) will execute.
pub fn run_stdio_server() -> anyhow::Result<()> {
    env_logger::init();

    let (connection, io_threads) = Connection::stdio();
    let (init_id, init_params) = connection
        .initialize_start()
        .context("waiting for initialize")?;
    let params: InitializeParams =
        serde_json::from_value(init_params).context("invalid initialize params")?;

    let workspace_root =
        workspace_root_from_params(&params).unwrap_or_else(|| std::env::current_dir().unwrap());
    let config = Config::new(PluginSettings::default());
    let provider = Provider::new(workspace_root);
    let service = Service::new(config, provider);

    let capabilities = advertised_capabilities();
    let init_result = InitializeResult {
        server_info: Some(lsp_types::ServerInfo {
            name: "ts-lsp-rs".to_string(),
            version: Some(env!("CARGO_PKG_VERSION").to_string()),
        }),
        capabilities,
    };
    connection
        .initialize_finish(init_id, serde_json::to_value(init_result)?)
        .context("failed to send initialize result")?;

    main_loop(connection, service)?;
    io_threads.join()?;

    Ok(())
}

fn advertised_capabilities() -> ServerCapabilities {
    ServerCapabilities {
        ..Default::default()
    }
}

#[allow(deprecated)]
fn workspace_root_from_params(params: &InitializeParams) -> Option<PathBuf> {
    if let Some(root_path) = &params.root_path {
        return Some(Path::new(root_path).to_path_buf());
    }

    None
}

fn main_loop(connection: Connection, mut service: Service) -> anyhow::Result<()> {
    if let Err(err) = service.start() {
        log::warn!("failed to start tsserver processes: {err:?}");
    }

    for message in &connection.receiver {
        match message {
            Message::Request(req) => {
                if handle_request(&connection, &mut service, req)? {
                    break;
                }
            }
            Message::Response(resp) => {
                log::debug!("ignoring stray response: {:?}", resp);
            }
            Message::Notification(notif) => {
                if notif.method == "exit" {
                    break;
                }
                if notif.method == DidOpenTextDocument::METHOD {
                    let params: crate::types::DidOpenTextDocumentParams =
                        serde_json::from_value(notif.params)?;
                    let spec = crate::protocol::text_document::did_open::handle(params);
                    if let Err(err) =
                        service.dispatch_request(spec.route, spec.payload, spec.priority)
                    {
                        log::warn!("failed to dispatch didOpen: {err}");
                    }
                    continue;
                }
                if notif.method == DidChangeTextDocument::METHOD {
                    let params: crate::types::DidChangeTextDocumentParams =
                        serde_json::from_value(notif.params)?;
                    let spec = crate::protocol::text_document::did_change::handle(params);
                    if let Err(err) =
                        service.dispatch_request(spec.route, spec.payload, spec.priority)
                    {
                        log::warn!("failed to dispatch didChange: {err}");
                    }
                    continue;
                }
                if notif.method == DidCloseTextDocument::METHOD {
                    let params: crate::types::DidCloseTextDocumentParams =
                        serde_json::from_value(notif.params)?;
                    let spec = crate::protocol::text_document::did_close::handle(params);
                    if let Err(err) =
                        service.dispatch_request(spec.route, spec.payload, spec.priority)
                    {
                        log::warn!("failed to dispatch didClose: {err}");
                    }
                    continue;
                }
                if let Some(spec) =
                    protocol::route_notification(&notif.method, notif.params.clone())
                {
                    if let Err(err) =
                        service.dispatch_request(spec.route, spec.payload, spec.priority)
                    {
                        log::warn!("failed to dispatch notification {}: {err}", notif.method);
                    }
                } else {
                    log::debug!("notification {} ignored", notif.method);
                }
            }
        }

        for event in service.poll_responses() {
            if let Some(params) = protocol::diagnostics::from_tsserver_event(&event.payload) {
                publish_diagnostics(&connection, params)?;
            } else {
                log::trace!("tsserver {:?} -> {}", event.server, event.payload);
            }
        }
    }

    Ok(())
}

fn publish_diagnostics(
    connection: &Connection,
    params: PublishDiagnosticsParams,
) -> anyhow::Result<()> {
    let notif = ServerNotification::new(
        PublishDiagnostics::METHOD.to_string(),
        serde_json::to_value(params)?,
    );
    connection.sender.send(Message::Notification(notif))?;
    Ok(())
}

fn handle_request(
    connection: &Connection,
    service: &mut Service,
    req: Request,
) -> anyhow::Result<bool> {
    if req.method == "shutdown" {
        let response = Response::new_ok(req.id, Value::Null);
        connection.sender.send(response.into())?;
        return Ok(true);
    }

    if req.method == "initialize" {
        // Already handled via initialize_start, but the client might resend; respond with error.
        let response = Response::new_err(
            req.id,
            ErrorCode::InvalidRequest as i32,
            "initialize already completed".to_string(),
        );
        connection.sender.send(response.into())?;
        return Ok(false);
    }

    if let Some(spec) = protocol::route_request(&req.method, req.params.clone()) {
        service.dispatch_request(spec.route, spec.payload, spec.priority)?;
        // TODO: store request id to respond when tsserver answers.
        let response = Response::new_err(
            req.id,
            ErrorCode::MethodNotFound as i32,
            "tsserver bridging not implemented yet".to_string(),
        );
        connection.sender.send(response.into())?;
        return Ok(false);
    }

    let response = Response::new_err(
        req.id,
        ErrorCode::MethodNotFound as i32,
        format!("method {} is not implemented yet", req.method),
    );
    connection.sender.send(response.into())?;

    Ok(false)
}
