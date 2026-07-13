//! The `import-lint lsp` server (L2, `docs/PLAN.md` §2, decisions E2-E5/E8/E10):
//! a synchronous `lsp-server`/`lsp-types` main loop wrapping the existing
//! [`crate::watch::WatchSession`] engine. Diagnostics are push-only
//! (`textDocument/publishDiagnostics`) for the whole project, not just open
//! documents (E3) — every engine cycle already computes the full set, so the
//! server's only job is diffing it against what it last published per file.
//!
//! Two entry points: [`run_stdio`] for the real binary (`main.rs`'s `lsp`
//! subcommand) and [`run_with_connection`], the testable core driven by
//! `crates/cli/tests/lsp.rs` over `lsp_server::Connection::memory()`.
//!
//! Buffer overlays (open editor content overriding disk, L1) are wired straight
//! into [`WatchSession::set_overlay`]/`clear_overlay` — see that module's doc
//! comment for the path-identity contract this module's [`convert::uri_to_canonical_path`]
//! must uphold.

pub mod convert;

use std::collections::HashMap;
use std::error::Error;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use lsp_server::{
    Connection, ErrorCode, ExtractError, Message, Notification as LspNotification,
    Request as LspRequest, RequestId, Response,
};
use lsp_types::notification::{
    DidChangeTextDocument, DidChangeWatchedFiles, DidCloseTextDocument, DidOpenTextDocument,
    DidSaveTextDocument, Exit, Notification as _, PublishDiagnostics, ShowMessage,
};
use lsp_types::request::{RegisterCapability, Request as _, Shutdown};
use lsp_types::{
    ClientCapabilities, Diagnostic, DidChangeTextDocumentParams, DidChangeWatchedFilesParams,
    DidChangeWatchedFilesRegistrationOptions, DidCloseTextDocumentParams,
    DidOpenTextDocumentParams, DidSaveTextDocumentParams, FileSystemWatcher, GlobPattern,
    InitializeParams, InitializeResult, MessageType, PublishDiagnosticsParams, Registration,
    RegistrationParams, SaveOptions, ServerCapabilities, ServerInfo, ShowMessageParams,
    TextDocumentSyncCapability, TextDocumentSyncKind, TextDocumentSyncOptions,
    TextDocumentSyncSaveOptions, Url,
};

use crate::output::RenderedDiagnostic;
use crate::watch::{ChangeKind, WatchSession, WatchSessionOptions};
use convert::{path_to_uri, to_lsp_diagnostic, uri_to_canonical_path};

/// Options for [`run_stdio`]/[`run_with_connection`].
#[derive(Debug, Clone, Copy)]
pub struct LspOptions {
    /// How long to wait after the last `didChange` before running a cycle
    /// (decision E4). Tests inject a short value to stay fast; production uses
    /// [`LspOptions::default`]'s 200ms.
    pub debounce: Duration,
}

impl Default for LspOptions {
    fn default() -> Self {
        LspOptions {
            debounce: Duration::from_millis(200),
        }
    }
}

/// Run the LSP server over stdio (the real `import-lint lsp` invocation): builds a
/// `Connection::stdio()`, runs [`run_with_connection`] to completion, then joins the
/// reader/writer IO threads so the process doesn't exit with them still flushing.
pub fn run_stdio(options: LspOptions) -> Result<(), Box<dyn Error + Send + Sync>> {
    let (connection, io_threads) = Connection::stdio();
    run_with_connection(connection, options)?;
    io_threads.join()?;
    Ok(())
}

