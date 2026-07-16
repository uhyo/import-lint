//! LSP protocol tests (L2, `docs/PLAN-lsp.md` §5): drive the real server loop
//! (`import_lint_cli::lsp::run_with_connection`) over `lsp_server::Connection::memory()`
//! — no stdio, no editor, no real filesystem watcher. The harness below is a small
//! client: it does the initialize handshake, sends notifications, and waits for
//! `publishDiagnostics`/`showMessage`/`client/registerCapability` traffic, answering
//! any other server->client request generically so the server never blocks on us.
//!
//! Every test uses a short injected debounce (`LspOptions::debounce`) so `onType`
//! cycles settle fast without making the suite flaky under load — see
//! `Server::run`'s doc comment in `crates/cli/src/lsp/mod.rs` for the debounce
//! design this exercises.

use std::error::Error;
use std::path::Path;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use import_lint_cli::lsp::{LspOptions, run_with_connection};
use import_lint_cli::watch::WatchSession;
use lsp_server::{
    Connection, ErrorCode, Message, Notification as LspNotification, Request as LspRequest,
    RequestId, Response, ResponseKind,
};
use lsp_types::notification::{
    DidChangeTextDocument, DidChangeWatchedFiles, DidCloseTextDocument, DidOpenTextDocument,
    DidSaveTextDocument, Exit, Initialized, Notification as _, PublishDiagnostics, ShowMessage,
};
use lsp_types::request::{Initialize, RegisterCapability, Request as _, Shutdown};
use lsp_types::{
    ClientCapabilities, Diagnostic, DidChangeTextDocumentParams,
    DidChangeWatchedFilesClientCapabilities, DidChangeWatchedFilesParams,
    DidCloseTextDocumentParams, DidOpenTextDocumentParams, DidSaveTextDocumentParams,
    FileChangeType, FileEvent, InitializeParams, InitializeResult, InitializedParams,
    NumberOrString, Position, PublishDiagnosticsParams, RegistrationParams, ShowMessageParams,
    TextDocumentContentChangeEvent, TextDocumentIdentifier, TextDocumentItem,
    TextDocumentSyncCapability, TextDocumentSyncKind, Url, VersionedTextDocumentIdentifier,
    WorkspaceClientCapabilities, WorkspaceFolder,
};
use tempfile::TempDir;

mod common;
use common::{canonical, session_options, write, write_violation_fixture};

/// A short but generous debounce for every test: long enough that a slow CI runner
/// (Windows especially) won't miscount a keystroke as two cycles, short enough that
/// `expect_no_publish_for`'s multiples of it don't make the suite slow.
const DEBOUNCE: Duration = Duration::from_millis(25);
const TIMEOUT: Duration = Duration::from_secs(5);

/// The client side of a running server: an `lsp_server::Connection` (the other end
/// of `Connection::memory()`) plus the server's own thread handle, joined by
/// [`TestClient::shutdown_and_join`].
struct TestClient {
    connection: Connection,
    thread: Option<JoinHandle<Result<(), Box<dyn Error + Send + Sync>>>>,
}

/// Start a server (on its own thread, over an in-memory connection) and drive it
/// through `initialize`/`initialized`. Returns the client handle plus the
/// `InitializeResult` for handshake assertions.
fn start(
    root: &Path,
    capabilities: ClientCapabilities,
    initialization_options: Option<serde_json::Value>,
    debounce: Duration,
) -> (TestClient, InitializeResult) {
    let (client, server) = Connection::memory();
    let thread = thread::spawn(move || run_with_connection(server, LspOptions { debounce }));

    let root_uri = Url::from_file_path(root).expect("fixture root must be an absolute path");
    let params = InitializeParams {
        workspace_folders: Some(vec![WorkspaceFolder {
            uri: root_uri,
            name: "root".to_string(),
        }]),
        capabilities,
        initialization_options,
        ..InitializeParams::default()
    };
    let init_id = RequestId::from(1);
    client
        .sender
        .send(Message::Request(LspRequest::new(
            init_id.clone(),
            Initialize::METHOD.to_string(),
            params,
        )))
        .expect("client channel should accept the initialize request");

    let result = match client
        .receiver
        .recv_timeout(TIMEOUT)
        .expect("expected an initialize response")
    {
        Message::Response(response) => {
            assert_eq!(response.id, init_id);
            match response.response_kind {
                ResponseKind::Ok { result } => serde_json::from_value::<InitializeResult>(result)
                    .expect("initialize result should deserialize"),
                ResponseKind::Err { error } => panic!("initialize failed: {error:?}"),
            }
        }
        other => panic!("expected an initialize response, got {other:?}"),
    };

    client
        .sender
        .send(Message::Notification(LspNotification::new(
            Initialized::METHOD.to_string(),
            InitializedParams {},
        )))
        .expect("client channel should accept the initialized notification");

    (
        TestClient {
            connection: client,
            thread: Some(thread),
        },
        result,
    )
}

