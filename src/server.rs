use std::collections::{HashMap, HashSet, VecDeque};
use std::io::{self, BufReader};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, anyhow};
use crossbeam_channel::{Receiver, RecvTimeoutError, Sender, TryRecvError, bounded, unbounded};
use lsp_server::{
    Connection, ErrorCode, Message, Notification as ServerNotification, Request, RequestId,
    Response,
};
use lsp_types::{
    CodeActionKind, CodeActionOptions, CodeActionProviderCapability, CompletionOptions,
    ExecuteCommandOptions, HoverProviderCapability, InitializeParams, InitializeResult,
    InlayHintOptions, InlayHintServerCapabilities, OneOf, PositionEncodingKind, ProgressParams,
    ProgressParamsValue, ProgressToken, PublishDiagnosticsParams, RenameOptions,
    ServerCapabilities, SignatureHelpOptions, TextDocumentSyncCapability, TextDocumentSyncKind,
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
use crate::documents::{DocumentStore, OpenDocumentSnapshot, TextSpan};
use crate::process::ServerKind;
use crate::protocol::diagnostics::{DiagnosticsEvent, DiagnosticsKind};
use crate::protocol::text_document::completion::TRIGGER_CHARACTERS;
use crate::protocol::text_document::signature_help::TRIGGER_CHARACTERS as SIG_HELP_TRIGGER_CHARACTERS;
use crate::protocol::{self, AdapterResult, ResponseAdapter};
use crate::provider::Provider;
use crate::rpc::{DispatchReceipt, Priority, Route, ServerEvent, Service, ServiceError};
use crate::utils::uri_to_file_path;

const DEFAULT_INLAY_HINT_SPAN: u32 = 5_000_000;
const DEFAULT_DAEMON_IDLE_TTL: Duration = Duration::from_secs(30 * 60);

fn current_epoch_seconds() -> u64 {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(duration) => duration.as_secs(),
        Err(_) => 0,
    }
}

fn idle_sweep_interval(idle_ttl: Duration) -> Duration {
    let min_interval = Duration::from_secs(5);
    let max_interval = Duration::from_secs(60);
    let mut interval = Duration::from_secs(idle_ttl.as_secs().saturating_div(2));
    if interval < min_interval {
        interval = min_interval;
    }
    if interval > max_interval {
        interval = max_interval;
    }
    interval
}

/// Runs the LSP server over stdio. This is the entry-point Neovim (or any LSP
/// client) will execute.
pub fn run_stdio_server() -> anyhow::Result<()> {
    let _ = env_logger::try_init();
    let registry = ProjectRegistry::new(None);
    let (connection, io_threads) = Connection::stdio();
    run_session(connection, &registry)?;
    io_threads.join()?;

    Ok(())
}

#[derive(Debug)]
pub struct DaemonConfig {
    pub listen: Option<std::net::SocketAddr>,
    pub socket: Option<PathBuf>,
    pub idle_ttl: Option<Duration>,
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            listen: None,
            socket: None,
            idle_ttl: Some(DEFAULT_DAEMON_IDLE_TTL),
        }
    }
}

pub fn run_daemon_server(config: DaemonConfig) -> anyhow::Result<()> {
    let _ = env_logger::try_init();
    if config.listen.is_some() && config.socket.is_some() {
        return Err(anyhow!("daemon listen and socket cannot be used together"));
    }

    let registry = ProjectRegistry::new(config.idle_ttl);

    if let Some(socket_path) = config.socket {
        return run_daemon_unix(socket_path, registry);
    }

    let addr = config
        .listen
        .unwrap_or_else(|| "127.0.0.1:0".parse().expect("valid default addr"));
    run_daemon_tcp(addr, registry)
}

fn run_daemon_tcp(addr: std::net::SocketAddr, registry: ProjectRegistry) -> anyhow::Result<()> {
    let listener = TcpListener::bind(addr).context("bind daemon listener")?;
    let bound = listener
        .local_addr()
        .context("resolve daemon listen addr")?;
    log::info!("daemon listening on {bound}");

    loop {
        let (stream, peer) = match listener.accept() {
            Ok(accepted) => accepted,
            Err(err) => {
                log::warn!("daemon accept failed: {err}");
                continue;
            }
        };
        log::info!("daemon accepted connection from {peer}");
        if let Err(err) = stream.set_nodelay(true) {
            log::debug!("failed to set TCP_NODELAY for {peer}: {err}");
        }
        let registry = registry.clone();
        thread::spawn(move || {
            if let Err(err) = run_stream_session(stream, registry) {
                log::warn!("session from {peer} exited with error: {err:?}");
            }
        });
    }
}

#[cfg(unix)]
fn run_daemon_unix(socket_path: PathBuf, registry: ProjectRegistry) -> anyhow::Result<()> {
    use std::fs;
    use std::os::unix::net::UnixListener;

    if socket_path.exists() {
        fs::remove_file(&socket_path)
            .with_context(|| format!("remove existing socket {}", socket_path.display()))?;
    }
    let listener = UnixListener::bind(&socket_path)
        .with_context(|| format!("bind unix socket {}", socket_path.display()))?;
    log::info!("daemon listening on {}", socket_path.display());

    loop {
        let (stream, _) = match listener.accept() {
            Ok(accepted) => accepted,
            Err(err) => {
                log::warn!("daemon accept failed: {err}");
                continue;
            }
        };
        log::info!("daemon accepted unix connection");
        let registry = registry.clone();
        thread::spawn(move || {
            if let Err(err) = run_unix_stream_session(stream, registry) {
                log::warn!("unix session exited with error: {err:?}");
            }
        });
    }
}

#[cfg(not(unix))]
fn run_daemon_unix(_socket_path: PathBuf, _registry: ProjectRegistry) -> anyhow::Result<()> {
    Err(anyhow!("unix domain sockets are not supported on this platform"))
}

fn run_stream_session(stream: TcpStream, registry: ProjectRegistry) -> anyhow::Result<()> {
    let (connection, io_threads) = connection_from_stream(stream)?;
    run_session(connection, &registry)?;
    io_threads.join().context("daemon session IO threads failed")?;
    Ok(())
}