/// The testable core (D-L2): performs the full initialize handshake (D-L3), then
/// drives the main loop until `exit`/a disconnected client. Takes an
/// `lsp_server::Connection` directly so `crates/cli/tests/lsp.rs` can drive the real
/// server loop over `Connection::memory()`, with no stdio involved.
pub fn run_with_connection(
    connection: Connection,
    options: LspOptions,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let (initialize_id, params_value) = connection.initialize_start()?;
    let params: InitializeParams = serde_json::from_value(params_value)?;

    let (workspace_root, multi_root_warning) = resolve_workspace_root(&params);
    let run_mode = parse_run_mode(params.initialization_options.as_ref());
    let register_watched_files = supports_watched_files_registration(&params.capabilities);

    let initialize_result = InitializeResult {
        server_info: Some(ServerInfo {
            name: "import-lint".to_string(),
            version: Some(env!("CARGO_PKG_VERSION").to_string()),
        }),
        capabilities: server_capabilities(),
    };
    // `initialize_finish` (not the one-shot `initialize()`) so we control the full
    // `InitializeResult`, including `serverInfo` (D-L3). It also blocks internally
    // until the client's `initialized` notification arrives, so everything below
    // this call is correctly "after initialized" per decision E8/D-L5.
    connection.initialize_finish(initialize_id, serde_json::to_value(&initialize_result)?)?;

    let mut server = Server::new(connection, options.debounce, workspace_root, run_mode);
    if let Some(message) = multi_root_warning {
        server.show_message(MessageType::WARNING, message);
    }
    // Build the session first (D-L4) so `register_watched_files` can consult its
    // `watch_targets()` for a non-default-named tsconfig (D-L5).
    server.try_build_session();
    if register_watched_files {
        server.register_watched_files();
    }

    server.run()
}

/// `initializationOptions.run` (decision E4): `"onType"` (default) lints on every
/// debounced keystroke, `"onSave"` only on save. Any unrecognized value falls back to
/// `onType` (D-L3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RunMode {
    OnType,
    OnSave,
}

fn parse_run_mode(initialization_options: Option<&serde_json::Value>) -> RunMode {
    match initialization_options
        .and_then(|options| options.get("run"))
        .and_then(|value| value.as_str())
    {
        Some("onSave") => RunMode::OnSave,
        _ => RunMode::OnType,
    }
}

/// Resolve the workspace root (decision E10: first workspace folder wins) and an
/// optional warning message to show once initialized if more than one folder was
/// offered. Falls back to the deprecated `rootUri`, then the server's own cwd.
#[allow(deprecated)]
fn resolve_workspace_root(params: &InitializeParams) -> (PathBuf, Option<String>) {
    if let Some(folders) = &params.workspace_folders
        && let Some(first) = folders.first()
        && let Some(path) = uri_to_dir_path(&first.uri)
    {
        let warning = (folders.len() > 1).then(|| {
            format!(
                "import-lint: multiple workspace folders are open; using {} \
                 (multi-root workspaces are not supported, docs/PLAN.md E10)",
                path.display()
            )
        });
        return (path, warning);
    }
    if let Some(root_uri) = &params.root_uri
        && let Some(path) = uri_to_dir_path(root_uri)
    {
        return (path, None);
    }
    (
        std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
        None,
    )
}

/// Like [`convert::uri_to_canonical_path`], but doesn't require the directory to
/// exist right now for canonicalization to "count" — falls back to the
/// uncanonicalized form (config discovery via `setup::load_config` doesn't require a
/// canonical `cwd`; only file-identity paths inside the graph do).
fn uri_to_dir_path(uri: &Url) -> Option<PathBuf> {
    let path = uri.to_file_path().ok()?;
    Some(path.canonicalize().unwrap_or(path))
}

fn supports_watched_files_registration(capabilities: &ClientCapabilities) -> bool {
    capabilities
        .workspace
        .as_ref()
        .and_then(|workspace| workspace.did_change_watched_files.as_ref())
        .and_then(|watched_files| watched_files.dynamic_registration)
        .unwrap_or(false)
}

/// v1 scope (D-L3, decision E10): full-sync text documents with save notifications
/// (no `includeText`, since overlays already carry the buffer's live content), and
/// nothing else — no hover, code actions, or pull diagnostics (E10, out of scope).
fn server_capabilities() -> ServerCapabilities {
    ServerCapabilities {
        text_document_sync: Some(TextDocumentSyncCapability::Options(
            TextDocumentSyncOptions {
                open_close: Some(true),
                change: Some(TextDocumentSyncKind::FULL),
                will_save: None,
                will_save_wait_until: None,
                save: Some(TextDocumentSyncSaveOptions::SaveOptions(SaveOptions {
                    include_text: Some(false),
                })),
            },
        )),
        ..ServerCapabilities::default()
    }
}

