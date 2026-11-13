//! Background actor that owns an LspClient and processes commands.
//!
//! The actor runs in a tokio::spawn task.
//! Commands arrive via an mpsc channel.
//! During each request to the LSP server, notifications (like publishDiagnostics)
//! are captured via request_capturing and processed by process_captured_notifications

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use lsp_types::{
    CallHierarchyIncomingCall, CallHierarchyIncomingCallsParams, CallHierarchyItem, CallHierarchyOutgoingCall,
    CallHierarchyOutgoingCallsParams, CallHierarchyPrepareParams, ClientCapabilities, Diagnostic, DocumentSymbolParams,
    GotoDefinitionParams, GotoDefinitionResponse, Location, PartialResultParams, Position, PublishDiagnosticsParams,
    ReferenceContext, ReferenceParams, TextDocumentIdentifier, TextDocumentPositionParams, Uri, WorkDoneProgressParams,
};
use serde::Serialize;
use serde_json::Value;
use tokio::sync::{mpsc, oneshot, watch};

use crate::{
    CoderagError, LspServerConfig, Result,
    lsp::{DocumentSymbol, LspStatus, LspTransport, client::LspClient, parse_document_symbols},
};

/// Maximum number of concurrently open documents in a single LSP server.
/// When this limit is reached, the actor closes the least recentrly used
/// document before opening a new one.
const DEFAULT_MAX_OPEN_DOCUMENTS: usize = 50;

/// A command sent from an LspHandle to the LspActor.
///
/// These are all the operations LspActor can execute.
/// Each variant carries the parameters for one LSP operacion and
/// a oneshot sender to return the result.
pub(crate) enum LspCommand {
    DocumentSymbols {
        path: PathBuf,
        reply: oneshot::Sender<Result<Vec<DocumentSymbol>>>,
    },
    GotoDefinition {
        path: PathBuf,
        line: u32,
        character: u32,
        reply: oneshot::Sender<Result<Vec<Location>>>,
    },
    References {
        path: PathBuf,
        line: u32,
        character: u32,
        reply: oneshot::Sender<Result<Vec<Location>>>,
    },
    PrepareCallHierarchy {
        path: PathBuf,
        line: u32,
        character: u32,
        reply: oneshot::Sender<Result<Vec<CallHierarchyItem>>>,
    },
    IncomingCalls {
        item: CallHierarchyItem,
        reply: oneshot::Sender<Result<Vec<CallHierarchyIncomingCall>>>,
    },
    OutgoingCalls {
        item: CallHierarchyItem,
        reply: oneshot::Sender<Result<Vec<CallHierarchyOutgoingCall>>>,
    },
    Diagnostics {
        path: PathBuf,
        reply: oneshot::Sender<Result<Vec<Diagnostic>>>,
    },
}

/// Metadata for a currently open document.
///
/// Information is used for LRU eviction when reach open documents limit
struct OpenDocument {
    uri: Uri,
    last_used: Instant,
}

/// Background actor that owns an LspClient and processes commands.
///
/// Created by LspActor::spawn, which returns an LspHandle.
/// The actor runs until the command channel closes ( all handles dropped)
pub(crate) struct LspActor {
    client: Box<dyn LspTransport>,
    commands: mpsc::Receiver<LspCommand>,
    status_tx: watch::Sender<LspStatus>,

    /// Diagnostics received via publishDiagnostics notifications.
    /// Keyed by file URI string. Each entry is the LATEST diagnostics for
    /// that file (each notification replaces the previous one, per LSP spec)
    diagnostics: HashMap<String, Vec<Diagnostic>>,

    /// Currently open documents, keyed by absolute path string.
    /// Used for LRU eviction when max_open_documents is reached.
    open_docs: HashMap<String, OpenDocument>,

    /// Maximum number of concurrently open documents
    max_open_documents: usize,
}

impl LspActor {
    pub(crate) async fn run(
        config: &LspServerConfig,
        root_path: &Path,
        capabilities: ClientCapabilities,
        language_id: String,
        commands: mpsc::Receiver<LspCommand>,
        status_tx: watch::Sender<LspStatus>,
    ) {
        let client = match LspClient::new_from_parts(&config, &root_path, &capabilities, &language_id).await {
            Ok(client) => {
                let _ = status_tx.send(LspStatus::Ready);
                tracing::info!("LSP actor ready for {language_id}");
                Box::new(client) as Box<dyn LspTransport>
            },
            Err(err) => {
                let reason = err.to_string();
                tracing::warn!("LSP actor failed for {language_id}: {reason}");
                let _ = status_tx.send(LspStatus::Failed(reason));
                return;
            },
        };

        Self::run_with_client(client, commands, status_tx).await;
    }