#[cfg(unix)]
fn run_unix_stream_session(
    stream: std::os::unix::net::UnixStream,
    registry: ProjectRegistry,
) -> anyhow::Result<()> {
    let (connection, io_threads) = connection_from_stream(stream)?;
    run_session(connection, &registry)?;
    io_threads.join().context("daemon session IO threads failed")?;
    Ok(())
}

trait CloneableStream: io::Read + io::Write + Send + 'static + Sized {
    fn try_clone(&self) -> io::Result<Self>;
}

impl CloneableStream for TcpStream {
    fn try_clone(&self) -> io::Result<Self> {
        TcpStream::try_clone(self)
    }
}

#[cfg(unix)]
impl CloneableStream for std::os::unix::net::UnixStream {
    fn try_clone(&self) -> io::Result<Self> {
        std::os::unix::net::UnixStream::try_clone(self)
    }
}

fn connection_from_stream<S: CloneableStream>(stream: S) -> anyhow::Result<(Connection, DaemonIo)> {
    let reader_stream = stream.try_clone().context("clone daemon stream")?;
    let (reader_sender, reader_receiver) = bounded::<Message>(0);
    let reader = thread::spawn(move || {
        let mut buf_read = BufReader::new(reader_stream);
        while let Some(msg) = Message::read(&mut buf_read)? {
            let is_exit = matches!(&msg, Message::Notification(n) if n.method == "exit");
            if reader_sender.send(msg).is_err() {
                break;
            }
            if is_exit {
                break;
            }
        }
        Ok(())
    });

    let (writer_sender, writer_receiver) = bounded::<Message>(0);
    let (drop_sender, drop_receiver) = bounded::<Message>(0);
    let writer = thread::spawn(move || {
        let mut stream = stream;
        writer_receiver.into_iter().try_for_each(|msg| {
            let result = msg.write(&mut stream);
            let _ = drop_sender.send(msg);
            result
        })
    });
    let dropper = thread::spawn(move || drop_receiver.into_iter().for_each(drop));

    Ok((
        Connection {
            sender: writer_sender,
            receiver: reader_receiver,
        },
        DaemonIo {
            reader,
            writer,
            dropper,
        },
    ))
}

struct DaemonIo {
    reader: thread::JoinHandle<io::Result<()>>,
    writer: thread::JoinHandle<io::Result<()>>,
    dropper: thread::JoinHandle<()>,
}

impl DaemonIo {
    fn join(self) -> io::Result<()> {
        match self.reader.join() {
            Ok(r) => r?,
            Err(err) => std::panic::panic_any(err),
        }
        match self.dropper.join() {
            Ok(_) => (),
            Err(err) => std::panic::panic_any(err),
        }
        match self.writer.join() {
            Ok(r) => r,
            Err(err) => std::panic::panic_any(err),
        }
    }
}

// ==============================================================================
// Project Registry And Shared Tsserver Service
// ==============================================================================

#[derive(Clone)]
struct ProjectRegistry {
    inner: Arc<Mutex<ProjectRegistryState>>,
}

struct ProjectRegistryState {
    entries: HashMap<PathBuf, ProjectEntry>,
    max_entries: Option<usize>,
    #[allow(dead_code)]
    idle_ttl: Option<Duration>,
}

impl ProjectRegistry {
    fn new(idle_ttl: Option<Duration>) -> Self {
        let registry = Self {
            inner: Arc::new(Mutex::new(ProjectRegistryState {
                entries: HashMap::new(),
                max_entries: None,
                idle_ttl,
            })),
        };
        registry.spawn_eviction_loop();
        registry
    }

    fn register_session(&self, params: &InitializeParams) -> anyhow::Result<SessionInit> {
        let workspace_root =
            workspace_root_from_params(params).unwrap_or_else(|| std::env::current_dir().unwrap());
        let mut config = Config::new(PluginSettings::default());

        if let Some(options) = params.initialization_options.as_ref()
            && config.apply_workspace_settings(options)
        {
            log::info!("applied initializationOptions to ts-bridge settings");
        }

        let handle = self.get_or_create(workspace_root.clone(), config.clone())?;
        let registration = handle.register_session(config)?;
        Ok(SessionInit {
            project: handle,
            events: registration.events,
            config: registration.config,
            workspace_root,
            session_id: registration.session_id,
        })
    }

    fn get_or_create(
        &self,
        workspace_root: PathBuf,
        config: Config,
    ) -> anyhow::Result<ProjectHandle> {
        let normalized = normalize_root(workspace_root);
        let mut guard = self
            .inner
            .lock()
            .expect("project registry mutex poisoned");
        guard.maybe_evict();
        if let Some(entry) = guard.entries.get_mut(&normalized) {
            entry.touch();
            return Ok(entry.handle.clone());
        }

        let provider = Provider::new(normalized.clone());
        let last_used = Arc::new(AtomicU64::new(current_epoch_seconds()));
        let session_count = Arc::new(AtomicUsize::new(0));
        let handle = ProjectHandle::spawn(
            normalized.clone(),
            config,
            provider,
            last_used.clone(),
            session_count.clone(),
        );
        let entry = ProjectEntry {
            handle: handle.clone(),
            last_used,
            session_count,
        };
        guard.entries.insert(normalized, entry);
        Ok(handle)
    }

    fn spawn_eviction_loop(&self) {
        let idle_ttl = {
            let guard = self
                .inner
                .lock()
                .expect("project registry mutex poisoned");
            guard.idle_ttl
        };
        let Some(idle_ttl) = idle_ttl else {
            return;
        };
        if idle_ttl.is_zero() {
            return;
        }
        let registry = self.clone();
        thread::spawn(move || registry.evict_idle_loop(idle_ttl));
    }

