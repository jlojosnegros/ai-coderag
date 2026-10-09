//! Background actor that owns an LspClient and processes commands.
//! 
//! The actor runs in a tokio::spawn task.
//! Commands arrive via an mpsc channel.
//! During each request to the LSP server, notifications (like publishDiagnostics)
//! are captured via request_capturing and processed by process_captured_notifications

use std::{collections::HashMap, path::{Path, PathBuf}, time::Instant};

use candle_core::error;
use lsp_types::{CallHierarchyIncomingCall, CallHierarchyItem, CallHierarchyOutgoingCall, ClientCapabilities, Diagnostic, Location, Uri};
use tokio::sync::{mpsc, oneshot, watch};

use crate::{LanguageLsp, LspServerConfig, Result, lsp::{DocumentSymbol, LspHandle, LspStatus, client::LspClient}};

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
    Shutdown,
}

/// Metadata for a currently open document.
/// 
/// Information is used for LRU eviction when reach open documents limit
struct OpenDocument {
    uri: Uri,
    last_used: Instant,
}

///Background actor that owns an LspClient and processes commands.
/// 
/// Created by LspActor::spawn, which returns an LspHandle.
/// The actor runs until the command channel closes ( all handles dropped)
pub(crate) struct LspActor {
    client: LspClient,
    commands: mpsc::Receiver<LspCommand>,
    status_tx: watch::Sender<LspStatus>,
    
    ///Diagnostics received via publishDiagnostics notifications.
    /// Keyed by file URI string. Each entry is the LATEST diagnostics for
    /// that file (each notification replaces the previous one, per LSP spec)
    diagnostics: HashMap<String, Vec<Diagnostic>>,
    
    /// Currently open documents, keyed by absolute path string.
    /// Used for LRU eviction when max_open_documents is reached.
    open_docs: HashMap<String,OpenDocument>,

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
            }
            Err(err) => {
                let reason = err.to_string();
                tracing::warn!("LSP actor failed for {language_id}: {reason}");
                let _ = status_tx.send(LspStatus::Failed(reason));
                return;
            }
        };

        let mut actor = LspActor {
            client,
            commands,
            status_tx: status_tx,
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

    }
}