    pub(crate) async fn run_with_client(
        client: Box<dyn LspTransport>,
        commands: mpsc::Receiver<LspCommand>,
        status_tx: watch::Sender<LspStatus>,
    ) {
        let mut actor = LspActor {
            client,
            commands,
            status_tx,
            diagnostics: HashMap::new(),
            open_docs: HashMap::new(),
            max_open_documents: DEFAULT_MAX_OPEN_DOCUMENTS,
        };
        actor.command_loop().await;
    }

    /// Main loop: receive commands from handles and process them.
    ///
    /// Processes commands sequentially (one at a time ) because the LspClient
    /// is sequential ( one outstanding request at a time). A future versions
    /// could read from stdout concurrently using tokio::select! to capture
    /// notifications between commands.
    ///
    /// Notifications are captured during request_capturing() calls inside each
    /// do_* method, then processed by process_captured_notifications()
    async fn command_loop(&mut self) {
        while let Some(cmd) = self.commands.recv().await {
            self.handle_command(cmd).await;
        }
        let _ = self.client.shutdown().await;
        tracing::info!("LSP actor shut down");
    }

    async fn handle_command(&mut self, cmd: LspCommand) {
        match cmd {
            LspCommand::DocumentSymbols { path, reply } => {
                let result = self.do_document_symbols(&path).await;
                let _ = reply.send(result);
            },
            LspCommand::GotoDefinition {
                path,
                line,
                character,
                reply,
            } => {
                let result = self.do_goto_definition(&path, line, character).await;
                let _ = reply.send(result);
            },
            LspCommand::References {
                path,
                line,
                character,
                reply,
            } => {
                let result = self.do_references(&path, line, character).await;
                let _ = reply.send(result);
            },
            LspCommand::PrepareCallHierarchy {
                path,
                line,
                character,
                reply,
            } => {
                let result = self.do_prepare_call_hierarchy(&path, line, character).await;
                let _ = reply.send(result);
            },
            LspCommand::IncomingCalls { item, reply } => {
                let result = self.do_incoming_calls(item).await;
                let _ = reply.send(result);
            },
            LspCommand::OutgoingCalls { item, reply } => {
                let result = self.do_outgoing_calls(item).await;
                let _ = reply.send(result);
            },
            LspCommand::Diagnostics { path, reply } => {
                let result = self.do_diagnostics(&path).await;
                let _ = reply.send(result);
            },
        }
    }

    // --- Command implementations ---
    async fn do_document_symbols(&mut self, path: &Path) -> Result<Vec<DocumentSymbol>> {
        let uri = self.ensure_document_open(path).await?;

        for attempt in 0..5u64 {
            if attempt > 0 {
                tokio::time::sleep(Duration::from_millis(200 * attempt)).await;
            }

            let params = DocumentSymbolParams {
                text_document: TextDocumentIdentifier::new(uri.clone()),
                work_done_progress_params: WorkDoneProgressParams::default(),
                partial_result_params: PartialResultParams::default(),
            };

            let result = self.request_and_capture("textDocument/documentSymbol", params).await?;
            let symbols = parse_document_symbols(result)?;
            if !symbols.is_empty() {
                return Ok(symbols);
            }
            tracing::debug!("documentSymbol attempt {attempt} empty, retrying");
        }
        Ok(Vec::new())
    }

    async fn do_goto_definition(&mut self, path: &Path, line: u32, character: u32) -> Result<Vec<Location>> {
        let uri = self.ensure_document_open(path).await?;

        for attempt in 0..3u64 {
            if attempt > 0 {
                tokio::time::sleep(Duration::from_millis(200 * attempt)).await;
            }

            let params = GotoDefinitionParams {
                text_document_position_params: TextDocumentPositionParams::new(
                    TextDocumentIdentifier::new(uri.clone()),
                    Position::new(line, character),
                ),
                work_done_progress_params: WorkDoneProgressParams::default(),
                partial_result_params: PartialResultParams::default(),
            };

            let result = self.request_and_capture("textDocument/definition", params).await?;
            if result.is_null() {
                tracing::debug!("gotoDefinition attemt {attempt} null, retrying");
                continue;
            }
            return match serde_json::from_value::<GotoDefinitionResponse>(result) {
                Ok(GotoDefinitionResponse::Scalar(loc)) => Ok(vec![loc]),
                Ok(GotoDefinitionResponse::Array(locs)) => Ok(locs),
                Ok(GotoDefinitionResponse::Link(links)) => Ok(links
                    .into_iter()
                    .map(|link| Location {
                        uri: link.target_uri,
                        range: link.target_selection_range,
                    })
                    .collect()),
                Err(_) => Ok(Vec::new()),
            };
        }
        Ok(Vec::new())
    }