    fn evict_idle_loop(self, idle_ttl: Duration) {
        let sweep_interval = idle_sweep_interval(idle_ttl);
        loop {
            thread::sleep(sweep_interval);
            let mut guard = self
                .inner
                .lock()
                .expect("project registry mutex poisoned");
            guard.maybe_evict();
        }
    }
}

struct ProjectEntry {
    handle: ProjectHandle,
    last_used: Arc<AtomicU64>,
    session_count: Arc<AtomicUsize>,
}

impl ProjectEntry {
    fn touch(&self) {
        self.last_used
            .store(current_epoch_seconds(), Ordering::Relaxed);
    }
}

impl ProjectRegistryState {
    fn maybe_evict(&mut self) {
        self.evict_idle_entries();
        self.evict_overflow_entries();
    }

    fn evict_idle_entries(&mut self) {
        let Some(idle_ttl) = self.idle_ttl else {
            return;
        };
        if idle_ttl.is_zero() {
            return;
        }
        let ttl_secs = idle_ttl.as_secs();
        let now = current_epoch_seconds();
        let mut expired = Vec::new();
        for (root, entry) in self.entries.iter() {
            if entry.session_count.load(Ordering::Relaxed) > 0 {
                continue;
            }
            let last_used = entry.last_used.load(Ordering::Relaxed);
            if now.saturating_sub(last_used) >= ttl_secs {
                expired.push(root.clone());
            }
        }
        for root in expired {
            if let Some(entry) = self.entries.remove(&root) {
                entry.handle.shutdown();
            }
        }
    }

    fn evict_overflow_entries(&mut self) {
        let Some(max_entries) = self.max_entries else {
            return;
        };
        if self.entries.len() <= max_entries {
            return;
        }

        let mut candidates = self
            .entries
            .iter()
            .map(|(root, entry)| (entry.last_used.load(Ordering::Relaxed), root.clone()))
            .collect::<Vec<_>>();
        candidates.sort_by_key(|(last_used, _)| *last_used);
        for (_, root) in candidates.into_iter().take(self.entries.len() - max_entries) {
            if let Some(entry) = self.entries.remove(&root) {
                entry.handle.shutdown();
            }
        }
    }
}

fn normalize_root(root: PathBuf) -> PathBuf {
    root.canonicalize().unwrap_or(root)
}

#[derive(Clone)]
struct ProjectHandle {
    root: PathBuf,
    label: String,
    commands: Sender<ProjectCommand>,
    last_used: Arc<AtomicU64>,
    session_count: Arc<AtomicUsize>,
}

impl ProjectHandle {
    fn spawn(
        root: PathBuf,
        config: Config,
        provider: Provider,
        last_used: Arc<AtomicU64>,
        session_count: Arc<AtomicUsize>,
    ) -> Self {
        let label = friendly_project_name(&root);
        let (tx, rx) = unbounded();
        let label_clone = label.clone();
        thread::spawn(move || project_thread(config, provider, label_clone, rx));
        Self {
            root,
            label,
            commands: tx,
            last_used,
            session_count,
        }
    }

    fn register_session(&self, config: Config) -> anyhow::Result<SessionRegistration> {
        let session_id = next_session_id();
        let (event_tx, event_rx) = unbounded();
        let (reply_tx, reply_rx) = bounded(0);
        self.commands
            .send(ProjectCommand::RegisterSession {
                session_id,
                sender: event_tx,
                config,
                reply: reply_tx,
            })
            .context("register session with project service")?;
        let confirmed = reply_rx
            .recv()
            .context("receive project session configuration")?;
        self.session_started();
        Ok(SessionRegistration {
            session_id,
            events: event_rx,
            config: confirmed,
        })
    }

    fn unregister_session(&self, session_id: SessionId) {
        let _ = self
            .commands
            .send(ProjectCommand::UnregisterSession { session_id });
        self.session_ended();
    }

    fn dispatch_request(
        &self,
        route: Route,
        payload: Value,
        priority: Priority,
    ) -> anyhow::Result<Vec<DispatchReceipt>> {
        self.touch();
        let (reply_tx, reply_rx) = bounded(0);
        self.commands
            .send(ProjectCommand::Dispatch {
                route,
                payload,
                priority,
                reply: reply_tx,
            })
            .context("dispatch request to project service")?;
        reply_rx
            .recv()
            .context("receive project dispatch receipt")?
            .map_err(|err| anyhow!(err))
    }

    fn update_config(&self, settings: Value) -> anyhow::Result<ConfigUpdate> {
        self.touch();
        let (reply_tx, reply_rx) = bounded(0);
        self.commands
            .send(ProjectCommand::UpdateConfig {
                settings,
                reply: reply_tx,
            })
            .context("update project configuration")?;
        reply_rx.recv().context("receive configuration update")
    }

    fn restart(&self, kind: RestartKind) -> anyhow::Result<()> {
        self.touch();
        let (reply_tx, reply_rx) = bounded(0);
        self.commands
            .send(ProjectCommand::Restart { kind, reply: reply_tx })
            .context("dispatch project restart")?;
        reply_rx
            .recv()
            .context("receive project restart result")?
            .map_err(|err| anyhow!(err))
    }

    fn shutdown(&self) {
        let _ = self.commands.send(ProjectCommand::Shutdown);
    }

    fn root(&self) -> &Path {
        &self.root
    }

    fn label(&self) -> &str {
        &self.label
    }

    fn touch(&self) {
        self.last_used
            .store(current_epoch_seconds(), Ordering::Relaxed);
    }

    fn session_started(&self) {
        self.session_count.fetch_add(1, Ordering::Relaxed);
        self.touch();
    }

    fn session_ended(&self) {
        let previous = self
            .session_count
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
                if value == 0 {
                    None
                } else {
                    Some(value - 1)
                }
            })
            .unwrap_or(0);
        if previous == 1 {
            self.touch();
        }
    }
}

struct SessionInit {
    project: ProjectHandle,
    events: Receiver<ProjectEvent>,
    config: Config,
    workspace_root: PathBuf,
    session_id: SessionId,
}

struct SessionRegistration {
    session_id: SessionId,
    events: Receiver<ProjectEvent>,
    config: Config,
}