impl TestClient {
    fn notify<N>(&self, params: N::Params)
    where
        N: lsp_types::notification::Notification,
    {
        self.connection
            .sender
            .send(Message::Notification(LspNotification::new(
                N::METHOD.to_string(),
                params,
            )))
            .expect("client channel should accept the notification");
    }

    /// Wait up to `timeout` for a message `pred` recognizes, generically answering
    /// (with a null `Ok` result) any server->client request encountered along the way
    /// that `pred` doesn't itself return `Some` for.
    fn wait_for<T>(&self, timeout: Duration, mut pred: impl FnMut(&Message) -> Option<T>) -> T {
        let deadline = Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                panic!("timed out waiting for an expected message");
            }
            let message = self
                .connection
                .receiver
                .recv_timeout(remaining)
                .expect("server should not disconnect while a test is waiting on it");
            if let Some(value) = pred(&message) {
                return value;
            }
            if let Message::Request(req) = &message {
                self.respond_ok(req.id.clone());
            }
        }
    }

    fn wait_for_publish(&self, uri: &Url, timeout: Duration) -> Vec<Diagnostic> {
        self.wait_for(timeout, |message| match message {
            Message::Notification(not) if not.method == PublishDiagnostics::METHOD => {
                let params: PublishDiagnosticsParams =
                    serde_json::from_value(not.params.clone()).expect("valid publishDiagnostics");
                (&params.uri == uri).then_some(params.diagnostics)
            }
            _ => None,
        })
    }

    fn wait_for_show_message(&self, timeout: Duration) -> ShowMessageParams {
        self.wait_for(timeout, |message| match message {
            Message::Notification(not) if not.method == ShowMessage::METHOD => {
                Some(serde_json::from_value(not.params.clone()).expect("valid showMessage"))
            }
            _ => None,
        })
    }

    fn wait_for_register_capability(&self, timeout: Duration) -> (RequestId, RegistrationParams) {
        self.wait_for(timeout, |message| match message {
            Message::Request(req) if req.method == RegisterCapability::METHOD => {
                let params: RegistrationParams = serde_json::from_value(req.params.clone())
                    .expect("valid registerCapability params");
                Some((req.id.clone(), params))
            }
            _ => None,
        })
    }

    fn respond_ok(&self, id: RequestId) {
        let _ = self
            .connection
            .sender
            .send(Message::Response(Response::new_ok(
                id,
                serde_json::Value::Null,
            )));
    }

    /// Drain traffic for `duration`, failing if a `publishDiagnostics` for `uri`
    /// shows up — used to assert `onSave` mode stays quiet across a bare `didChange`.
    fn expect_no_publish_for(&self, uri: &Url, duration: Duration) {
        let deadline = Instant::now() + duration;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return;
            }
            match self.connection.receiver.recv_timeout(remaining) {
                Ok(Message::Notification(not)) if not.method == PublishDiagnostics::METHOD => {
                    let params: PublishDiagnosticsParams =
                        serde_json::from_value(not.params).expect("valid publishDiagnostics");
                    if &params.uri == uri {
                        panic!(
                            "unexpected publishDiagnostics for {uri} during the onSave quiet window"
                        );
                    }
                }
                Ok(Message::Request(req)) => self.respond_ok(req.id),
                Ok(_) => {}
                Err(_) => return,
            }
        }
    }

    fn shutdown_and_join(mut self) {
        let id = RequestId::from(999);
        self.connection
            .sender
            .send(Message::Request(LspRequest::new(
                id.clone(),
                Shutdown::METHOD.to_string(),
                (),
            )))
            .expect("client channel should accept the shutdown request");
        match self
            .connection
            .receiver
            .recv_timeout(TIMEOUT)
            .expect("expected a shutdown response")
        {
            Message::Response(response) => assert_eq!(response.id, id),
            other => panic!("expected a shutdown response, got {other:?}"),
        }
        self.connection
            .sender
            .send(Message::Notification(LspNotification::new(
                Exit::METHOD.to_string(),
                (),
            )))
            .expect("client channel should accept the exit notification");
        let thread = self.thread.take().expect("thread taken only once");
        thread
            .join()
            .expect("server thread should not panic")
            .expect("server should exit cleanly");
    }
}