    async fn do_references(&mut self, path: &Path, line: u32, character: u32) -> Result<Vec<Location>> {
        let uri = self.ensure_document_open(path).await?;

        for attempt in 0..3u64 {
            if attempt > 0 {
                tokio::time::sleep(Duration::from_millis(200 * attempt)).await;
            }

            let params = ReferenceParams {
                text_document_position: TextDocumentPositionParams::new(
                    TextDocumentIdentifier::new(uri.clone()),
                    Position::new(line, character),
                ),
                work_done_progress_params: WorkDoneProgressParams::default(),
                partial_result_params: PartialResultParams::default(),
                context: ReferenceContext {
                    include_declaration: false,
                },
            };

            let result = self.request_and_capture("textDocument/references", params).await?;
            if result.is_null() {
                tracing::debug!("references attempt {attempt} null, retrying");
                continue;
            }

            return Ok(serde_json::from_value(result).unwrap_or_default());
        }
        Ok(Vec::new())
    }

    async fn do_prepare_call_hierarchy(
        &mut self,
        path: &Path,
        line: u32,
        character: u32,
    ) -> Result<Vec<CallHierarchyItem>> {
        let uri = self.ensure_document_open(path).await?;

        let params = CallHierarchyPrepareParams {
            text_document_position_params: TextDocumentPositionParams::new(
                TextDocumentIdentifier::new(uri),
                Position::new(line, character),
            ),
            work_done_progress_params: WorkDoneProgressParams::default(),
        };

        let result = self
            .request_and_capture("textDocument/prepareCallHierarchy", params)
            .await?;
        Ok(serde_json::from_value(result).unwrap_or_default())
    }

    async fn do_incoming_calls(&mut self, item: CallHierarchyItem) -> Result<Vec<CallHierarchyIncomingCall>> {
        let params = CallHierarchyIncomingCallsParams {
            item,
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
        };

        let result = self.request_and_capture("callHierarchy/incomingCalls", params).await?;
        Ok(serde_json::from_value(result).unwrap_or_default())
    }

    async fn do_outgoing_calls(&mut self, item: CallHierarchyItem) -> Result<Vec<CallHierarchyOutgoingCall>> {
        let params = CallHierarchyOutgoingCallsParams {
            item,
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
        };

        let result = self.request_and_capture("callHierarchy/outgoingCalls", params).await?;
        Ok(serde_json::from_value(result).unwrap_or_default())
    }

    async fn do_diagnostics(&mut self, path: &Path) -> Result<Vec<Diagnostic>> {
        let uri = self.ensure_document_open(path).await?;

        // Force the LSP to emit pending publishDiagnostics by sending a
        // documentSymbol  request.
        // Not perfect(the LSP may not have finished analysis), but captures
        // diagnostics that are already available
        let params = DocumentSymbolParams {
            text_document: TextDocumentIdentifier::new(uri.clone()),
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
        };

        let _ = self.request_and_capture("textDocument/documentSymbol", params).await;
        Ok(self.diagnostics.get(uri.as_str()).cloned().unwrap_or_default())
    }

    // --- document lifecycle ---
    /// Ensure a document is open in the LSP server.
    /// If the document is already open, update its last_used timestamp.
    /// If we have reached the max open documents limit, close the LRU one first
    async fn ensure_document_open(&mut self, path: &Path) -> Result<Uri> {
        let key = path.to_string_lossy().to_string();

        if let Some(doc) = self.open_docs.get_mut(&key) {
            doc.last_used = Instant::now();
            return Ok(doc.uri.clone());
        }

        if self.open_docs.len() >= self.max_open_documents {
            self.evict_lru_document().await;
        }

        let uri = self.client.open_document(path).await?;
        self.open_docs.insert(
            key,
            OpenDocument {
                uri: uri.clone(),
                last_used: Instant::now(),
            },
        );
        Ok(uri)
    }

    /// Close the least recently used document to free a slot
    async fn evict_lru_document(&mut self) {
        let lru_key = self
            .open_docs
            .iter()
            .min_by_key(|(_, doc)| doc.last_used)
            .map(|(key, _)| key.clone());

        if let Some(key) = lru_key {
            if let Some(doc) = self.open_docs.remove(&key) {
                let _ = self.client.close_document(&doc.uri).await;
                self.diagnostics.remove(doc.uri.as_str());
                tracing::debug!("Evicted LRU document: {key}");
            }
        }
    }

    // --- Notification handling ---
    /// Process captured notifications from a request_capturing call.
    ///
    /// Called after every do_* method that uses request_capturing.
    /// Iterates the captured messages, extracts publishDiagnostics notifications,
    /// and stores them via store_diagnostics.
    /// Other notification types are ignored
    fn process_captured_notifications(&mut self, notifications: Vec<serde_json::Value>) {
        for msg in notifications {
            let method = msg.get("method").and_then(|m| m.as_str()).unwrap_or("");
            if method == "textDocument/publishDiagnostics" {
                if let Some(params) = msg.get("params") {
                    if let Ok(diag_params) = serde_json::from_value::<PublishDiagnosticsParams>(params.clone()) {
                        self.store_diagnostics(diag_params);
                    }
                }
            }
        }
    }