/// Live server state (D-L6): the engine session (`None` until a config loads
/// successfully), open-document bookkeeping, the debounce/pending-changes queue, and
/// the last-published-per-file diagnostic map used to diff publishes (D-L7).
struct Server {
    connection: Connection,
    debounce: Duration,
    workspace_root: PathBuf,
    run_mode: RunMode,

    /// `None` until a config loads successfully (D-L4) — every request/notification
    /// handler that needs it degrades gracefully (no linting, but the protocol keeps
    /// working) rather than panicking.
    session: Option<WatchSession>,

    /// URI -> open document, resolved once at `didOpen` and reused for
    /// `didChange`/`didClose` (D-L6): canonicalization can fail later for a file
    /// deleted on disk, so the identity must be captured while it still exists.
    /// Carries the buffer's current text alongside the path so a session rebuilt
    /// while documents are open (e.g. [`Server::try_build_session`]'s retry after a
    /// config fix) can replay every overlay instead of silently regressing to disk
    /// content for still-open buffers.
    open_docs: HashMap<Url, OpenDocument>,

    /// The last diagnostic set actually published per file, so [`Server::publish_diagnostics`]
    /// only sends `publishDiagnostics` for files whose set changed (D-L7). Only
    /// files with a currently non-empty set are keys here — a file that never had
    /// any diagnostics, or whose diagnostics were just cleared, is absent.
    published: HashMap<PathBuf, Vec<Diagnostic>>,

    /// Changes accumulated since the last flush (D-L6/D-L7).
    pending: Vec<ChangeKind>,
    /// When to flush `pending` if no message arrives first (the debounce timer,
    /// `onType` mode only) — `None` means "block indefinitely for the next message"
    /// (D-L6).
    deadline: Option<Instant>,

    next_request_id: i32,
}

/// One open document's identity and current buffer content (D-L4 replay, see
/// [`Server::open_docs`]).
struct OpenDocument {
    path: PathBuf,
    text: String,
}

impl Server {
    fn new(
        connection: Connection,
        debounce: Duration,
        workspace_root: PathBuf,
        run_mode: RunMode,
    ) -> Self {
        Server {
            connection,
            debounce,
            workspace_root,
            run_mode,
            session: None,
            open_docs: HashMap::new(),
            published: HashMap::new(),
            pending: Vec::new(),
            deadline: None,
            next_request_id: 1,
        }
    }

    fn next_request_id(&mut self) -> RequestId {
        let id = self.next_request_id;
        self.next_request_id += 1;
        RequestId::from(id)
    }

    /// The main loop (D-L6): a `crossbeam_channel::select!` over the connection's
    /// receiver with a debounce timeout arm. Returns `Ok(())` on `exit`, a completed
    /// `shutdown`/`exit` handshake, or the client disconnecting.
    fn run(mut self) -> Result<(), Box<dyn Error + Send + Sync>> {
        loop {
            match self.recv_next() {
                NextMessage::Message(Message::Request(req)) => {
                    if req.method == Shutdown::METHOD {
                        self.connection.handle_shutdown(&req)?;
                        return Ok(());
                    }
                    let response = Response::new_err(
                        req.id,
                        ErrorCode::MethodNotFound as i32,
                        format!("import-lint: unsupported request method {}", req.method),
                    );
                    if self.connection.sender.send(response.into()).is_err() {
                        return Ok(());
                    }
                }
                NextMessage::Message(Message::Notification(not)) => {
                    if not.method == Exit::METHOD {
                        return Ok(());
                    }
                    self.handle_notification(not);
                }
                // Responses to our own outgoing requests (e.g. `client/registerCapability`) —
                // nothing to do with them (D-L6).
                NextMessage::Message(Message::Response(_)) => {}
                NextMessage::Timeout => self.flush(),
                NextMessage::Disconnected => return Ok(()),
            }
        }
    }

    fn recv_next(&self) -> NextMessage {
        if let Some(deadline) = self.deadline {
            let remaining = deadline.saturating_duration_since(Instant::now());
            crossbeam_channel::select! {
                recv(self.connection.receiver) -> msg => match msg {
                    Ok(message) => NextMessage::Message(message),
                    Err(_) => NextMessage::Disconnected,
                },
                default(remaining) => NextMessage::Timeout,
            }
        } else {
            match self.connection.receiver.recv() {
                Ok(message) => NextMessage::Message(message),
                Err(_) => NextMessage::Disconnected,
            }
        }
    }