fn open(client: &TestClient, uri: &Url, text: &str) {
    client.notify::<DidOpenTextDocument>(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            language_id: "typescript".to_string(),
            version: 1,
            text: text.to_string(),
        },
    });
}

fn change(client: &TestClient, uri: &Url, version: i32, text: &str) {
    client.notify::<DidChangeTextDocument>(DidChangeTextDocumentParams {
        text_document: VersionedTextDocumentIdentifier {
            uri: uri.clone(),
            version,
        },
        content_changes: vec![TextDocumentContentChangeEvent {
            range: None,
            range_length: None,
            text: text.to_string(),
        }],
    });
}

fn save(client: &TestClient, uri: &Url) {
    client.notify::<DidSaveTextDocument>(DidSaveTextDocumentParams {
        text_document: TextDocumentIdentifier { uri: uri.clone() },
        text: None,
    });
}

fn close(client: &TestClient, uri: &Url) {
    client.notify::<DidCloseTextDocument>(DidCloseTextDocumentParams {
        text_document: TextDocumentIdentifier { uri: uri.clone() },
    });
}

fn changed_watched_file(client: &TestClient, path: &Path) {
    client.notify::<DidChangeWatchedFiles>(DidChangeWatchedFilesParams {
        changes: vec![FileEvent {
            uri: Url::from_file_path(path).expect("watched path must be absolute"),
            typ: FileChangeType::CHANGED,
        }],
    });
}

#[test]
fn handshake_reports_server_info_and_full_sync() {
    let dir = TempDir::new().unwrap();
    let (client, result) = start(dir.path(), ClientCapabilities::default(), None, DEBOUNCE);

    assert_eq!(
        result.server_info.as_ref().map(|info| info.name.as_str()),
        Some("import-lint")
    );
    match result.capabilities.text_document_sync {
        Some(TextDocumentSyncCapability::Options(options)) => {
            assert_eq!(options.open_close, Some(true));
            assert_eq!(options.change, Some(TextDocumentSyncKind::FULL));
        }
        other => panic!("expected FULL text document sync options, got {other:?}"),
    }

    client.shutdown_and_join();
}

/// D-L4's initial publish: a project with an existing `@package` violation on disk
/// gets it published for the consumer file right after `initialize`+`initialized`,
/// with no editor interaction at all.
#[test]
fn initial_publish_reports_existing_violation() {
    let dir = TempDir::new().unwrap();
    let (consumer, _util) = write_violation_fixture(dir.path());
    let consumer_uri = Url::from_file_path(&consumer).unwrap();

    // A reference session over the same fixture gives the exact (1-based) line/col
    // the engine computed, so the assertion below checks the LSP 0-based conversion
    // itself rather than a hardcoded position.
    let reference = WatchSession::new(session_options(dir.path())).expect("session builds");
    let expected = reference.last_diagnostics()[0].clone();

    let (client, _) = start(dir.path(), ClientCapabilities::default(), None, DEBOUNCE);

    let diagnostics = client.wait_for_publish(&consumer_uri, TIMEOUT);
    assert_eq!(diagnostics.len(), 1);
    let diagnostic = &diagnostics[0];
    assert_eq!(
        diagnostic.code,
        Some(NumberOrString::String("import-access/jsdoc".to_string()))
    );
    assert_eq!(diagnostic.source.as_deref(), Some("import-lint"));
    assert_eq!(
        diagnostic.range.start,
        Position::new(expected.line - 1, expected.column - 1)
    );
    assert_eq!(
        diagnostic.range.end,
        Position::new(expected.end_line - 1, expected.end_column - 1)
    );

    client.shutdown_and_join();
}

