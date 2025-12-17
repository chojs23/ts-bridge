use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use anyhow::Context;
use crossbeam_channel::RecvTimeoutError;
use lsp_server::{
    Connection, ErrorCode, Message, Notification as ServerNotification, Request, RequestId,
    Response,
};
use lsp_types::{
    CompletionOptions, HoverProviderCapability, InitializeParams, InitializeResult, OneOf,
    PositionEncodingKind, ProgressParams, ProgressParamsValue, ProgressToken,
    PublishDiagnosticsParams, ServerCapabilities, SignatureHelpOptions, TextDocumentSyncCapability,
    TextDocumentSyncKind, TextDocumentSyncOptions, TextDocumentSyncSaveOptions,
    TypeDefinitionProviderCapability, WorkDoneProgress as LspWorkDoneProgress,
    WorkDoneProgressBegin, WorkDoneProgressCreateParams, WorkDoneProgressEnd,
    WorkDoneProgressReport,
    notification::{
        DidChangeTextDocument, DidCloseTextDocument, DidOpenTextDocument,
        Notification as LspNotification, Progress, PublishDiagnostics,
    },
    request::{Request as LspRequest, WorkDoneProgressCreate},
};
use serde_json::{self, Value};

use crate::config::{Config, PluginSettings};
use crate::process::ServerKind;
use crate::protocol::diagnostics::{DiagnosticsEvent, DiagnosticsKind};
use crate::protocol::text_document::completion::TRIGGER_CHARACTERS;
use crate::protocol::text_document::signature_help::TRIGGER_CHARACTERS as SIG_HELP_TRIGGER_CHARACTERS;
use crate::protocol::{self, ResponseAdapter};
use crate::provider::Provider;
use crate::rpc::{DispatchReceipt, Service};
use crate::utils::uri_to_file_path;

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
    let text_sync = TextDocumentSyncOptions {
        open_close: Some(true),
        change: Some(TextDocumentSyncKind::INCREMENTAL),
        will_save: Some(false),
        will_save_wait_until: Some(false),
        save: Some(TextDocumentSyncSaveOptions::SaveOptions(
            lsp_types::SaveOptions::default(),
        )),
    };
    let completion_provider = CompletionOptions {
        resolve_provider: Some(true),
        trigger_characters: Some(TRIGGER_CHARACTERS.iter().map(|ch| ch.to_string()).collect()),
        ..CompletionOptions::default()
    };
    let signature_help_provider = SignatureHelpOptions {
        trigger_characters: Some(
            SIG_HELP_TRIGGER_CHARACTERS
                .iter()
                .map(|ch| ch.to_string())
                .collect(),
        ),
        retrigger_characters: Some(vec![",".into(), ")".into()]),
        ..SignatureHelpOptions::default()
    };
    ServerCapabilities {
        position_encoding: Some(PositionEncodingKind::UTF16),
        hover_provider: Some(HoverProviderCapability::Simple(true)),
        definition_provider: Some(OneOf::Left(true)),
        references_provider: Some(OneOf::Left(true)),
        type_definition_provider: Some(TypeDefinitionProviderCapability::Simple(true)),
        completion_provider: Some(completion_provider),
        signature_help_provider: Some(signature_help_provider),
        text_document_sync: Some(TextDocumentSyncCapability::Options(text_sync)),
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

    let mut pending = PendingRequests::default();
    let mut diag_state = DiagnosticsState::default();
    let mut progress = LoadingProgress::new();
    if let Err(err) = progress.begin(
        &connection,
        "ts-lsp-rs",
        "Booting the TypeScript language service",
    ) {
        log::debug!("work-done progress begin failed: {err:?}");
    }

    let poll_interval = Duration::from_millis(10);
    loop {
        drain_tsserver(
            &connection,
            &mut service,
            &mut pending,
            &mut diag_state,
            &mut progress,
        )?;

        match connection.receiver.recv_timeout(poll_interval) {
            Ok(message) => match message {
                Message::Request(req) => {
                    if handle_request(&connection, &mut service, &mut pending, req)? {
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
                        let file_for_diagnostics =
                            uri_to_file_path(params.text_document.uri.as_str())
                                .unwrap_or_else(|| params.text_document.uri.to_string());
                        let spec = crate::protocol::text_document::did_open::handle(
                            params,
                            service.workspace_root(),
                        );
                        if let Err(err) =
                            service.dispatch_request(spec.route, spec.payload, spec.priority)
                        {
                            log::warn!("failed to dispatch didOpen: {err}");
                        }
                        if let Err(err) = progress.report(
                            &connection,
                            &format!("Analyzing {}", friendly_file_name(&file_for_diagnostics)),
                        ) {
                            log::debug!("work-done progress report failed: {err:?}");
                        }
                        request_file_diagnostics(
                            &mut service,
                            &file_for_diagnostics,
                            &mut diag_state,
                        );
                        continue;
                    }
                    if notif.method == DidChangeTextDocument::METHOD {
                        let params: crate::types::DidChangeTextDocumentParams =
                            serde_json::from_value(notif.params)?;
                        let file_for_diagnostics =
                            uri_to_file_path(params.text_document.uri.as_str())
                                .unwrap_or_else(|| params.text_document.uri.to_string());
                        let spec = crate::protocol::text_document::did_change::handle(
                            params,
                            service.workspace_root(),
                        );
                        if let Err(err) =
                            service.dispatch_request(spec.route, spec.payload, spec.priority)
                        {
                            log::warn!("failed to dispatch didChange: {err}");
                        }
                        if let Err(err) = progress.report(
                            &connection,
                            &format!("Analyzing {}", friendly_file_name(&file_for_diagnostics)),
                        ) {
                            log::debug!("work-done progress report failed: {err:?}");
                        }
                        request_file_diagnostics(
                            &mut service,
                            &file_for_diagnostics,
                            &mut diag_state,
                        );
                        continue;
                    }
                    if notif.method == DidCloseTextDocument::METHOD {
                        let params: crate::types::DidCloseTextDocumentParams =
                            serde_json::from_value(notif.params)?;
                        let uri = params.text_document.uri.clone();
                        let spec = crate::protocol::text_document::did_close::handle(
                            params,
                            service.workspace_root(),
                        );
                        if let Err(err) =
                            service.dispatch_request(spec.route, spec.payload, spec.priority)
                        {
                            log::warn!("failed to dispatch didClose: {err}");
                        }
                        clear_client_diagnostics(&connection, uri)?;
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
            },
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }

    Ok(())
}

fn drain_tsserver(
    connection: &Connection,
    service: &mut Service,
    pending: &mut PendingRequests,
    diag_state: &mut DiagnosticsState,
    progress: &mut LoadingProgress,
) -> anyhow::Result<()> {
    for event in service.poll_responses() {
        if let Some(diag_event) = protocol::diagnostics::parse_tsserver_event(&event.payload) {
            diag_state.handle_event(event.server, diag_event);
            while let Some((uri, diagnostics)) = diag_state.take_ready() {
                publish_diagnostics(
                    connection,
                    PublishDiagnosticsParams {
                        uri,
                        diagnostics,
                        version: None,
                    },
                )?;
                if let Err(err) = progress.end(connection, "Language features ready") {
                    log::debug!("work-done progress end failed: {err:?}");
                }
            }
            continue;
        } else if let Some(response) = pending.resolve(event.server, &event.payload)? {
            connection.sender.send(response.into())?;
        } else {
            log::trace!("tsserver {:?} -> {}", event.server, event.payload);
        }
    }
    Ok(())
}

fn request_file_diagnostics(service: &mut Service, file: &str, diag_state: &mut DiagnosticsState) {
    let spec = protocol::diagnostics::request_for_file(file);
    match service.dispatch_request(spec.route, spec.payload, spec.priority) {
        Ok(receipts) => {
            for receipt in receipts {
                diag_state.register_pending(receipt.server, receipt.seq);
            }
        }
        Err(err) => {
            log::warn!("failed to dispatch geterr for {}: {err}", file);
        }
    }
}

fn clear_client_diagnostics(connection: &Connection, uri_str: String) -> anyhow::Result<()> {
    let uri =
        lsp_types::Uri::from_str(&uri_str).context("invalid URI while clearing diagnostics")?;
    publish_diagnostics(
        connection,
        PublishDiagnosticsParams {
            uri,
            diagnostics: Vec::new(),
            version: None,
        },
    )
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
    pending: &mut PendingRequests,
    req: Request,
) -> anyhow::Result<bool> {
    let lsp_server::Request { id, method, params } = req;

    if method == "shutdown" {
        let response = Response::new_ok(id, Value::Null);
        connection.sender.send(response.into())?;
        return Ok(true);
    }

    if method == "initialize" {
        // Already handled via initialize_start, but the client might resend; respond with error.
        let response = Response::new_err(
            id,
            ErrorCode::InvalidRequest as i32,
            "initialize already completed".to_string(),
        );
        connection.sender.send(response.into())?;
        return Ok(false);
    }

    if let Some(spec) = protocol::route_request(&method, params) {
        match service.dispatch_request(spec.route, spec.payload, spec.priority) {
            Ok(receipts) => {
                if let Some(adapter) = spec.on_response {
                    if receipts.is_empty() {
                        let response = Response::new_err(
                            id,
                            ErrorCode::InternalError as i32,
                            "tsserver route produced no requests".to_string(),
                        );
                        connection.sender.send(response.into())?;
                    } else {
                        pending.track(&receipts, id, adapter, spec.response_context);
                    }
                } else {
                    let response = Response::new_err(
                        id,
                        ErrorCode::InternalError as i32,
                        "handler missing response adapter".to_string(),
                    );
                    connection.sender.send(response.into())?;
                }
            }
            Err(err) => {
                let response = Response::new_err(
                    id,
                    ErrorCode::InternalError as i32,
                    format!("failed to dispatch tsserver request: {err}"),
                );
                connection.sender.send(response.into())?;
            }
        }
        return Ok(false);
    }

    let response = Response::new_err(
        id,
        ErrorCode::MethodNotFound as i32,
        format!("method {method} is not implemented yet"),
    );
    connection.sender.send(response.into())?;

    Ok(false)
}

#[derive(Default)]
struct PendingRequests {
    entries: HashMap<PendingKey, PendingEntry>,
}

impl PendingRequests {
    fn track(
        &mut self,
        receipts: &[DispatchReceipt],
        id: RequestId,
        adapter: ResponseAdapter,
        context: Option<Value>,
    ) {
        for receipt in receipts {
            self.entries.insert(
                PendingKey {
                    server: receipt.server,
                    seq: receipt.seq,
                },
                PendingEntry {
                    id: id.clone(),
                    adapter,
                    context: context.clone(),
                },
            );
        }
    }

    fn resolve(&mut self, server: ServerKind, payload: &Value) -> anyhow::Result<Option<Response>> {
        if payload
            .get("type")
            .and_then(|kind| kind.as_str())
            .map(|kind| kind != "response")
            .unwrap_or(true)
        {
            return Ok(None);
        }

        let request_seq = match payload.get("request_seq").and_then(|seq| seq.as_u64()) {
            Some(seq) => seq,
            None => return Ok(None),
        };

        let entry = match self.entries.remove(&PendingKey {
            server,
            seq: request_seq,
        }) {
            Some(entry) => entry,
            None => return Ok(None),
        };

        let success = payload
            .get("success")
            .and_then(|value| value.as_bool())
            .unwrap_or(false);

        if success {
            match (entry.adapter)(payload, entry.context.as_ref()) {
                Ok(result) => Ok(Some(Response::new_ok(entry.id, result))),
                Err(err) => Ok(Some(Response::new_err(
                    entry.id,
                    ErrorCode::InternalError as i32,
                    format!("failed to adapt tsserver response: {err}"),
                ))),
            }
        } else {
            let message = payload
                .get("message")
                .and_then(|m| m.as_str())
                .unwrap_or("tsserver request failed");
            Ok(Some(Response::new_err(
                entry.id,
                ErrorCode::InternalError as i32,
                message.to_string(),
            )))
        }
    }
}

#[derive(Debug, Hash, PartialEq, Eq)]
struct PendingKey {
    server: ServerKind,
    seq: u64,
}

struct PendingEntry {
    id: RequestId,
    adapter: ResponseAdapter,
    context: Option<Value>,
}

#[derive(Default)]
struct DiagnosticsState {
    pending: HashMap<(ServerKind, u64), HashMap<lsp_types::Uri, FileDiagnostics>>,
    order: HashMap<ServerKind, VecDeque<u64>>,
    latest: HashMap<lsp_types::Uri, FileDiagnostics>,
    ready: VecDeque<(lsp_types::Uri, Vec<lsp_types::Diagnostic>)>,
}

impl DiagnosticsState {
    fn register_pending(&mut self, server: ServerKind, seq: u64) {
        self.order
            .entry(server)
            .or_insert_with(VecDeque::new)
            .push_back(seq);
        self.pending
            .entry((server, seq))
            .or_insert_with(HashMap::new);
    }

    fn handle_event(&mut self, server: ServerKind, event: DiagnosticsEvent) {
        match event {
            DiagnosticsEvent::Report {
                uri,
                diagnostics,
                request_seq,
                kind,
            } => {
                let key = request_seq.map(|seq| (server, seq)).or_else(|| {
                    self.order
                        .get(&server)
                        .and_then(|queue| queue.front().copied())
                        .map(|seq| (server, seq))
                });
                if let Some(key) = key {
                    if let Some(files) = self.pending.get_mut(&key) {
                        files
                            .entry(uri.clone())
                            .or_insert_with(FileDiagnostics::default)
                            .update_kind(kind, diagnostics);
                        return;
                    }
                }
                let mut latest = self.latest.remove(&uri).unwrap_or_default();
                latest.update_kind(kind, diagnostics);
                let combined = latest.collect();
                if !combined.is_empty() {
                    self.latest.insert(uri.clone(), latest);
                }
                self.ready.push_back((uri, combined));
            }
            DiagnosticsEvent::Completed { request_seq } => {
                let key = (server, request_seq);
                if let Some(files) = self.pending.remove(&key) {
                    if let Some(queue) = self.order.get_mut(&server) {
                        if let Some(pos) = queue.iter().position(|seq| *seq == request_seq) {
                            queue.remove(pos);
                        }
                    }
                    for (uri, diags) in files {
                        let combined = diags.collect();
                        if combined.is_empty() {
                            self.latest.remove(&uri);
                        } else {
                            self.latest.insert(uri.clone(), diags);
                        }
                        self.ready.push_back((uri, combined));
                    }
                }
            }
        }
    }

    fn take_ready(&mut self) -> Option<(lsp_types::Uri, Vec<lsp_types::Diagnostic>)> {
        self.ready.pop_front()
    }
}

#[derive(Clone, Default)]
struct FileDiagnostics {
    syntax: Vec<lsp_types::Diagnostic>,
    semantic: Vec<lsp_types::Diagnostic>,
    suggestion: Vec<lsp_types::Diagnostic>,
}

struct LoadingProgress {
    token: ProgressToken,
    created: bool,
    active: bool,
}

impl LoadingProgress {
    fn new() -> Self {
        let token = ProgressToken::String(format!("ts-lsp-rs:{}", std::process::id()));
        Self {
            token,
            created: false,
            active: false,
        }
    }

    fn begin(&mut self, connection: &Connection, title: &str, message: &str) -> anyhow::Result<()> {
        if self.active {
            return Ok(());
        }
        self.ensure_token(connection)?;
        let params = ProgressParams {
            token: self.token.clone(),
            value: ProgressParamsValue::WorkDone(LspWorkDoneProgress::Begin(
                WorkDoneProgressBegin {
                    title: title.to_string(),
                    message: Some(message.to_string()),
                    ..WorkDoneProgressBegin::default()
                },
            )),
        };
        send_progress(connection, params)?;
        self.active = true;
        Ok(())
    }

    fn report(&mut self, connection: &Connection, message: &str) -> anyhow::Result<()> {
        if !self.active {
            return Ok(());
        }
        let params = ProgressParams {
            token: self.token.clone(),
            value: ProgressParamsValue::WorkDone(LspWorkDoneProgress::Report(
                WorkDoneProgressReport {
                    message: Some(message.to_string()),
                    ..WorkDoneProgressReport::default()
                },
            )),
        };
        send_progress(connection, params)
    }

    fn end(&mut self, connection: &Connection, message: &str) -> anyhow::Result<()> {
        if !self.active {
            return Ok(());
        }
        let params = ProgressParams {
            token: self.token.clone(),
            value: ProgressParamsValue::WorkDone(LspWorkDoneProgress::End(WorkDoneProgressEnd {
                message: Some(message.to_string()),
            })),
        };
        send_progress(connection, params)?;
        self.active = false;
        Ok(())
    }

    fn ensure_token(&mut self, connection: &Connection) -> anyhow::Result<()> {
        if self.created {
            return Ok(());
        }
        let params = WorkDoneProgressCreateParams {
            token: self.token.clone(),
        };
        let request = Request::new(
            next_request_id(),
            <WorkDoneProgressCreate as LspRequest>::METHOD.to_string(),
            serde_json::to_value(params)?,
        );
        connection.sender.send(Message::Request(request))?;
        self.created = true;
        Ok(())
    }
}

fn send_progress(connection: &Connection, params: ProgressParams) -> anyhow::Result<()> {
    let notif =
        ServerNotification::new(Progress::METHOD.to_string(), serde_json::to_value(params)?);
    connection.sender.send(Message::Notification(notif))?;
    Ok(())
}

static SERVER_REQUEST_IDS: AtomicU64 = AtomicU64::new(1);

fn next_request_id() -> RequestId {
    let seq = SERVER_REQUEST_IDS.fetch_add(1, Ordering::Relaxed);
    RequestId::from(format!("ts-lsp-rs-request-{seq}"))
}

fn friendly_file_name(path: &str) -> String {
    Path::new(path)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(path)
        .to_string()
}

impl FileDiagnostics {
    fn update_kind(&mut self, kind: DiagnosticsKind, diagnostics: Vec<lsp_types::Diagnostic>) {
        match kind {
            DiagnosticsKind::Syntax => self.syntax = diagnostics,
            DiagnosticsKind::Semantic => self.semantic = diagnostics,
            DiagnosticsKind::Suggestion => self.suggestion = diagnostics,
        }
    }

    fn collect(&self) -> Vec<lsp_types::Diagnostic> {
        let mut all =
            Vec::with_capacity(self.syntax.len() + self.semantic.len() + self.suggestion.len());
        all.extend(self.syntax.iter().cloned());
        all.extend(self.semantic.iter().cloned());
        all.extend(self.suggestion.iter().cloned());
        all
    }
}