struct ConfigUpdate {
    changed: bool,
    config: Config,
}

#[derive(Debug, Clone)]
enum ProjectEvent {
    Server(ServerEvent),
    Restarting { kind: RestartKind },
    Restarted { kind: RestartKind },
    RestartFailed { kind: RestartKind, message: String },
    ConfigUpdated(Config),
}

#[derive(Debug, Clone, Copy)]
enum RestartKind {
    Syntax,
    Semantic,
    Both,
}

impl RestartKind {
    fn from_str(value: &str) -> Option<Self> {
        match value {
            "syntax" => Some(Self::Syntax),
            "semantic" => Some(Self::Semantic),
            "both" => Some(Self::Both),
            _ => None,
        }
    }

    fn as_flags(self) -> (bool, bool) {
        match self {
            Self::Syntax => (true, false),
            Self::Semantic => (false, true),
            Self::Both => (true, true),
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Syntax => "syntax",
            Self::Semantic => "semantic",
            Self::Both => "both",
        }
    }
}

enum ProjectCommand {
    RegisterSession {
        session_id: SessionId,
        sender: Sender<ProjectEvent>,
        config: Config,
        reply: Sender<Config>,
    },
    UnregisterSession {
        session_id: SessionId,
    },
    Dispatch {
        route: Route,
        payload: Value,
        priority: Priority,
        reply: Sender<Result<Vec<DispatchReceipt>, ServiceError>>,
    },
    UpdateConfig {
        settings: Value,
        reply: Sender<ConfigUpdate>,
    },
    Restart {
        kind: RestartKind,
        reply: Sender<Result<(), ServiceError>>,
    },
    Shutdown,
}

type SessionId = u64;

static SESSION_IDS: AtomicU64 = AtomicU64::new(1);

fn next_session_id() -> SessionId {
    SESSION_IDS.fetch_add(1, Ordering::Relaxed)
}

fn project_thread(config: Config, provider: Provider, label: String, rx: Receiver<ProjectCommand>) {
    let mut service = Service::new(config.clone(), provider);
    let mut config = config;
    let mut sessions: HashMap<SessionId, Sender<ProjectEvent>> = HashMap::new();
    let poll_interval = Duration::from_millis(10);

    loop {
        for event in service.poll_responses() {
            broadcast_event(&mut sessions, ProjectEvent::Server(event));
        }

        let command = match rx.recv_timeout(poll_interval) {
            Ok(command) => command,
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => break,
        };

        if !handle_project_command(
            command,
            &mut service,
            &mut config,
            &mut sessions,
            &label,
        ) {
            break;
        }
        while let Ok(command) = rx.try_recv() {
            if !handle_project_command(
                command,
                &mut service,
                &mut config,
                &mut sessions,
                &label,
            ) {
                return;
            }
        }
    }
}

fn handle_project_command(
    command: ProjectCommand,
    service: &mut Service,
    config: &mut Config,
    sessions: &mut HashMap<SessionId, Sender<ProjectEvent>>,
    label: &str,
) -> bool {
    match command {
        ProjectCommand::RegisterSession {
            session_id,
            sender,
            config: session_config,
            reply,
        } => {
            if session_config != *config {
                log::warn!(
                    "session config mismatch for project {label}; using first session settings"
                );
            }
            sessions.insert(session_id, sender);
            let _ = reply.send(config.clone());
            true
        }
        ProjectCommand::UnregisterSession { session_id } => {
            sessions.remove(&session_id);
            true
        }
        ProjectCommand::Dispatch {
            route,
            payload,
            priority,
            reply,
        } => {
            let result = service.dispatch_request(route, payload, priority);
            let _ = reply.send(result);
            true
        }
        ProjectCommand::UpdateConfig { settings, reply } => {
            let changed = config.apply_workspace_settings(&settings);
            if changed {
                log::info!("project {label} settings updated");
                service.update_config(config.clone());
                broadcast_event(sessions, ProjectEvent::ConfigUpdated(config.clone()));
            }
            let _ = reply.send(ConfigUpdate {
                changed,
                config: config.clone(),
            });
            true
        }
        ProjectCommand::Restart { kind, reply } => {
            broadcast_event(sessions, ProjectEvent::Restarting { kind });
            let (restart_syntax, restart_semantic) = kind.as_flags();
            let result = service.restart(restart_syntax, restart_semantic);
            match &result {
                Ok(_) => broadcast_event(sessions, ProjectEvent::Restarted { kind }),
                Err(err) => broadcast_event(
                    sessions,
                    ProjectEvent::RestartFailed {
                        kind,
                        message: err.to_string(),
                    },
                ),
            }
            let _ = reply.send(result);
            true
        }
        ProjectCommand::Shutdown => {
            sessions.clear();
            false
        }
    }
}

fn broadcast_event(sessions: &mut HashMap<SessionId, Sender<ProjectEvent>>, event: ProjectEvent) {
    let mut stale = Vec::new();
    for (session_id, sender) in sessions.iter() {
        if sender.send(event.clone()).is_err() {
            stale.push(*session_id);
        }
    }
    for session_id in stale {
        sessions.remove(&session_id);
    }
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
    let execute_command_provider = Some(ExecuteCommandOptions {
        commands: crate::protocol::workspace::execute_command::USER_COMMANDS
            .iter()
            .map(|cmd| (*cmd).to_string())
            .collect(),
        work_done_progress_options: Default::default(),
    });
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
        execute_command_provider,
        text_document_sync: Some(TextDocumentSyncCapability::Options(text_sync)),
        ..Default::default()
    }
}

