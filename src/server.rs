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
    CodeActionKind, CodeActionOptions, CodeActionProviderCapability, CompletionOptions,
    HoverProviderCapability, InitializeParams, InitializeResult, InlayHintOptions,
    InlayHintServerCapabilities, OneOf, PositionEncodingKind, ProgressParams, ProgressParamsValue,
    ProgressToken, PublishDiagnosticsParams, RenameOptions, ServerCapabilities,
    SignatureHelpOptions, TextDocumentSyncCapability, TextDocumentSyncKind,
    TextDocumentSyncOptions, TextDocumentSyncSaveOptions, TypeDefinitionProviderCapability,
    WorkDoneProgress as LspWorkDoneProgress, WorkDoneProgressBegin, WorkDoneProgressCreateParams,
    WorkDoneProgressEnd, WorkDoneProgressReport,
    notification::{
        DidChangeConfiguration, DidChangeTextDocument, DidCloseTextDocument, DidOpenTextDocument,
        Notification as LspNotification, Progress, PublishDiagnostics,
    },
    request::{
        InlayHintRefreshRequest, InlayHintRequest, Request as LspRequest, WorkDoneProgressCreate,
    },
};
use serde_json::{self, Value, json};

use crate::config::{Config, PluginSettings};
use crate::documents::{DocumentStore, TextSpan};
use crate::process::ServerKind;
use crate::protocol::diagnostics::{DiagnosticsEvent, DiagnosticsKind};
use crate::protocol::text_document::completion::TRIGGER_CHARACTERS;
use crate::protocol::text_document::signature_help::TRIGGER_CHARACTERS as SIG_HELP_TRIGGER_CHARACTERS;
use crate::protocol::{self, ResponseAdapter};
use crate::provider::Provider;
use crate::rpc::{DispatchReceipt, Priority, Route, Service};
use crate::utils::uri_to_file_path;