/// The L2 exit criterion: an unsaved edit in one open buffer (the exporter) moves a
/// diagnostic in a *different*, closed file (the consumer) that imports it.
#[test]
fn cross_file_edit_moves_diagnostic_in_a_different_closed_file() {
    let dir = TempDir::new().unwrap();
    let (consumer, util) = write_violation_fixture(dir.path());
    let consumer_uri = Url::from_file_path(&consumer).unwrap();
    let util_uri = Url::from_file_path(&util).unwrap();

    let (client, _) = start(dir.path(), ClientCapabilities::default(), None, DEBOUNCE);
    let diagnostics = client.wait_for_publish(&consumer_uri, TIMEOUT);
    assert_eq!(
        diagnostics.len(),
        1,
        "expected the on-disk violation initially"
    );

    open(
        &client,
        &util_uri,
        "/** @package */\nexport const helper = 1;\n",
    );
    change(
        &client,
        &util_uri,
        2,
        "/** @public */\nexport const helper = 1;\n",
    );

    let diagnostics = client.wait_for_publish(&consumer_uri, TIMEOUT);
    assert!(
        diagnostics.is_empty(),
        "expected the consumer's violation to clear from an unsaved overlay edit to util.ts, got {diagnostics:?}"
    );

    client.shutdown_and_join();
}

/// Clear semantics (E3/D-L7): fixing a violation must republish an *empty* array,
/// not just stop publishing.
#[test]
fn fixing_a_violation_publishes_an_empty_array() {
    let dir = TempDir::new().unwrap();
    let (consumer, _util) = write_violation_fixture(dir.path());
    let consumer_uri = Url::from_file_path(&consumer).unwrap();
    let original = std::fs::read_to_string(&consumer).unwrap();

    let (client, _) = start(dir.path(), ClientCapabilities::default(), None, DEBOUNCE);
    let diagnostics = client.wait_for_publish(&consumer_uri, TIMEOUT);
    assert_eq!(diagnostics.len(), 1);

    open(&client, &consumer_uri, &original);
    change(&client, &consumer_uri, 2, "console.log(1);\n");

    let diagnostics = client.wait_for_publish(&consumer_uri, TIMEOUT);
    assert!(
        diagnostics.is_empty(),
        "expected an empty publishDiagnostics after the fix, got {diagnostics:?}"
    );

    client.shutdown_and_join();
}

/// `didClose` makes disk truth again: an overlay that fixed the violation is
/// discarded on close, and the on-disk violation is republished.
#[test]
fn did_close_reverts_to_disk_content() {
    let dir = TempDir::new().unwrap();
    let (consumer, _util) = write_violation_fixture(dir.path());
    let consumer_uri = Url::from_file_path(&consumer).unwrap();
    let original = std::fs::read_to_string(&consumer).unwrap();

    let (client, _) = start(dir.path(), ClientCapabilities::default(), None, DEBOUNCE);
    let diagnostics = client.wait_for_publish(&consumer_uri, TIMEOUT);
    assert_eq!(diagnostics.len(), 1);

    open(&client, &consumer_uri, &original);
    change(&client, &consumer_uri, 2, "console.log(1);\n");
    let diagnostics = client.wait_for_publish(&consumer_uri, TIMEOUT);
    assert!(
        diagnostics.is_empty(),
        "expected the overlay to clear the violation"
    );

    close(&client, &consumer_uri);
    let diagnostics = client.wait_for_publish(&consumer_uri, TIMEOUT);
    assert_eq!(
        diagnostics.len(),
        1,
        "expected didClose to revert to the on-disk (violating) content, got {diagnostics:?}"
    );

    client.shutdown_and_join();
}