fn run_session(connection: Connection, registry: &ProjectRegistry) -> anyhow::Result<()> {
    let (init_id, init_params) = connection
        .initialize_start()
        .context("waiting for initialize")?;
    let params: InitializeParams =
        serde_json::from_value(init_params).context("invalid initialize params")?;

    let session_init = registry.register_session(&params)?;
    let capabilities = advertised_capabilities(session_init.config.plugin());
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

    let mut session = SessionState::new(connection, session_init);
    let result = session.run();
    session
        .project
        .unregister_session(session.session_id);
    result
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

struct SessionState {
    connection: Connection,
    project: ProjectHandle,
    events: Receiver<ProjectEvent>,
    config: Config,
    workspace_root: PathBuf,
    session_id: SessionId,
    project_label: String,
    pending: PendingRequests,
    diag_state: DiagnosticsState,
    progress: LoadingProgress,
    restart_progress: RestartProgress,
    documents: DocumentStore,
    inlay_cache: InlayHintCache,
    inlay_preferences: InlayPreferenceState,
}

impl SessionState {
    fn new(connection: Connection, init: SessionInit) -> Self {
        let project_label = init.project.label().to_string();
        Self {
            connection,
            project: init.project,
            events: init.events,
            config: init.config,
            workspace_root: init.workspace_root,
            session_id: init.session_id,
            project_label,
            pending: PendingRequests::default(),
            diag_state: DiagnosticsState::default(),
            progress: LoadingProgress::new(init.session_id),
            restart_progress: RestartProgress::new(init.session_id),
            documents: DocumentStore::default(),
            inlay_cache: InlayHintCache::default(),
            inlay_preferences: InlayPreferenceState::default(),
        }
    }

    fn run(&mut self) -> anyhow::Result<()> {
        if let Err(err) = self.progress.begin(
            &self.connection,
            "ts-bridge",
            &format!("Booting {}", self.project_label),
        ) {
            log::debug!("work-done progress begin failed: {err:?}");
        }

        let poll_interval = Duration::from_millis(10);
        loop {
            self.drain_project_events()?;

            match self.connection.receiver.recv_timeout(poll_interval) {
                Ok(message) => match message {
                    Message::Request(req) => {
                        if self.handle_request(req)? {
                            break;
                        }
                    }
                    Message::Response(resp) => {
                        log::debug!("ignoring stray response: {:?}", resp);
                    }
                    Message::Notification(notif) => {
                        if self.handle_notification(notif)? {
                            break;
                        }
                    }
                },
                Err(RecvTimeoutError::Timeout) => continue,
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }

        Ok(())
    }

    fn drain_project_events(&mut self) -> anyhow::Result<()> {
        loop {
            match self.events.try_recv() {
                Ok(event) => self.handle_project_event(event)?,
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    log::warn!("project event channel closed for {}", self.project_label);
                    break;
                }
            }
        }
        Ok(())
    }

    fn handle_project_event(&mut self, event: ProjectEvent) -> anyhow::Result<()> {
        match event {
            ProjectEvent::Server(event) => self.handle_server_event(event),
            ProjectEvent::ConfigUpdated(config) => {
                self.config = config;
                self.inlay_preferences.invalidate();
                self.inlay_cache.clear();
                Ok(())
            }
            ProjectEvent::Restarting { kind } => self.handle_restart_start(kind),
            ProjectEvent::Restarted { kind } => self.handle_restart_complete(kind),
            ProjectEvent::RestartFailed { kind, message } => {
                self.handle_restart_failure(kind, &message)
            }
        }
    }

    fn handle_server_event(&mut self, event: ServerEvent) -> anyhow::Result<()> {
        if let Some(diag_event) = protocol::diagnostics::parse_tsserver_event(&event.payload) {
            let stage_label = match &diag_event {
                DiagnosticsEvent::Report { kind, .. } => Some(stage_text(*kind)),
                DiagnosticsEvent::Completed { .. } => Some("finalizing diagnostics"),
            };
            self.diag_state.handle_event(event.server, diag_event);
            while let Some((uri, diagnostics)) = self.diag_state.take_ready() {
                if !self.documents.is_open(&uri) {
                    self.diag_state.clear_file(&uri);
                    continue;
                }
                publish_diagnostics(
                    &self.connection,
                    PublishDiagnosticsParams {
                        uri,
                        diagnostics,
                        version: None,
                    },
                )?;
            }
            if self.diag_state.has_pending() {
                let message = if let Some(stage) = stage_label {
                    format!("Analyzing {} — {stage}", self.project_label)
                } else {
                    format!("Analyzing {}", self.project_label)
                };
                if let Err(err) =
                    self.progress
                        .report(&self.connection, &message, self.diag_state.progress_percent())
                {
                    log::debug!("work-done progress report failed: {err:?}");
                }
            } else {
                if let Err(err) = self.progress.end(
                    &self.connection,
                    &format!("Language features ready in {}", self.project_label),
                ) {
                    log::debug!("work-done progress end failed: {err:?}");
                }
                self.diag_state.reset_if_idle();
            }
            return Ok(());
        }

        if let Some(response) =
            self.pending
                .resolve(event.server, &event.payload, &mut self.inlay_cache, &self.project)?
        {
            self.connection.sender.send(response.into())?;
        } else {
            log::trace!("tsserver {:?} -> {}", event.server, event.payload);
        }
        Ok(())
    }

    fn handle_notification(
        &mut self,
        notif: ServerNotification,
    ) -> anyhow::Result<bool> {
        if notif.method == "exit" {
            return Ok(true);
        }
        if notif.method == "ts-bridge/control" {
            self.handle_control_notification(notif.params)?;
            return Ok(false);
        }
        if notif.method == DidOpenTextDocument::METHOD {
            let params: crate::types::DidOpenTextDocumentParams =
                serde_json::from_value(notif.params)?;
            if let Ok(uri) = lsp_types::Uri::from_str(&params.text_document.uri) {
                self.documents.open(
                    &uri,
                    &params.text_document.text,
                    Some(params.text_document.version),
                    params.text_document.language_id.clone(),
                );
                self.inlay_cache.invalidate(&uri);
            }
            let file_for_diagnostics = uri_to_file_path(params.text_document.uri.as_str())
                .unwrap_or_else(|| params.text_document.uri.to_string());
            let spec = crate::protocol::text_document::did_open::handle(
                params,
                &self.workspace_root,
            );
            if let Err(err) =
                self.project
                    .dispatch_request(spec.route, spec.payload, spec.priority)
            {
                log::warn!("failed to dispatch didOpen: {err}");
            }
            self.request_file_diagnostics(&file_for_diagnostics);
            if let Err(err) = self.progress.report(
                &self.connection,
                &format!("Analyzing {} — scheduling diagnostics", self.project_label),
                self.diag_state.progress_percent(),
            ) {
                log::debug!("work-done progress report failed: {err:?}");
            }
            return Ok(false);
        }
        if notif.method == DidChangeTextDocument::METHOD {
            let params: crate::types::DidChangeTextDocumentParams =
                serde_json::from_value(notif.params)?;
            if let Ok(uri) = lsp_types::Uri::from_str(&params.text_document.uri) {
                self.documents.apply_changes(
                    &uri,
                    &params.content_changes,
                    params.text_document.version,
                );
                self.inlay_cache.invalidate(&uri);
            }
            let file_for_diagnostics = uri_to_file_path(params.text_document.uri.as_str())
                .unwrap_or_else(|| params.text_document.uri.to_string());
            let spec = crate::protocol::text_document::did_change::handle(
                params,
                &self.workspace_root,
            );
            if let Err(err) =
                self.project
                    .dispatch_request(spec.route, spec.payload, spec.priority)
            {
                log::warn!("failed to dispatch didChange: {err}");
            }
            self.request_file_diagnostics(&file_for_diagnostics);
            if let Err(err) = self.progress.report(
                &self.connection,
                &format!("Analyzing {} — scheduling diagnostics", self.project_label),
                self.diag_state.progress_percent(),
            ) {
                log::debug!("work-done progress report failed: {err:?}");
            }
            return Ok(false);
        }
        if notif.method == DidCloseTextDocument::METHOD {
            let params: crate::types::DidCloseTextDocumentParams =
                serde_json::from_value(notif.params)?;
            let uri = params.text_document.uri.clone();
            if let Ok(parsed) = lsp_types::Uri::from_str(&uri) {
                self.documents.close(&parsed);
                self.inlay_cache.invalidate(&parsed);
                self.diag_state.clear_file(&parsed);
            }
            let spec = crate::protocol::text_document::did_close::handle(
                params,
                &self.workspace_root,
            );
            if let Err(err) =
                self.project
                    .dispatch_request(spec.route, spec.payload, spec.priority)
            {
                log::warn!("failed to dispatch didClose: {err}");
            }
            clear_client_diagnostics(&self.connection, uri)?;
            return Ok(false);
        }
        if notif.method == DidChangeConfiguration::METHOD {
            let params: lsp_types::DidChangeConfigurationParams =
                serde_json::from_value(notif.params)?;
            let update = self.project.update_config(params.settings)?;
            self.config = update.config;
            if update.changed {
                log::info!("workspace settings reloaded from didChangeConfiguration");
                self.inlay_preferences.invalidate();
                // TODO: restart auxiliary tsserver processes when toggles require it.
            }
            return Ok(false);
        }
        if let Some(spec) = protocol::route_notification(&notif.method, notif.params.clone())
        {
            if let Err(err) =
                self.project
                    .dispatch_request(spec.route, spec.payload, spec.priority)
            {
                log::warn!("failed to dispatch notification {}: {err}", notif.method);
            }
        } else {
            log::debug!("notification {} ignored", notif.method);
        }
        Ok(false)
    }

    fn handle_control_notification(&mut self, params: Value) -> anyhow::Result<()> {
        let Some(action) = params.get("action").and_then(|value| value.as_str()) else {
            log::warn!("control notification missing action");
            return Ok(());
        };
        if action != "restart" {
            log::warn!("control notification action {action} ignored");
            return Ok(());
        }

            let restart = match parse_restart_request(Some(&params)) {
                Ok(restart) => restart,
                Err(err) => {
                    log::warn!("control restart params invalid: {err}");
                    return Ok(());
                }
            };
        if let Some(root_uri) = &restart.root_uri {
            if !self.matches_root_uri(root_uri) {
                    log::warn!(
                        "restart request ignored for non-matching root {}",
                        root_uri.as_str()
                    );
                return Ok(());
            }
        }
        if let Err(err) = self.project.restart(restart.kind) {
            log::warn!("control restart failed: {err}");
        }
        Ok(())
    }

    fn handle_request(&mut self, req: Request) -> anyhow::Result<bool> {
        let lsp_server::Request { id, method, params } = req;

        if method == "shutdown" {
            let response = Response::new_ok(id, Value::Null);
            self.connection.sender.send(response.into())?;
            return Ok(true);
        }

        if method == "initialize" {
            // Already handled via initialize_start, but the client might resend; respond with error.
            let response = Response::new_err(
                id,
                ErrorCode::InvalidRequest as i32,
                "initialize already completed".to_string(),
            );
            self.connection.sender.send(response.into())?;
            return Ok(false);
        }

        if method == InlayHintRefreshRequest::METHOD {
            self.inlay_cache.clear();
            let response = Response::new_ok(id, Value::Null);
            self.connection.sender.send(response.into())?;
            return Ok(false);
        }

        if method == lsp_types::request::ExecuteCommand::METHOD {
            let command_params: lsp_types::ExecuteCommandParams =
                serde_json::from_value(params.clone())
                    .context("invalid execute command params")?;
            if command_params.command == "TSBRestartProject" {
                self.handle_restart_command(id, command_params)?;
                return Ok(false);
            }
        }

        let params_value = params;
        let spec: Option<protocol::RequestSpec>;
        let mut postprocess = None;

        if method == InlayHintRequest::METHOD {
            let enabled = self.config.plugin().enable_inlay_hints;
            self.inlay_preferences.ensure(&self.config, &self.project)?;
            if !enabled {
                let response = Response::new_ok(id, Value::Array(Vec::new()));
                self.connection.sender.send(response.into())?;
                return Ok(false);
            }
            let hint_params: lsp_types::InlayHintParams =
                serde_json::from_value(params_value.clone())
                    .context("invalid inlay hint params")?;
            if let Some(cached) = self.inlay_cache.lookup(&hint_params) {
                let response = Response::new_ok(id, serde_json::to_value(cached)?);
                self.connection.sender.send(response.into())?;
                return Ok(false);
            }
            let span = self
                .documents
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
            match self
                .project
                .dispatch_request(spec.route, spec.payload, spec.priority)
            {
                Ok(receipts) => {
                    if let Some(adapter) = spec.on_response {
                        if receipts.is_empty() {
                            let response = Response::new_err(
                                id,
                                ErrorCode::InternalError as i32,
                                "tsserver route produced no requests".to_string(),
                            );
                            self.connection.sender.send(response.into())?;
                        } else {
                            self.pending.track(
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
                        self.connection.sender.send(response.into())?;
                    }
                }
                Err(err) => {
                    let response = Response::new_err(
                        id,
                        ErrorCode::InternalError as i32,
                        format!("failed to dispatch tsserver request: {err}"),
                    );
                    self.connection.sender.send(response.into())?;
                }
            }
            return Ok(false);
        }

        let response = Response::new_err(
            id,
            ErrorCode::MethodNotFound as i32,
            format!("method {method} is not implemented yet"),
        );
        self.connection.sender.send(response.into())?;

        Ok(false)
    }

    fn handle_restart_command(
        &mut self,
        id: RequestId,
        params: lsp_types::ExecuteCommandParams,
    ) -> anyhow::Result<()> {
            let restart = match parse_restart_request(params.arguments.first()) {
                Ok(restart) => restart,
                Err(err) => {
                    let response = Response::new_err(
                        id,
                        ErrorCode::InvalidParams as i32,
                        format!("invalid restart command arguments: {err}"),
                    );
                    self.connection.sender.send(response.into())?;
                    return Ok(());
                }
            };
        if let Some(root_uri) = &restart.root_uri {
            if !self.matches_root_uri(root_uri) {
                let response = Response::new_err(
                    id,
                    ErrorCode::InvalidParams as i32,
                        format!(
                            "rootUri {} does not match this session",
                            root_uri.as_str()
                        ),
                );
                self.connection.sender.send(response.into())?;
                return Ok(());
            }
        }

        match self.project.restart(restart.kind) {
            Ok(()) => {
                let response = Response::new_ok(id, Value::Null);
                self.connection.sender.send(response.into())?;
            }
            Err(err) => {
                let response = Response::new_err(
                    id,
                    ErrorCode::InternalError as i32,
                    format!("failed to restart project: {err}"),
                );
                self.connection.sender.send(response.into())?;
            }
        }

        Ok(())
    }

    fn handle_restart_start(&mut self, kind: RestartKind) -> anyhow::Result<()> {
        let responses = self
            .pending
            .fail_all("tsserver restart canceled outstanding requests");
        for response in responses {
            self.connection.sender.send(response.into())?;
        }

        self.diag_state.clear();
        self.inlay_cache.clear();
        self.inlay_preferences.invalidate();
        if let Err(err) = self.restart_progress.begin(
            &self.connection,
            "Restarting TypeScript server",
            kind,
        ) {
            log::debug!("restart progress begin failed: {err:?}");
        }
        Ok(())
    }

    fn handle_restart_complete(&mut self, kind: RestartKind) -> anyhow::Result<()> {
        self.reopen_documents()?;
        if let Err(err) = self.restart_progress.end(
            &self.connection,
            "TypeScript server restarted",
            kind,
        ) {
            log::debug!("restart progress end failed: {err:?}");
        }
        Ok(())
    }

    fn handle_restart_failure(&mut self, kind: RestartKind, message: &str) -> anyhow::Result<()> {
        if let Err(err) = self.restart_progress.end(
            &self.connection,
            "TypeScript server restart failed",
            kind,
        ) {
            log::debug!("restart progress end failed: {err:?}");
        }
        show_message(
            &self.connection,
            &format!("ts-bridge restart failed: {message}"),
            lsp_types::MessageType::ERROR,
        )?;
        Ok(())
    }

    fn request_file_diagnostics(&mut self, file: &str) {
        let spec = protocol::diagnostics::request_for_file(file);
        match self
            .project
            .dispatch_request(spec.route, spec.payload, spec.priority)
        {
            Ok(receipts) => {
                for receipt in receipts {
                    self.diag_state
                        .register_pending(receipt.server, receipt.seq);
                }
            }
            Err(err) => {
                log::warn!("failed to dispatch geterr for {}: {err}", file);
            }
        }
    }

    fn reopen_documents(&mut self) -> anyhow::Result<()> {
        let open_documents = self.documents.open_documents();
        for snapshot in open_documents {
            self.reopen_document(snapshot)?;
        }
        Ok(())
    }

    fn reopen_document(&mut self, snapshot: OpenDocumentSnapshot) -> anyhow::Result<()> {
        let params = crate::types::DidOpenTextDocumentParams {
            text_document: crate::types::TextDocumentItem {
                uri: snapshot.uri.clone(),
                language_id: snapshot.language_id,
                version: snapshot.version.unwrap_or(0),
                text: snapshot.text,
            },
        };
        let spec = crate::protocol::text_document::did_open::handle(
            params,
            &self.workspace_root,
        );
        if let Err(err) =
            self.project
                .dispatch_request(spec.route, spec.payload, spec.priority)
        {
            log::warn!("failed to dispatch reopened didOpen: {err}");
            return Ok(());
        }
        let file_for_diagnostics =
            uri_to_file_path(snapshot.uri.as_str()).unwrap_or(snapshot.uri);
        self.request_file_diagnostics(&file_for_diagnostics);
        Ok(())
    }

    fn matches_root_uri(&self, root_uri: &lsp_types::Uri) -> bool {
        let Some(path) = uri_to_file_path(root_uri.as_str()) else {
            return false;
        };
        normalize_root(PathBuf::from(path)) == normalize_root(self.project.root().to_path_buf())
    }
}

struct RestartRequest {
    kind: RestartKind,
    root_uri: Option<lsp_types::Uri>,
}

fn parse_restart_request(value: Option<&Value>) -> anyhow::Result<RestartRequest> {
    let mut root_uri = None;
    let mut kind = RestartKind::Both;

    if let Some(value) = value {
        let obj = value
            .as_object()
            .ok_or_else(|| anyhow!("restart params must be an object"))?;

        if let Some(root_str) = obj.get("rootUri").and_then(|value| value.as_str()) {
            root_uri = Some(
                lsp_types::Uri::from_str(root_str)
                    .context("restart params rootUri must be a URI")?,
            );
        }

        if let Some(kind_str) = obj.get("kind").and_then(|value| value.as_str()) {
            kind = RestartKind::from_str(kind_str)
                .ok_or_else(|| anyhow!("invalid restart kind {kind_str}"))?;
        }
    }

    Ok(RestartRequest { kind, root_uri })
}

fn show_message(
    connection: &Connection,
    message: &str,
    kind: lsp_types::MessageType,
) -> anyhow::Result<()> {
    let params = lsp_types::ShowMessageParams {
        typ: kind,
        message: message.to_string(),
    };
    let notif = ServerNotification::new(
        lsp_types::notification::ShowMessage::METHOD.to_string(),
        serde_json::to_value(params)?,
    );
    connection.sender.send(Message::Notification(notif))?;
    Ok(())
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
        project: &ProjectHandle,
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
                Ok(AdapterResult::Ready(result)) => {
                    if let Some(postprocess) = entry.postprocess {
                        postprocess.apply(&result, inlay_cache)?;
                    }
                    Ok(Some(Response::new_ok(entry.id, result)))
                }
                Ok(AdapterResult::Continue(next_spec)) => {
                    let request_id = entry.id;
                    let postprocess = entry.postprocess;
                    let Some(adapter) = next_spec.on_response else {
                        return Ok(Some(Response::new_err(
                            request_id,
                            ErrorCode::InternalError as i32,
                            "handler missing response adapter".to_string(),
                        )));
                    };
                    match project.dispatch_request(
                        next_spec.route,
                        next_spec.payload,
                        next_spec.priority,
                    ) {
                        Ok(receipts) => {
                            if receipts.is_empty() {
                                Ok(Some(Response::new_err(
                                    request_id,
                                    ErrorCode::InternalError as i32,
                                    "tsserver route produced no requests".to_string(),
                                )))
                            } else {
                                self.track(
                                    &receipts,
                                    request_id,
                                    adapter,
                                    next_spec.response_context,
                                    postprocess,
                                );
                                Ok(None)
                            }
                        }
                        Err(err) => Ok(Some(Response::new_err(
                            request_id,
                            ErrorCode::InternalError as i32,
                            format!("failed to dispatch tsserver request: {err}"),
                        ))),
                    }
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

    fn fail_all(&mut self, message: &str) -> Vec<Response> {
        let mut responses = Vec::new();
        let mut seen = HashSet::new();
        for entry in self.entries.values() {
            if seen.insert(entry.id.clone()) {
                responses.push(Response::new_err(
                    entry.id.clone(),
                    ErrorCode::InternalError as i32,
                    message.to_string(),
                ));
            }
        }
        self.entries.clear();
        responses
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
    fn ensure(&mut self, config: &Config, project: &ProjectHandle) -> anyhow::Result<()> {
        let desired = config.plugin().enable_inlay_hints;
        if self.configured_for == Some(desired) {
            return Ok(());
        }
        self.dispatch(project, desired)?;
        self.configured_for = Some(desired);
        Ok(())
    }

    fn dispatch(&self, project: &ProjectHandle, enabled: bool) -> anyhow::Result<()> {
        let request = json!({
            "command": "configure",
            "arguments": {
                "preferences": crate::protocol::text_document::inlay_hint::preferences(enabled),
            }
        });
        let _ = project
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

    fn clear(&mut self) {
        self.pending.clear();
        self.order.clear();
        self.latest.clear();
        self.ready.clear();
        self.workload.reset();
    }

    fn clear_file(&mut self, uri: &lsp_types::Uri) {
        self.latest.remove(uri);
        self.ready.retain(|(ready_uri, _)| ready_uri != uri);
        for entry in self.pending.values_mut() {
            entry.files.remove(uri);
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
    fn new(session_id: SessionId) -> Self {
        let token = ProgressToken::String(format!(
            "ts-bridge:{}:{session_id}",
            std::process::id()
        ));
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

struct RestartProgress {
    token: ProgressToken,
    created: bool,
    active: bool,
}

impl RestartProgress {
    fn new(session_id: SessionId) -> Self {
        let token = ProgressToken::String(format!(
            "ts-bridge-restart:{}:{session_id}",
            std::process::id()
        ));
        Self {
            token,
            created: false,
            active: false,
        }
    }

    fn begin(
        &mut self,
        connection: &Connection,
        message: &str,
        kind: RestartKind,
    ) -> anyhow::Result<()> {
        if self.active {
            return Ok(());
        }
        self.ensure_token(connection)?;
        let params = ProgressParams {
            token: self.token.clone(),
            value: ProgressParamsValue::WorkDone(LspWorkDoneProgress::Begin(
                WorkDoneProgressBegin {
                    title: "ts-bridge".to_string(),
                    message: Some(format!("{message} ({})", kind.label())),
                    ..WorkDoneProgressBegin::default()
                },
            )),
        };
        send_progress(connection, params)?;
        self.active = true;
        Ok(())
    }

    fn end(
        &mut self,
        connection: &Connection,
        message: &str,
        kind: RestartKind,
    ) -> anyhow::Result<()> {
        if !self.active {
            return Ok(());
        }
        let params = ProgressParams {
            token: self.token.clone(),
            value: ProgressParamsValue::WorkDone(LspWorkDoneProgress::End(WorkDoneProgressEnd {
                message: Some(format!("{message} ({})", kind.label())),
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