const DEFAULT_INLAY_HINT_SPAN: u32 = 5_000_000;

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
    let mut config = Config::new(PluginSettings::default());

    if let Some(options) = params.initialization_options.as_ref()
        && config.apply_workspace_settings(options)
    {
        log::info!("applied initializationOptions to ts-bridge settings");
    }

    let provider = Provider::new(workspace_root);
    let service = Service::new(config, provider);

    let capabilities = advertised_capabilities(service.config().plugin());
    let init_result = InitializeResult {
        server_info: Some(lsp_types::ServerInfo {
            name: "ts-bridge".to_string(),
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

fn advertised_capabilities(settings: &PluginSettings) -> ServerCapabilities {
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
    let code_action_provider = CodeActionProviderCapability::Options(CodeActionOptions {
        code_action_kinds: Some(vec![
            CodeActionKind::QUICKFIX,
            CodeActionKind::SOURCE_ORGANIZE_IMPORTS,
        ]),
        resolve_provider: Some(true),
        work_done_progress_options: Default::default(),
    });
    let rename_provider = OneOf::Right(RenameOptions {
        prepare_provider: Some(true),
        work_done_progress_options: Default::default(),
    });
    let semantic_tokens_provider =
        lsp_types::SemanticTokensServerCapabilities::SemanticTokensOptions(
            lsp_types::SemanticTokensOptions {
                legend: crate::protocol::text_document::semantic_tokens::legend(),
                range: Some(true),
                full: Some(lsp_types::SemanticTokensFullOptions::Bool(true)),
                work_done_progress_options: Default::default(),
            },
        );
    let inlay_hint_provider = if settings.enable_inlay_hints {
        Some(OneOf::Right(InlayHintServerCapabilities::Options(
            InlayHintOptions {
                work_done_progress_options: Default::default(),
                resolve_provider: None,
            },
        )))
    } else {
        None
    };
    ServerCapabilities {
        position_encoding: Some(PositionEncodingKind::UTF16),
        hover_provider: Some(HoverProviderCapability::Simple(true)),
        definition_provider: Some(OneOf::Left(true)),
        references_provider: Some(OneOf::Left(true)),
        type_definition_provider: Some(TypeDefinitionProviderCapability::Simple(true)),
        document_symbol_provider: Some(OneOf::Left(true)),
        workspace_symbol_provider: Some(OneOf::Left(true)),
        completion_provider: Some(completion_provider),
        signature_help_provider: Some(signature_help_provider),
        code_action_provider: Some(code_action_provider),
        rename_provider: Some(rename_provider),
        document_formatting_provider: Some(OneOf::Left(true)),
        semantic_tokens_provider: Some(semantic_tokens_provider),
        inlay_hint_provider,
        text_document_sync: Some(TextDocumentSyncCapability::Options(text_sync)),
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn advertised_capabilities_include_inlay_hints_when_enabled() {
        let settings = PluginSettings::default();
        let caps = advertised_capabilities(&settings);

        assert!(caps.inlay_hint_provider.is_some());
        assert_eq!(
            caps.position_encoding,
            Some(PositionEncodingKind::UTF16),
            "initialize should advertise UTF-16 positions"
        );
        match caps.text_document_sync {
            Some(TextDocumentSyncCapability::Options(options)) => {
                assert_eq!(options.change, Some(TextDocumentSyncKind::INCREMENTAL));
            }
            other => panic!("unexpected sync capability: {other:?}"),
        }
    }

    #[test]
    fn advertised_capabilities_disable_inlay_hints_when_setting_is_false() {
        let settings = PluginSettings {
            enable_inlay_hints: false,
            ..Default::default()
        };

        let caps = advertised_capabilities(&settings);
        assert!(
            caps.inlay_hint_provider.is_none(),
            "initialize must omit inlay hint capability when disabled"
        );
    }
}

#[allow(deprecated)]
fn workspace_root_from_params(params: &InitializeParams) -> Option<PathBuf> {
    // Prefer the modern URIs so Neovim/VSCode multi-root setups resolve to
    // the correct project instead of wherever `ts-bridge` happens to run.
    if let Some(root_path) = &params.root_path {
        return Some(Path::new(root_path).to_path_buf());
    }

    if let Some(root_uri) = &params.root_uri {
        if let Some(path) = uri_to_file_path(root_uri.as_str()) {
            return Some(PathBuf::from(path));
        }
    }

    if let Some(folders) = &params.workspace_folders {
        for folder in folders {
            if let Some(path) = uri_to_file_path(folder.uri.as_str()) {
                return Some(PathBuf::from(path));
            }
        }
    }

    None
}

fn main_loop(connection: Connection, mut service: Service) -> anyhow::Result<()> {
    let mut pending = PendingRequests::default();
    let mut diag_state = DiagnosticsState::default();
    let mut progress = LoadingProgress::new();
    let mut documents = DocumentStore::default();
    let mut inlay_cache = InlayHintCache::default();
    let mut inlay_preferences = InlayPreferenceState::default();
    let project_label = friendly_project_name(service.workspace_root());
    if let Err(err) = progress.begin(
        &connection,
        "ts-bridge",
        &format!("Booting {project_label}"),
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
            &mut inlay_cache,
            &project_label,
        )?;

        match connection.receiver.recv_timeout(poll_interval) {
            Ok(message) => match message {
                Message::Request(req) => {
                    if handle_request(
                        &connection,
                        &mut service,
                        &mut pending,
                        &documents,
                        &mut inlay_cache,
                        &mut inlay_preferences,
                        req,
                    )? {
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
                        if let Ok(uri) = lsp_types::Uri::from_str(&params.text_document.uri) {
                            documents.open(
                                &uri,
                                &params.text_document.text,
                                Some(params.text_document.version),
                            );
                            inlay_cache.invalidate(&uri);
                        }
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
                        request_file_diagnostics(
                            &mut service,
                            &file_for_diagnostics,
                            &mut diag_state,
                        );
                        if let Err(err) = progress.report(
                            &connection,
                            &format!("Analyzing {project_label} — scheduling diagnostics"),
                            diag_state.progress_percent(),
                        ) {
                            log::debug!("work-done progress report failed: {err:?}");
                        }
                        continue;
                    }
                    if notif.method == DidChangeTextDocument::METHOD {
                        let params: crate::types::DidChangeTextDocumentParams =
                            serde_json::from_value(notif.params)?;
                        if let Ok(uri) = lsp_types::Uri::from_str(&params.text_document.uri) {
                            documents.apply_changes(
                                &uri,
                                &params.content_changes,
                                params.text_document.version,
                            );
                            inlay_cache.invalidate(&uri);
                        }
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
                        request_file_diagnostics(
                            &mut service,
                            &file_for_diagnostics,
                            &mut diag_state,
                        );
                        if let Err(err) = progress.report(
                            &connection,
                            &format!("Analyzing {project_label} — scheduling diagnostics"),
                            diag_state.progress_percent(),
                        ) {
                            log::debug!("work-done progress report failed: {err:?}");
                        }
                        continue;
                    }
                    if notif.method == DidCloseTextDocument::METHOD {
                        let params: crate::types::DidCloseTextDocumentParams =
                            serde_json::from_value(notif.params)?;
                        let uri = params.text_document.uri.clone();
                        if let Ok(parsed) = lsp_types::Uri::from_str(&uri) {
                            documents.close(&parsed);
                            inlay_cache.invalidate(&parsed);
                        }
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
                    if notif.method == DidChangeConfiguration::METHOD {
                        let params: lsp_types::DidChangeConfigurationParams =
                            serde_json::from_value(notif.params)?;
                        let changed = service
                            .config_mut()
                            .apply_workspace_settings(&params.settings);
                        if changed {
                            log::info!("workspace settings reloaded from didChangeConfiguration");
                            inlay_preferences.invalidate();
                            // TODO: restart auxiliary tsserver processes when toggles require it.
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
    inlay_cache: &mut InlayHintCache,
    project_label: &str,
) -> anyhow::Result<()> {
    for event in service.poll_responses() {
        if let Some(diag_event) = protocol::diagnostics::parse_tsserver_event(&event.payload) {
            let stage_label = match &diag_event {
                DiagnosticsEvent::Report { kind, .. } => Some(stage_text(*kind)),
                DiagnosticsEvent::Completed { .. } => Some("finalizing diagnostics"),
            };
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
            }
            if diag_state.has_pending() {
                let message = if let Some(stage) = stage_label {
                    format!("Analyzing {project_label} — {stage}")
                } else {
                    format!("Analyzing {project_label}")
                };
                if let Err(err) =
                    progress.report(connection, &message, diag_state.progress_percent())
                {
                    log::debug!("work-done progress report failed: {err:?}");
                }
            } else {
                if let Err(err) = progress.end(
                    connection,
                    &format!("Language features ready in {project_label}"),
                ) {
                    log::debug!("work-done progress end failed: {err:?}");
                }
                diag_state.reset_if_idle();
            }
            continue;
        } else if let Some(response) = pending.resolve(event.server, &event.payload, inlay_cache)? {
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
    documents: &DocumentStore,
    inlay_cache: &mut InlayHintCache,
    inlay_preferences: &mut InlayPreferenceState,
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

    if method == InlayHintRefreshRequest::METHOD {
        inlay_cache.clear();
        let response = Response::new_ok(id, Value::Null);
        connection.sender.send(response.into())?;
        return Ok(false);
    }

    let params_value = params;
    let spec: Option<protocol::RequestSpec>;
    let mut postprocess = None;

    if method == InlayHintRequest::METHOD {
        let enabled = service.config().plugin().enable_inlay_hints;
        inlay_preferences.ensure(service)?;
        if !enabled {
            let response = Response::new_ok(id, Value::Array(Vec::new()));
            connection.sender.send(response.into())?;
            return Ok(false);
        }
        let hint_params: lsp_types::InlayHintParams =
            serde_json::from_value(params_value.clone()).context("invalid inlay hint params")?;
        if let Some(cached) = inlay_cache.lookup(&hint_params) {
            let response = Response::new_ok(id, serde_json::to_value(cached)?);
            connection.sender.send(response.into())?;
            return Ok(false);
        }
        let span = documents
            .span_for_range(&hint_params.text_document.uri, &hint_params.range)
            .unwrap_or_else(|| {
                log::warn!(
                    "missing document snapshot for {}; requesting wide span",
                    hint_params.text_document.uri.as_str()
                );
                TextSpan::covering_length(DEFAULT_INLAY_HINT_SPAN)
            });
        postprocess = Some(PostProcess::inlay_hint(&hint_params));
        spec = Some(crate::protocol::text_document::inlay_hint::handle(
            hint_params,
            span,
        ));
    } else {
        spec = protocol::route_request(&method, params_value);
    }

    if let Some(spec) = spec {
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
                        pending.track(
                            &receipts,
                            id,
                            adapter,
                            spec.response_context,
                            postprocess.clone(),
                        );
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
        postprocess: Option<PostProcess>,
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
                    postprocess: postprocess.clone(),
                },
            );
        }
    }

    fn resolve(
        &mut self,
        server: ServerKind,
        payload: &Value,
        inlay_cache: &mut InlayHintCache,
    ) -> anyhow::Result<Option<Response>> {
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
                Ok(result) => {
                    if let Some(postprocess) = entry.postprocess {
                        postprocess.apply(&result, inlay_cache)?;
                    }
                    Ok(Some(Response::new_ok(entry.id, result)))
                }
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
    postprocess: Option<PostProcess>,
}

#[derive(Clone)]
enum PostProcess {
    InlayHints { key: HintCacheKey },
}

impl PostProcess {
    fn inlay_hint(params: &lsp_types::InlayHintParams) -> Self {
        Self::InlayHints {
            key: HintCacheKey::new(&params.text_document.uri, &params.range),
        }
    }

    fn apply(self, value: &Value, cache: &mut InlayHintCache) -> anyhow::Result<()> {
        match self {
            PostProcess::InlayHints { key } => {
                let hints: Vec<lsp_types::InlayHint> = serde_json::from_value(value.clone())
                    .context("failed to decode inlay hint response payload")?;
                cache.store(key, hints);
            }
        }
        Ok(())
    }
}

#[derive(Default)]
struct InlayHintCache {
    entries: HashMap<HintCacheKey, Vec<lsp_types::InlayHint>>,
}

#[derive(Default)]
struct InlayPreferenceState {
    configured_for: Option<bool>,
}

impl InlayPreferenceState {
    fn ensure(&mut self, service: &mut Service) -> anyhow::Result<()> {
        let desired = service.config().plugin().enable_inlay_hints;
        if self.configured_for == Some(desired) {
            return Ok(());
        }
        self.dispatch(service, desired)?;
        self.configured_for = Some(desired);
        Ok(())
    }

    fn dispatch(&self, service: &mut Service, enabled: bool) -> anyhow::Result<()> {
        let request = json!({
            "command": "configure",
            "arguments": {
                "preferences": crate::protocol::text_document::inlay_hint::preferences(enabled),
            }
        });
        let _ = service
            .dispatch_request(Route::Both, request, Priority::Const)
            .context("failed to dispatch tsserver configure request")?;
        Ok(())
    }

    fn invalidate(&mut self) {
        self.configured_for = None;
    }
}

impl InlayHintCache {
    fn lookup(&self, params: &lsp_types::InlayHintParams) -> Option<Vec<lsp_types::InlayHint>> {
        let key = HintCacheKey::new(&params.text_document.uri, &params.range);
        self.entries.get(&key).cloned()
    }

    fn store(&mut self, key: HintCacheKey, hints: Vec<lsp_types::InlayHint>) {
        self.entries.insert(key, hints);
    }

    fn invalidate(&mut self, uri: &lsp_types::Uri) {
        let needle = uri.to_string();
        self.entries.retain(|key, _| key.uri != needle);
    }

    fn clear(&mut self) {
        self.entries.clear();
    }
}

#[derive(Hash, PartialEq, Eq, Clone)]
struct HintCacheKey {
    uri: String,
    range: RangeFingerprint,
}

impl HintCacheKey {
    fn new(uri: &lsp_types::Uri, range: &lsp_types::Range) -> Self {
        Self {
            uri: uri.to_string(),
            range: RangeFingerprint::from_range(range),
        }
    }
}

#[derive(Hash, PartialEq, Eq, Clone)]
struct RangeFingerprint {
    start_line: u32,
    start_character: u32,
    end_line: u32,
    end_character: u32,
}

impl RangeFingerprint {
    fn from_range(range: &lsp_types::Range) -> Self {
        Self {
            start_line: range.start.line,
            start_character: range.start.character,
            end_line: range.end.line,
            end_character: range.end.character,
        }
    }
}

#[derive(Default)]
struct DiagnosticsState {
    pending: HashMap<(ServerKind, u64), PendingDiagnosticsEntry>,
    order: HashMap<ServerKind, VecDeque<u64>>,
    latest: HashMap<lsp_types::Uri, FileDiagnostics>,
    ready: VecDeque<(lsp_types::Uri, Vec<lsp_types::Diagnostic>)>,
    workload: Workload,
}

impl DiagnosticsState {
    fn register_pending(&mut self, server: ServerKind, seq: u64) {
        self.order.entry(server).or_default().push_back(seq);
        let entry = PendingDiagnosticsEntry::new(server);
        self.workload.add_expected(entry.progress.expected_count());
        self.pending.insert((server, seq), entry);
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
                    if let Some(entry) = self.pending.get_mut(&key) {
                        entry
                            .files
                            .entry(uri.clone())
                            .or_insert_with(FileDiagnostics::default)
                            .update_kind(kind, diagnostics);
                        if entry.progress.mark(kind) {
                            self.workload.add_completed(1);
                        }
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
                if let Some(mut entry) = self.pending.remove(&key) {
                    if let Some(queue) = self.order.get_mut(&server) {
                        if let Some(pos) = queue.iter().position(|seq| *seq == request_seq) {
                            queue.remove(pos);
                        }
                    }
                    for (uri, diags) in entry.files.into_iter() {
                        let combined = diags.collect();
                        if combined.is_empty() {
                            self.latest.remove(&uri);
                        } else {
                            self.latest.insert(uri.clone(), diags);
                        }
                        self.ready.push_back((uri, combined));
                    }
                    let forced = entry.progress.finish_outstanding();
                    if forced > 0 {
                        self.workload.add_completed(forced);
                    }
                }
            }
        }
    }

    fn take_ready(&mut self) -> Option<(lsp_types::Uri, Vec<lsp_types::Diagnostic>)> {
        self.ready.pop_front()
    }

    fn progress_percent(&self) -> Option<u32> {
        if self.workload.expected == 0 {
            None
        } else {
            Some(
                (self.workload.completed.saturating_mul(100) / self.workload.expected)
                    .clamp(0, 100),
            )
        }
    }

    fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }

    fn reset_if_idle(&mut self) {
        if self.pending.is_empty() {
            self.workload.reset();
        }
    }
}

struct PendingDiagnosticsEntry {
    files: HashMap<lsp_types::Uri, FileDiagnostics>,
    progress: StepProgress,
}

impl PendingDiagnosticsEntry {
    fn new(server: ServerKind) -> Self {
        Self {
            files: HashMap::new(),
            progress: StepProgress::for_server(server),
        }
    }
}

#[derive(Clone, Default)]
struct FileDiagnostics {
    syntax: Vec<lsp_types::Diagnostic>,
    semantic: Vec<lsp_types::Diagnostic>,
    suggestion: Vec<lsp_types::Diagnostic>,
}

#[derive(Clone, Copy)]
struct StepProgress {
    syntax: StepState,
    semantic: StepState,
    suggestion: StepState,
}

impl StepProgress {
    fn for_server(server: ServerKind) -> Self {
        match server {
            ServerKind::Syntax => Self {
                syntax: StepState::expected(true),
                semantic: StepState::expected(false),
                suggestion: StepState::expected(true),
            },
            ServerKind::Semantic => Self {
                syntax: StepState::expected(false),
                semantic: StepState::expected(true),
                suggestion: StepState::expected(false),
            },
        }
    }

    fn expected_count(&self) -> u32 {
        self.syntax.expected_count()
            + self.semantic.expected_count()
            + self.suggestion.expected_count()
    }

    fn mark(&mut self, kind: DiagnosticsKind) -> bool {
        match kind {
            DiagnosticsKind::Syntax => self.syntax.mark_done(),
            DiagnosticsKind::Semantic => self.semantic.mark_done(),
            DiagnosticsKind::Suggestion => self.suggestion.mark_done(),
        }
    }

    fn finish_outstanding(&mut self) -> u32 {
        let mut added = 0;
        if self.syntax.finish() {
            added += 1;
        }
        if self.semantic.finish() {
            added += 1;
        }
        if self.suggestion.finish() {
            added += 1;
        }
        added
    }
}

#[derive(Clone, Copy)]
struct StepState {
    expected: bool,
    done: bool,
}

impl StepState {
    fn expected(expected: bool) -> Self {
        Self {
            expected,
            done: !expected,
        }
    }

    fn expected_count(&self) -> u32 {
        if self.expected { 1 } else { 0 }
    }

    fn mark_done(&mut self) -> bool {
        if self.expected && !self.done {
            self.done = true;
            true
        } else {
            false
        }
    }

    fn finish(&mut self) -> bool {
        self.mark_done()
    }
}

#[derive(Clone, Copy, Default)]
struct Workload {
    expected: u32,
    completed: u32,
}

impl Workload {
    fn add_expected(&mut self, count: u32) {
        self.expected = self.expected.saturating_add(count);
    }

    fn add_completed(&mut self, count: u32) {
        if count == 0 {
            return;
        }
        self.completed = (self.completed + count).min(self.expected);
    }

    fn reset(&mut self) {
        self.expected = 0;
        self.completed = 0;
    }
}

struct LoadingProgress {
    token: ProgressToken,
    created: bool,
    active: bool,
}

impl LoadingProgress {
    fn new() -> Self {
        let token = ProgressToken::String(format!("ts-bridge:{}", std::process::id()));
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

    fn report(
        &mut self,
        connection: &Connection,
        message: &str,
        percent: Option<u32>,
    ) -> anyhow::Result<()> {
        if !self.active {
            return Ok(());
        }
        let params = ProgressParams {
            token: self.token.clone(),
            value: ProgressParamsValue::WorkDone(LspWorkDoneProgress::Report(
                WorkDoneProgressReport {
                    message: Some(message.to_string()),
                    percentage: percent,
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
    RequestId::from(format!("ts-bridge-request-{seq}"))
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

fn friendly_project_name(root: &Path) -> String {
    root.file_name()
        .and_then(|name| name.to_str())
        .map(|name| name.to_string())
        .unwrap_or_else(|| root.display().to_string())
}

fn stage_text(kind: DiagnosticsKind) -> &'static str {
    match kind {
        DiagnosticsKind::Syntax => "running syntax checks",
        DiagnosticsKind::Semantic => "evaluating semantic diagnostics",
        DiagnosticsKind::Suggestion => "collecting suggestions",
    }
}