/// Config hot-reload (E8/D-L5): editing `.importlintrc.jsonc` to exclude the
/// consumer file makes its violation disappear once `didChangeWatchedFiles` fires;
/// a subsequently broken config surfaces a `showMessage` and leaves diagnostics as
/// they were (matching `WatchSession::run_cycle`'s own `config_error` contract).
#[test]
fn config_hot_reload_and_broken_config_recovery() {
    let dir = TempDir::new().unwrap();
    let (consumer, _util) = write_violation_fixture(dir.path());
    let consumer_uri = Url::from_file_path(&consumer).unwrap();
    let config_path = write(dir.path(), ".importlintrc.jsonc", "{}");

    let (client, _) = start(dir.path(), ClientCapabilities::default(), None, DEBOUNCE);
    let diagnostics = client.wait_for_publish(&consumer_uri, TIMEOUT);
    assert_eq!(diagnostics.len(), 1);

    std::fs::write(&config_path, r#"{ "exclude": ["src/consumer.ts"] }"#).unwrap();
    changed_watched_file(&client, &config_path);

    let diagnostics = client.wait_for_publish(&consumer_uri, TIMEOUT);
    assert!(
        diagnostics.is_empty(),
        "expected the exclude to drop consumer.ts's violation, got {diagnostics:?}"
    );

    std::fs::write(&config_path, "{ not valid json ").unwrap();
    changed_watched_file(&client, &config_path);

    let message = client.wait_for_show_message(TIMEOUT);
    assert_eq!(message.typ, lsp_types::MessageType::WARNING);

    client.shutdown_and_join();
}

/// D-L4 replay fix (follow-up review): a session (re)built while a document is
/// already open must not silently regress that document to its on-disk content.
/// Starts with a broken `.importlintrc.jsonc` (so the handshake's own
/// `try_build_session` fails and reports it via `showMessage`, leaving no session),
/// opens the violating file and fixes it *in the buffer only*, then fixes the config
/// on disk and notifies the server — the resulting rebuild must replay the open
/// buffer's content before its first publish.
///
/// Note on the assertion shape: because the rebuild's diff-publish only sends
/// `publishDiagnostics` for files whose set *changed* (D-L7), the correct outcome
/// here (buffer content is clean, nothing was ever published before) produces *no*
/// message for `consumer_uri` at all — only the buggy behavior (replaying nothing,
/// so the still-violating on-disk content gets checked instead) produces one. A
/// second file (`marker.ts`) is opened afterwards and edited into its own violation
/// purely as a synchronization anchor: the server is single-threaded and processes
/// notifications strictly in order, so once `marker.ts`'s publish arrives, the
/// config-fix rebuild has already fully completed and any publish it produced for
/// `consumer_uri` is already sitting in the channel, ready to have been observed by
/// the same `wait_for` call.
#[test]
fn session_rebuild_replays_open_document_overlays() {
    let dir = TempDir::new().unwrap();
    let (consumer, _util) = write_violation_fixture(dir.path());
    let consumer_uri = Url::from_file_path(&consumer).unwrap();
    let disk_content = std::fs::read_to_string(&consumer).unwrap();
    let config_path = write(dir.path(), ".importlintrc.jsonc", "{ not valid json ");
    let marker_uri = Url::from_file_path(canonical(&write(
        dir.path(),
        "src/marker.ts",
        "export const marker = 1;\n",
    )))
    .unwrap();

    let (client, _) = start(dir.path(), ClientCapabilities::default(), None, DEBOUNCE);

    // No session yet (broken config at startup): the handshake's own
    // `try_build_session` call reports it via `showMessage`.
    let message = client.wait_for_show_message(TIMEOUT);
    assert_eq!(message.typ, lsp_types::MessageType::ERROR);

    // Open the violating file and fix it in the buffer only — disk still violates.
    open(&client, &consumer_uri, &disk_content);
    change(&client, &consumer_uri, 2, "console.log(1);\n");

    // Fix the config on disk and notify the server: this is the retry path that
    // rebuilds the session while `consumer.ts` is already open with an unsaved fix.
    std::fs::write(&config_path, "{}").unwrap();
    changed_watched_file(&client, &config_path);

    // The synchronization anchor described above: `marker.ts` is in the same
    // directory as `consumer.ts`, so importing `helper` from it is a violation too
    // (same one-hop rule), giving us a publish that's guaranteed to happen.
    open(&client, &marker_uri, "export const marker = 1;\n");
    change(
        &client,
        &marker_uri,
        2,
        "import { helper } from \"./internal/util\";\nconsole.log(helper);\n",
    );

    let mut consumer_diagnostics: Option<Vec<Diagnostic>> = None;
    client.wait_for(TIMEOUT, |message| match message {
        Message::Notification(not) if not.method == PublishDiagnostics::METHOD => {
            let params: PublishDiagnosticsParams =
                serde_json::from_value(not.params.clone()).expect("valid publishDiagnostics");
            if params.uri == consumer_uri {
                consumer_diagnostics = Some(params.diagnostics.clone());
            }
            (params.uri == marker_uri).then_some(())
        }
        _ => None,
    });

    assert!(
        consumer_diagnostics.as_ref().is_none_or(Vec::is_empty),
        "expected the rebuilt session to reflect the open buffer's fix, not the \
         on-disk violation, got {consumer_diagnostics:?}"
    );

    client.shutdown_and_join();
}

/// Dynamic registration (E8/D-L5): a client that advertises
/// `didChangeWatchedFiles.dynamicRegistration: true` receives a
/// `client/registerCapability` request after `initialized`.
#[test]
fn dynamic_registration_sends_register_capability() {
    let dir = TempDir::new().unwrap();
    let capabilities = ClientCapabilities {
        workspace: Some(WorkspaceClientCapabilities {
            did_change_watched_files: Some(DidChangeWatchedFilesClientCapabilities {
                dynamic_registration: Some(true),
                relative_pattern_support: None,
            }),
            ..WorkspaceClientCapabilities::default()
        }),
        ..ClientCapabilities::default()
    };

    let (client, _) = start(dir.path(), capabilities, None, DEBOUNCE);

    let (id, params) = client.wait_for_register_capability(TIMEOUT);
    assert_eq!(params.registrations.len(), 1);
    assert_eq!(
        params.registrations[0].method,
        DidChangeWatchedFiles::METHOD
    );
    client.respond_ok(id);

    client.shutdown_and_join();
}

/// `onSave` mode (E4): a bare `didChange` produces no publish for a good while, and
/// `didSave` then flushes it.
#[test]
fn on_save_mode_only_publishes_after_save() {
    let dir = TempDir::new().unwrap();
    let (consumer, _util) = write_violation_fixture(dir.path());
    let consumer_uri = Url::from_file_path(&consumer).unwrap();
    let original = std::fs::read_to_string(&consumer).unwrap();

    let init_options = serde_json::json!({ "run": "onSave" });
    let (client, _) = start(
        dir.path(),
        ClientCapabilities::default(),
        Some(init_options),
        DEBOUNCE,
    );
    let diagnostics = client.wait_for_publish(&consumer_uri, TIMEOUT);
    assert_eq!(diagnostics.len(), 1);

    open(&client, &consumer_uri, &original);
    change(&client, &consumer_uri, 2, "console.log(1);\n");
    client.expect_no_publish_for(&consumer_uri, DEBOUNCE * 8);

    save(&client, &consumer_uri);
    let diagnostics = client.wait_for_publish(&consumer_uri, TIMEOUT);
    assert!(
        diagnostics.is_empty(),
        "expected didSave to flush the fix, got {diagnostics:?}"
    );

    client.shutdown_and_join();
}

#[test]
fn shutdown_then_exit_joins_the_server_thread_cleanly() {
    let dir = TempDir::new().unwrap();
    let (client, _) = start(dir.path(), ClientCapabilities::default(), None, DEBOUNCE);
    client.shutdown_and_join();
}

/// An unsupported request gets a `MethodNotFound` error response rather than being
/// silently ignored or crashing the server.
#[test]
fn unsupported_request_gets_method_not_found() {
    let dir = TempDir::new().unwrap();
    let (client, _) = start(dir.path(), ClientCapabilities::default(), None, DEBOUNCE);

    let id = RequestId::from(42);
    client
        .connection
        .sender
        .send(Message::Request(LspRequest::new(
            id.clone(),
            "textDocument/hover".to_string(),
            serde_json::Value::Null,
        )))
        .unwrap();

    match client
        .connection
        .receiver
        .recv_timeout(TIMEOUT)
        .expect("expected a response")
    {
        Message::Response(response) => {
            assert_eq!(response.id, id);
            match response.response_kind {
                ResponseKind::Err { error } => {
                    assert_eq!(error.code, ErrorCode::MethodNotFound as i32);
                }
                ResponseKind::Ok { .. } => panic!("expected an error response"),
            }
        }
        other => panic!("expected a response, got {other:?}"),
    }

    client.shutdown_and_join();
}