    fn handle_notification(&mut self, not: LspNotification) {
        let not = match cast_notification::<DidOpenTextDocument>(not) {
            Ok(params) => return self.on_did_open(params),
            Err(not) => not,
        };
        let not = match cast_notification::<DidChangeTextDocument>(not) {
            Ok(params) => return self.on_did_change(params),
            Err(not) => not,
        };
        let not = match cast_notification::<DidSaveTextDocument>(not) {
            Ok(params) => return self.on_did_save(params),
            Err(not) => not,
        };
        let not = match cast_notification::<DidCloseTextDocument>(not) {
            Ok(params) => return self.on_did_close(params),
            Err(not) => not,
        };
        if let Ok(params) = cast_notification::<DidChangeWatchedFiles>(not) {
            self.on_did_change_watched_files(params);
        }
        // Anything else (e.g. `$/cancelRequest`, `workspace/didChangeConfiguration`)
        // is silently ignored — v1 has no use for it.
    }

    /// `didOpen`: resolve the URI to the canonical path identity (ignoring the
    /// document entirely on failure — non-`file:` scheme, untitled buffer, or a
    /// canonicalize error), set the overlay, and flush immediately (D-L6).
    fn on_did_open(&mut self, params: DidOpenTextDocumentParams) {
        let uri = params.text_document.uri;
        let Some(path) = uri_to_canonical_path(&uri) else {
            return;
        };
        let text = params.text_document.text;
        if let Some(session) = &mut self.session {
            session.set_overlay(path.clone(), text.clone());
        }
        self.open_docs.insert(
            uri,
            OpenDocument {
                path: path.clone(),
                text,
            },
        );
        self.pending.push(ChangeKind::ContentEdit(path));
        self.flush();
    }

    /// `didChange`: full sync, so only the last content change's text matters.
    /// `onType` schedules a debounced flush; `onSave` just accumulates (D-L6).
    fn on_did_change(&mut self, params: DidChangeTextDocumentParams) {
        let Some(change) = params.content_changes.into_iter().last() else {
            return;
        };
        let Some(doc) = self.open_docs.get_mut(&params.text_document.uri) else {
            return;
        };
        doc.text = change.text;
        let path = doc.path.clone();
        if let Some(session) = &mut self.session {
            session.set_overlay(path.clone(), doc.text.clone());
        }
        self.pending.push(ChangeKind::ContentEdit(path));
        match self.run_mode {
            RunMode::OnType => self.deadline = Some(Instant::now() + self.debounce),
            RunMode::OnSave => {}
        }
    }

    /// `didSave`: flush immediately, which also drains any pending `onSave` debounce
    /// (D-L6).
    fn on_did_save(&mut self, params: DidSaveTextDocumentParams) {
        let Some(doc) = self.open_docs.get(&params.text_document.uri) else {
            return;
        };
        self.pending.push(ChangeKind::ContentEdit(doc.path.clone()));
        self.flush();
    }

    /// `didClose`: disk is truth again — clear the overlay and flush now (D-L6).
    fn on_did_close(&mut self, params: DidCloseTextDocumentParams) {
        let Some(doc) = self.open_docs.remove(&params.text_document.uri) else {
            return;
        };
        if let Some(session) = &mut self.session {
            session.clear_overlay(&doc.path);
        }
        self.pending.push(ChangeKind::ContentEdit(doc.path));
        self.flush();
    }

    /// `didChangeWatchedFiles`: classify each changed file (D-L5) and flush
    /// immediately — no debounce for config/tsconfig/structural changes.
    fn on_did_change_watched_files(&mut self, params: DidChangeWatchedFilesParams) {
        let tsconfig_path = self
            .session
            .as_ref()
            .and_then(|session| session.watch_targets().tsconfig_path);
        for change in params.changes {
            let Some(path) = change.uri.to_file_path().ok() else {
                continue;
            };
            self.pending
                .push(classify_watched_file(&path, tsconfig_path.as_deref()));
        }
        self.flush();
    }

