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

use candle_core::error;
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
    LanguageLsp, LspServerConfig, Result,
    lsp::{DocumentSymbol, LspHandle, LspStatus, client::LspClient, parse_document_symbols},
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
    client: LspClient,
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
                client
            },
            Err(err) => {
                let reason = err.to_string();
                tracing::warn!("LSP actor failed for {language_id}: {reason}");
                let _ = status_tx.send(LspStatus::Failed(reason));
                return;
            },
        };

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
        let mut captured = Vec::new();
        let result = self.client.request_capturing(method, params, &mut captured).await?;
        self.process_captured_notifications(captured);
        Ok(result)
    }
}