    /// Process publishDiagnostics notification from the LSP server.
    /// Replaces the diagnostics for the given URI (per LSP spec semantics)
    fn store_diagnostics(&mut self, params: PublishDiagnosticsParams) {
        let uri_str = params.uri.as_str().to_string();
        if params.diagnostics.is_empty() {
            self.diagnostics.remove(&uri_str);
        } else {
            self.diagnostics.insert(uri_str, params.diagnostics);
        }
    }

    // --- other functions
    async fn request_and_capture(&mut self, method: &str, params: impl Serialize) -> Result<Value> {
        let params_value =
            serde_json::to_value(params).map_err(|err| CoderagError::Lsp(format!("serialize params: {err}")))?;
        let mut captured = Vec::new();
        let result = self
            .client
            .request_capturing(method, params_value, &mut captured)
            .await?;
        self.process_captured_notifications(captured);
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use std::{
        collections::VecDeque,
        path::{Path, PathBuf},
        sync::{Arc, Mutex},
    };

    use async_trait::async_trait;
    use lsp_types::{Diagnostic, Uri};
    use serde_json::{Value, json};
    use tokio::sync::{mpsc, oneshot, watch};

    use crate::{
        CoderagError, LspStatus, Result,
        lsp::{
            DocumentSymbol, LspTransport,
            actor::{DEFAULT_MAX_OPEN_DOCUMENTS, LspActor, LspCommand},
            path_to_file_uri,
        },
    };

    #[derive(Default)]
    struct TestLog {
        /// Paths passed to open_document, in call order
        opened: Vec<PathBuf>,

        /// URI strings passed to close_document, in call order
        closed: Vec<String>,

        /// Whether shutdown was called
        shutdown_called: bool,

        /// LSP method strings passed to request_capturing, in call order
        methods_called: Vec<String>,
    }

    struct MockTransport {
        /// Pre loaded responses, consumed FIFO on each request_capturing call
        responses: VecDeque<Value>,

        /// Pre-loaded notification batches, consumed FIFO.
        /// Each entry is a Vec<Value> that gets pushed into `captured`
        /// on the corresponding `request_capturing` call
        notifications: VecDeque<Vec<Value>>,

        /// Shared log for post-test inspection of side-effects
        log: Arc<Mutex<TestLog>>,
    }

    #[async_trait]
    impl LspTransport for MockTransport {
        async fn request_capturing(
            &mut self,
            method: &str,
            _params: Value,
            captured: &mut Vec<Value>,
        ) -> Result<Value> {
            self.log.lock().unwrap().methods_called.push(method.to_string());
            if let Some(notifs) = self.notifications.pop_front() {
                captured.extend(notifs);
            }
            self.responses
                .pop_front()
                .ok_or_else(|| CoderagError::Lsp("no more mock responses".into()))
        }

        async fn open_document(&mut self, path: &Path) -> Result<Uri> {
            self.log.lock().unwrap().opened.push(path.to_path_buf());
            Ok(path_to_file_uri(path)?)
        }

        async fn close_document(&mut self, uri: &Uri) -> Result<()> {
            self.log.lock().unwrap().closed.push(uri.as_str().to_string());
            Ok(())
        }

        async fn shutdown(&mut self) -> Result<()> {
            self.log.lock().unwrap().shutdown_called = true;
            Ok(())
        }
    }

    impl MockTransport {
        fn new(log: Arc<Mutex<TestLog>>) -> Self {
            Self {
                responses: VecDeque::new(),
                notifications: VecDeque::new(),
                log,
            }
        }

        /// Enqueue a response that `request_capturing` will return
        fn push_response(&mut self, response: Value) {
            self.responses.push_back(response);
        }

        /// Enqueue a batch of notifications that will be injected into the
        /// `captured` vec on the next `request_capturing` call
        fn push_notifications(&mut self, notifs: Vec<Value>) {
            self.notifications.push_back(notifs);
        }
    }

    /// Helper: Spawn an actor in a background task with the given mock.
    /// Returns the command sender, the status receiver and shared log.
    ///
    /// The actor runs until `cmd_tx` is dropped, at which point it calls
    /// `shutdown()` and exits.
    fn spawn_actor(
        mock: MockTransport,
    ) -> (
        mpsc::Sender<LspCommand>,
        watch::Receiver<LspStatus>,
        Arc<Mutex<TestLog>>,
    ) {
        let log = Arc::clone(&mock.log);
        let (cmd_tx, cmd_rx) = mpsc::channel(32);
        let (status_tx, status_rx) = watch::channel(LspStatus::Ready);

        tokio::spawn(async move {
            LspActor::run_with_client(Box::new(mock), cmd_rx, status_tx).await;
        });

        (cmd_tx, status_rx, log)
    }

    /// Helper: Create a path and its corresponding URI from a single source.
    fn test_file(name: &str) -> (PathBuf, String) {
        let path = PathBuf::from(format!("/tmp/{name}"));
        let uri = format!("file:///tmp/{name}");
        (path, uri)
    }

    /// Helper: build a valid Lsp DocumentSymbol JSON value
    fn symbol_json(name: &str, kind: u32, line: u32) -> Value {
        json!({
            "name": name,
            "kind": kind,
            "range" : {
                "start": {"line": line, "character": 0},
                "end": {"line": line+5, "character": 1},
            },
            "selectionRange" : {
                "start": {"line": line, "character": 3},
                "end": {"line": line, "character": 3 + name.len()},
            }
        })
    }

    /// Helper: build a publishDiagnostics notification JSON
    fn publish_diagnostics_notification(uri: &str, messages: &[&str]) -> Value {
        let diagnostics = messages
            .iter()
            .map(|msg| {
                json!({
                    "range": {
                        "start": {"line": 0, "character": 0},
                        "end": {"line": 0, "character": 1},
                    },
                    "message": msg
                })
            })
            .collect::<Vec<_>>();

        json!({
            "method": "textDocument/publishDiagnostics",
            "params": {
                "uri": uri,
                "diagnostics": diagnostics
            }
        })
    }

    /// Helper: Send a DocumentSymbols command and return the result
    async fn send_document_symbols(cmd_tx: &mpsc::Sender<LspCommand>, path: &Path) -> Result<Vec<DocumentSymbol>> {
        let (tx, rx) = oneshot::channel();
        cmd_tx
            .send(LspCommand::DocumentSymbols {
                path: path.to_path_buf(),
                reply: tx,
            })
            .await
            .unwrap();
        rx.await.unwrap()
    }

    /// Helper: Send a Diagnostics command and return the result
    async fn send_diagnostics(cmd_tx: &mpsc::Sender<LspCommand>, path: &Path) -> Result<Vec<Diagnostic>> {
        let (tx, rx) = oneshot::channel();
        cmd_tx
            .send(LspCommand::Diagnostics {
                path: path.to_path_buf(),
                reply: tx,
            })
            .await
            .unwrap();
        rx.await.unwrap()
    }

    #[tokio::test]
    async fn store_diagnostics_replaces_existing() {
        // Goal: a second publishDiagnostics for the same URI replaces the first
        //
        // Setup: Pre load 3 commands in the mock
        // 1.- DocumentSymbol -> carries notification with "error 1"
        // 2.- DocumentSymbol -> carries notification with "error 2" (same URI)
        // 3.- Diagnostics -> do_diagnostics send a dummy DocumentSymbol
        //                  internally (needs 1 response) then returns stored diagnostics
        //
        // Expected: Diagnostic returns 1 diagnostic with "error 2"

        const MESSAGE_ONE: &str = "error 1";
        const MESSAGE_TWO: &str = "error 2";

        let (path, uri) = test_file("test.rs");
        let log = Arc::new(Mutex::new(TestLog::default()));
        let mut mock = MockTransport::new(Arc::clone(&log));

        // First DocumentSymbols command: should return symbols + notification with "error 1"
        mock.push_response(json!([symbol_json("main", 12, 0)]));
        mock.push_notifications(vec![publish_diagnostics_notification(&uri, &[MESSAGE_ONE])]);

        // Second DocumentSymbols command: should return symbols + notification with "error 2"
        mock.push_response(json!([symbol_json("main", 12, 0)]));
        mock.push_notifications(vec![publish_diagnostics_notification(&uri, &[MESSAGE_TWO])]);

        // do_diagnostics sends a dummy documentSymbol request => we need a dummy response
        mock.push_response(json!([]));

        let (cmd_tx, _status_tx, _log) = spawn_actor(mock);
        tokio::time::pause();

        // Send first command
        send_document_symbols(&cmd_tx, &path).await.unwrap();
        // Send second command
        send_document_symbols(&cmd_tx, &path).await.unwrap();

        // get diagnostics
        let diags = send_diagnostics(&cmd_tx, &path).await.unwrap();

        // check
        assert_eq!(
            diags.len(),
            1,
            "Second notification should replace first, not accumulate"
        );
        assert_eq!(diags[0].message, MESSAGE_TWO);
    }

    #[tokio::test]
    async fn store_diagnostics_removes_on_empty() {
        // Goal: publishDiagnostics with empty vec removes the entry.
        //       This means "this file has no more errors"
        //
        // Setup: pre load 3 commands
        // 1.- DocumentSymbols -> notification with "error 1"
        // 2.- DocumentSymbols -> notification with empty diagnostics (same URI)
        // 3.- Diagnostics -> dummy response + lookup
        //
        // Expected: Diagnostics returns empty

        let (path, uri) = test_file("test.rs");
        let log = Arc::new(Mutex::new(TestLog::default()));
        let mut mock = MockTransport::new(Arc::clone(&log));

        // First DocumentSymbols command: should return symbols + notification with "error 1"
        mock.push_response(json!([symbol_json("main`", 12, 0)]));
        mock.push_notifications(vec![publish_diagnostics_notification(&uri, &["error 1"])]);

        // Second DocumentSymbols command: should return symbols + empty notification ( no errors)
        mock.push_response(json!([symbol_json("main`", 12, 0)]));
        mock.push_notifications(vec![publish_diagnostics_notification(&uri, &[])]);

        // do_diagnostics sends a dummy documentSymbol request => we need a dummy response
        mock.push_response(json!([]));

        let (cmd_tx, _status_tx, _log) = spawn_actor(mock);
        tokio::time::pause();

        // Send first command
        send_document_symbols(&cmd_tx, &path).await.unwrap();
        // Send second command
        send_document_symbols(&cmd_tx, &path).await.unwrap();

        // get diagnostics
        let diags = send_diagnostics(&cmd_tx, &path).await.unwrap();

        // check
        assert!(diags.is_empty(), "Empty notification should remove the entry");
    }

    #[tokio::test]
    async fn document_symbols_retries_on_empty() {
        // Goal: do_document_symbols retries 5 times on empty results,
        //        then returns OK(empty_vec), not an error
        //
        // Setup: pre load 5 empty responses, so parse_document_symbols returns empty
        //
        // Expected: Ok(vec![])

        let log = Arc::new(Mutex::new(TestLog::default()));
        let mut mock = MockTransport::new(Arc::clone(&log));

        for _ in 0..5 {
            mock.push_response(json!([]));
        }

        let (cmd_tx, _, log) = spawn_actor(mock);
        tokio::time::pause();

        let (path, _) = test_file("test.rs");
        let symbols = send_document_symbols(&cmd_tx, &path).await.unwrap();
        assert!(symbols.is_empty());

        let log = log.lock().unwrap();
        assert_eq!(log.methods_called.len(), 5, "Should have made exactly 5 attempts");
    }

    #[tokio::test]
    async fn document_symbols_returns_first_nonempty() {
        // Goal: retry loop stops at the first non-empty result
        //
        // Setup: pre load 2 empty responses + 1 with a symbol.
        //        As there are only 3 responses pre loaded if the actor try a 4th
        //        the mock will return Err, failing the test.
        //
        // Expected: returns the symbol from attempt 2

        let log = Arc::new(Mutex::new(TestLog::default()));
        let mut mock = MockTransport::new(Arc::clone(&log));

        mock.push_response(json!([]));
        mock.push_response(json!([]));
        mock.push_response(json!([symbol_json("main", 12, 0)]));

        let (cmd_tx, _, log) = spawn_actor(mock);
        tokio::time::pause();

        let (path, _) = test_file("test.rs");
        let symbols = send_document_symbols(&cmd_tx, &path).await.unwrap();
        assert_eq!(symbols.len(), 1);
        assert_eq!(symbols[0].name, "main");

        let log = log.lock().unwrap();
        assert_eq!(log.methods_called.len(), 3, "Should stop after 3 attempts");
    }

    #[tokio::test]
    async fn goto_definition_retries_on_null() {
        // Goal: do_goto_definition retries up to 3 times on null
        //      then returns Ok(empty_vec)
        //
        // SetUp: pre load 3 null responses
        //
        // Expected: Ok(vec![])

        let log = Arc::new(Mutex::new(TestLog::default()));
        let mut mock = MockTransport::new(Arc::clone(&log));

        mock.push_response(Value::Null);
        mock.push_response(Value::Null);
        mock.push_response(Value::Null);

        let (cmd_tx, _, log) = spawn_actor(mock);
        tokio::time::pause();

        let (path, _) = test_file("test.rs");
        let (tx, rx) = oneshot::channel();

        cmd_tx
            .send(LspCommand::GotoDefinition {
                path,
                line: 10,
                character: 5,
                reply: tx,
            })
            .await
            .unwrap();

        let locations = rx.await.unwrap().unwrap();
        assert!(locations.is_empty());

        let log = log.lock().unwrap();
        assert_eq!(log.methods_called.len(), 3, "Should retry 3 times");
    }

    #[tokio::test]
    async fn goto_definition_parses_scalar() {
        // Goal: a single Location response (GotoDefinitionResponse::Scalar)
        //       is parsed correctly into Vec<Location> with 1 element

        let log = Arc::new(Mutex::new(TestLog::default()));
        let mut mock = MockTransport::new(Arc::clone(&log));

        // pre-load the response for GotoDefinition
        // Symbol is defined in the "target.rs" file
        const TARGET_SYMBOL_FILE_URI: &str = "file:///tmp/target.rs";
        const TARGET_SYMBOL_LINE: u32 = 5;
        mock.push_response(json!({
            "uri": TARGET_SYMBOL_FILE_URI,
            "range": {
                "start": {"line": TARGET_SYMBOL_LINE, "character": 0},
                "end": {"line": TARGET_SYMBOL_LINE, "character": 10},
            }
        }));

        let (cmd_tx, ..) = spawn_actor(mock);
        tokio::time::pause();

        let (path, _) = test_file("test.rs");
        let (tx, rx) = oneshot::channel();

        // ask to go to the definition of the symbol in "test.rs" file, line 10 character 5
        cmd_tx
            .send(LspCommand::GotoDefinition {
                path,
                line: 10,
                character: 5,
                reply: tx,
            })
            .await
            .unwrap();

        let locations = rx.await.unwrap().unwrap();

        assert_eq!(locations.len(), 1);
        assert_eq!(locations[0].uri.as_str(), TARGET_SYMBOL_FILE_URI);
        assert_eq!(locations[0].range.start.line, TARGET_SYMBOL_LINE);
    }

    #[tokio::test]
    async fn goto_definition_parses_array() {
        // Goal: an array of Locations( GotoDefinitionResponse::Array)
        //       is parsed correctly

        let log = Arc::new(Mutex::new(TestLog::default()));
        let mut mock = MockTransport::new(Arc::clone(&log));

        const SYMBOL_DEFINITION_FILE_URI_A: &str = "file:///tmp/a.rs";
        const SYMBOL_DEFINITION_FILE_URI_B: &str = "file:///tmp/b.rs";
        mock.push_response(json!([
            {
                "uri": SYMBOL_DEFINITION_FILE_URI_A,
                "range": {
                    "start": {"line": 1, "character": 0},
                    "end": {"line": 1, "character": 5}
                }
            },
            {
                "uri": SYMBOL_DEFINITION_FILE_URI_B,
                "range": {
                    "start": {"line": 2, "character": 0},
                    "end": {"line": 2, "character": 5}
                }
            }
        ]));

        let (cmd_tx, ..) = spawn_actor(mock);
        tokio::time::pause();

        let (path, _) = test_file("test.rs");
        let (tx, rx) = oneshot::channel();
        cmd_tx
            .send(LspCommand::GotoDefinition {
                path,
                line: 10,
                character: 5,
                reply: tx,
            })
            .await
            .unwrap();

        let locations = rx.await.unwrap().unwrap();
        assert_eq!(locations.len(), 2);
        assert_eq!(locations[0].uri.as_str(), SYMBOL_DEFINITION_FILE_URI_A);
        assert_eq!(locations[1].uri.as_str(), SYMBOL_DEFINITION_FILE_URI_B);
    }

    #[tokio::test]
    async fn references_retries_on_null() {
        // Goal: do_references retries on null, returns valid result on 3rd attempt.
        //
        // Setup: 2 nulls + 1 valid array.
        //
        // Expected: returns the location from attempt 2.

        let log = Arc::new(Mutex::new(TestLog::default()));
        let mut mock = MockTransport::new(Arc::clone(&log));

        mock.push_response(Value::Null);
        mock.push_response(Value::Null);

        const LOCATION_LINE: u32 = 20;
        mock.push_response(json!([{
            "uri": "file:///tmp/caller.rs",
            "range": {
                "start": {"line": LOCATION_LINE, "character": 0},
                "end": {"line": LOCATION_LINE, "character": 10}
            }
        }]));

        let (cmd_tx, _, log) = spawn_actor(mock);
        tokio::time::pause();

        let (path, _) = test_file("test.rs");
        let (tx, rx) = oneshot::channel();

        cmd_tx
            .send(LspCommand::References {
                path,
                line: 10,
                character: 5,
                reply: tx,
            })
            .await
            .unwrap();

        let locations = rx.await.unwrap().unwrap();
        assert_eq!(locations.len(), 1);
        assert_eq!(locations[0].range.start.line, LOCATION_LINE);

        let log = log.lock().unwrap();
        assert_eq!(log.methods_called.len(), 3, "should retry on null");
    }

    #[tokio::test]
    async fn ensure_document_open_caches_second_call() {
        // Goal: two commands for the same file call open_document only once.
        //       The second call hits the open_docs cache.
        //
        // Verification: inspect log.opened — must have exactly 1 entry.

        let log = Arc::new(Mutex::new(TestLog::default()));
        let mut mock = MockTransport::new(Arc::clone(&log));

        // same response for each
        mock.push_response(json!([symbol_json("main", 12, 0)]));
        mock.push_response(json!([symbol_json("main", 12, 0)]));

        let (cmd_tx, _, log) = spawn_actor(mock);
        tokio::time::pause();

        let (path, _) = test_file("test.rs");

        // first command opens the file
        send_document_symbols(&cmd_tx, &path).await.unwrap();
        // second command should hit cache
        send_document_symbols(&cmd_tx, &path).await.unwrap();

        let log = log.lock().unwrap();
        assert_eq!(
            log.opened.len(),
            1,
            "Second call should hit cache, not call open_document again"
        );
        assert_eq!(log.opened[0], path);
    }

    #[tokio::test]
    async fn evict_lru_closes_oldest_document() {
        // Goal: when max_open_documents is reached, opening one more file
        //       evicts the least recently used document.
        //
        // Setup: max_open_documents + 1 DocumentSymbols commands for different paths.
        //
        // Verification: inspect log.closed — must contain the URI of file 0
        //   (it was opened first, has the oldest last_used).

        let capacity = DEFAULT_MAX_OPEN_DOCUMENTS;

        let log = Arc::new(Mutex::new(TestLog::default()));
        let mut mock = MockTransport::new(Arc::clone(&log));
        for _ in 0..=capacity {
            mock.push_response(json!([symbol_json("fn", 12, 0)]));
        }

        let (cmd_tx, _, log) = spawn_actor(mock);
        tokio::time::pause();

        // open `capacity` files, then one more to trigger eviction
        for i in 0..=capacity {
            let (path, _) = test_file(&format!("file_{i}.rs"));
            send_document_symbols(&cmd_tx, &path).await.unwrap();
        }

        let log = log.lock().unwrap();
        assert_eq!(log.closed.len(), 1, "Exactly 1 document should have been evicted");
        let (_, expected_uri) = test_file("file_0.rs");
        assert_eq!(log.closed[0], expected_uri, "file_0 should be evicted (oldest)");
    }

    #[tokio::test]
    async fn evict_lru_removes_diagnostics() {
        // Goal: evicting a document also removes its diagnostics.
        //
        // Setup:
        //   1. Open file_0 with a publishDiagnostics notification ("error")
        //   2. Open files 1..(capacity-1) (plain)
        //   3. Open file_capacity → triggers eviction of file_0
        //   4. Query Diagnostics for file_0
        //
        // Note: do_diagnostics calls ensure_document_open, which re-opens
        //   file_0 (evicted). This triggers ANOTHER eviction (file_1,
        //   the new oldest). So the total responses needed:
        //   (capacity + 1) DocumentSymbols + 1 dummy for do_diagnostics.
        //
        // Expected: Diagnostics returns empty (diagnostics were removed by
        //   the first eviction, and the re-open doesn't restore them).

        let capacity = DEFAULT_MAX_OPEN_DOCUMENTS;

        let (file_0_path, file_0_uri) = test_file("file_0.rs");
        let log = Arc::new(Mutex::new(TestLog::default()));
        let mut mock = MockTransport::new(Arc::clone(&log));

        // file_0: response + notification
        mock.push_response(json!([symbol_json("fn", 12, 0)]));
        mock.push_notifications(vec![publish_diagnostics_notification(&file_0_uri, &["error"])]);

        // files 1..(capacity-1): plain responses
        for _ in 1..capacity {
            mock.push_response(json!([symbol_json("fn", 12, 0)]));
        }

        // file_capacity: triggers eviction of file_0
        mock.push_response(json!([symbol_json("fn", 12, 0)]));

        // do_diagnostics for file_0 => reopens file_0 and evicts file_1,
        // then sends dummy documentSymbol request
        mock.push_response(json!([]));

        let (cmd_tx, _, log) = spawn_actor(mock);
        tokio::time::pause();

        // send commands for all files up to `capacity` + 1
        for idx in 0..=capacity {
            let (path, _) = test_file(&format!("file_{idx}.rs"));
            send_document_symbols(&cmd_tx, &path).await.unwrap();
        }

        // send diagnostics for file_0 (re open file_0 and evicts file_1)
        let diags = send_diagnostics(&cmd_tx, &file_0_path).await.unwrap();
        assert!(diags.is_empty(), "Diagnostics should be removed by eviction");

        // Verify eviction happened for both file_0 and file_1
        let log = log.lock().unwrap();
        assert_eq!(log.closed.len(), 2);
        let (_, file_1_uri) = test_file("file_1.rs");
        assert_eq!(log.closed[0], file_0_uri, "file_0 evicted first");
        assert_eq!(log.closed[1], file_1_uri, "file_1 evicted second");
    }
}