    /// Send the `client/registerCapability` request for `workspace/didChangeWatchedFiles`
    /// (decision E8/D-L5): generic globs for the config/tsconfig file names, plus an
    /// exact-path watcher for the tsconfig if it's not named `tsconfig.json` (a
    /// generic glob can't express "watch this one specific path").
    fn register_watched_files(&mut self) {
        let mut watchers = vec![
            watcher_for("**/.importlintrc.json"),
            watcher_for("**/.importlintrc.jsonc"),
            watcher_for("**/tsconfig.json"),
            watcher_for("**/package.json"),
        ];
        if let Some(session) = &self.session
            && let Some(tsconfig_path) = session.watch_targets().tsconfig_path
        {
            let default_name =
                tsconfig_path.file_name().and_then(|name| name.to_str()) == Some("tsconfig.json");
            if !default_name {
                watchers.push(watcher_for(&tsconfig_path.to_string_lossy()));
            }
        }

        let registration = Registration {
            id: "import-lint-watched-files".to_string(),
            method: DidChangeWatchedFiles::METHOD.to_string(),
            register_options: Some(
                serde_json::to_value(DidChangeWatchedFilesRegistrationOptions { watchers })
                    .expect("DidChangeWatchedFilesRegistrationOptions always serializes"),
            ),
        };
        let params = RegistrationParams {
            registrations: vec![registration],
        };
        let id = self.next_request_id();
        let request = LspRequest::new(id, RegisterCapability::METHOD.to_string(), params);
        // Best-effort: if the client never responds (or the channel is gone), there's
        // nothing more to do — the response itself is swallowed (D-L6).
        let _ = self.connection.sender.send(request.into());
    }

    /// Try to (re)build the engine session (D-L4): on success, replay every
    /// currently open document's buffer content into the fresh session (a rebuild
    /// can happen with documents already open — e.g. this retry firing right after
    /// a broken `.importlintrc.jsonc` is fixed on disk — and `WatchSession::new`
    /// always starts with empty overlays, so skipping this would silently regress
    /// those buffers to their on-disk content) and diff-publish the result; on
    /// failure, keep `session` at `None` and report the error via
    /// `window/showMessage`.
    fn try_build_session(&mut self) {
        let options = WatchSessionOptions {
            cli_paths: Vec::new(),
            explicit_config: None,
            cli_tsconfig: None,
            report_unresolved: false,
            quiet: false,
            cwd: self.workspace_root.clone(),
        };
        match WatchSession::new(options) {
            Ok(mut session) => {
                if self.open_docs.is_empty() {
                    // The common case (no documents open yet, e.g. the initial
                    // build right after the handshake): `last_diagnostics()`
                    // already reflects the just-completed full pipeline run, no
                    // `run_cycle` needed.
                    let diagnostics = session.last_diagnostics().to_vec();
                    self.session = Some(session);
                    self.publish_diagnostics(diagnostics);
                } else {
                    let replay: Vec<ChangeKind> = self
                        .open_docs
                        .values()
                        .map(|doc| {
                            session.set_overlay(doc.path.clone(), doc.text.clone());
                            ChangeKind::ContentEdit(doc.path.clone())
                        })
                        .collect();
                    let outcome = session.run_cycle(&replay);
                    self.session = Some(session);
                    self.publish_diagnostics(outcome.diagnostics);
                }
            }
            Err(err) => {
                self.session = None;
                self.show_message(MessageType::ERROR, format!("import-lint: {err}"));
            }
        }
    }

    /// Flush = run one engine cycle over the accumulated `pending` changes (D-L7).
    /// A no-op if nothing is pending. If the session is `None` (broken config at
    /// startup, or since the last watched-files event), a batch containing a
    /// `ConfigChanged` is the one case worth retrying [`Server::try_build_session`]
    /// for — anything else just has nothing to check against yet.
    fn flush(&mut self) {
        self.deadline = None;
        let pending = std::mem::take(&mut self.pending);
        if pending.is_empty() {
            return;
        }
        let Some(session) = &mut self.session else {
            if pending
                .iter()
                .any(|change| matches!(change, ChangeKind::ConfigChanged))
            {
                self.try_build_session();
            }
            return;
        };
        let outcome = session.run_cycle(&pending);
        if let Some(message) = outcome.config_error {
            self.show_message(MessageType::WARNING, message);
        }
        self.publish_diagnostics(outcome.diagnostics);
    }

    /// Diff `diagnostics` (the engine's full current set) against `self.published`
    /// and send `textDocument/publishDiagnostics` only for files whose set changed,
    /// including an empty array for any file that dropped out of the set entirely
    /// (decision E3/D-L7).
    fn publish_diagnostics(&mut self, diagnostics: Vec<RenderedDiagnostic>) {
        let mut grouped: HashMap<PathBuf, Vec<Diagnostic>> = HashMap::new();
        for diagnostic in &diagnostics {
            grouped
                .entry(diagnostic.file.clone())
                .or_default()
                .push(to_lsp_diagnostic(diagnostic));
        }

        for (path, diags) in &grouped {
            if self.published.get(path) != Some(diags) {
                self.publish_for_path(path, diags.clone());
            }
        }
        let vanished: Vec<PathBuf> = self
            .published
            .keys()
            .filter(|path| !grouped.contains_key(*path))
            .cloned()
            .collect();
        for path in vanished {
            self.publish_for_path(&path, Vec::new());
        }

        self.published = grouped;
    }

    fn publish_for_path(&self, path: &Path, diagnostics: Vec<Diagnostic>) {
        let Some(uri) = path_to_uri(path) else {
            return;
        };
        let params = PublishDiagnosticsParams {
            uri,
            diagnostics,
            version: None,
        };
        let not = LspNotification::new(PublishDiagnostics::METHOD.to_string(), params);
        let _ = self.connection.sender.send(not.into());
    }

    fn show_message(&self, typ: MessageType, message: String) {
        let params = ShowMessageParams { typ, message };
        let not = LspNotification::new(ShowMessage::METHOD.to_string(), params);
        let _ = self.connection.sender.send(not.into());
    }
}

enum NextMessage {
    Message(Message),
    Timeout,
    Disconnected,
}

/// rust-analyzer-style chained cast: try to interpret `not` as notification kind `N`,
/// returning it back (unchanged) on a method mismatch so the caller can try the next
/// kind. A malformed payload for a *matching* method name is a client protocol bug,
/// not a recoverable condition — panics with the details, same severity as any other
/// unhandled `Err` from a client that violates the wire format it claimed to speak.
fn cast_notification<N>(not: LspNotification) -> Result<N::Params, LspNotification>
where
    N: lsp_types::notification::Notification,
{
    match not.extract(N::METHOD) {
        Ok(params) => Ok(params),
        Err(ExtractError::MethodMismatch(not)) => Err(not),
        Err(ExtractError::JsonError { method, error }) => {
            panic!("import-lint: malformed {method} notification: {error}")
        }
    }
}

fn watcher_for(glob: &str) -> FileSystemWatcher {
    FileSystemWatcher {
        glob_pattern: GlobPattern::String(glob.to_string()),
        kind: None,
    }
}

/// Classify one `didChangeWatchedFiles` event (decision E8/D-L5): the config file
/// (either extension) -> `ConfigChanged`; the tsconfig (by default name, or by exact
/// path match against the session's actual tsconfig) -> `TsconfigChanged`; anything
/// else (including `package.json`, which the LSP client's watcher list also covers)
/// -> `Structural`, matching watch mode's own "package.json anywhere means
/// re-walk" treatment in `crates/cli/src/watch.rs`'s `classify_path`.
fn classify_watched_file(path: &Path, tsconfig_path: Option<&Path>) -> ChangeKind {
    let filename = path.file_name().and_then(|name| name.to_str());
    if matches!(
        filename,
        Some(".importlintrc.json") | Some(".importlintrc.jsonc")
    ) {
        return ChangeKind::ConfigChanged;
    }
    if filename == Some("tsconfig.json") || tsconfig_path.is_some_and(|expected| expected == path) {
        return ChangeKind::TsconfigChanged;
    }
    ChangeKind::Structural
}
