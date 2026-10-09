//! Public interface to an LSP language server managed by a background actor
//!
//! Tools interact with the LSP through LspHandle, NEVER directly with LspClient.
//! The handle sends commands to the actor via a channel and receives responses
//! via oneshot channels.

use std::path::Path;

use lsp_types::{CallHierarchyIncomingCall, CallHierarchyItem, CallHierarchyOutgoingCall, Diagnostic, Location};
use tokio::sync::{mpsc, oneshot, watch};

use crate::{
    CoderagError, Result,
    lsp::{
        self, LspStatus,
        actor::{LspActor, LspCommand},
    },
    traits::{LanguageLsp, LspServerConfig},
};

/// Interface to an LSP language server.
///
/// This is what MCP tool handlers use.
/// Each method sends a command to the background actor and awaits the response.
/// The handle is cheap to clone
#[derive(Clone)]
pub struct LspHandle {
    commands: mpsc::Sender<LspCommand>,
    status: watch::Receiver<LspStatus>,
    language_id: String,
}
impl LspHandle {
    /// Creates a handle with pre-built channels.
    pub(crate) fn new(
        commands: mpsc::Sender<LspCommand>,
        status: watch::Receiver<LspStatus>,
        language_id: String,
    ) -> Self {
        Self {
            commands,
            status,
            language_id,
        }
    }

    /// Spawn a new LSP actor and return a handle to it.
    ///
    /// Create channels, extract capabilities and language_id from the trait object
    /// (they must be owned to cross tokio::spawn boundary), launches the actor
    /// in a background task, and returns immmediately with status = Initializing.
    ///
    /// Check `is_ready()` or `status()` to know when the LSP is available
    pub fn spawn(lsp: &(dyn LanguageLsp + Send + Sync), config: &LspServerConfig, root_path: &Path) -> LspHandle {
        //TODO - the number of buffered commands should be configurable somehow
        let (cmd_tx, cmd_rx) = mpsc::channel::<LspCommand>(32);
        let (status_tx, status_rx) = watch::channel(LspStatus::Initializing);
        let language_id = lsp.language_id().to_string();

        let config = config.clone();
        let root_path = root_path.to_path_buf();
        let capabilities = lsp.initialize_capabilities();
        let lsp_language_id = language_id.clone();

        tokio::spawn(async move {
            LspActor::run(&config, &root_path, capabilities, lsp_language_id, cmd_rx, status_tx).await
        });

        Self {
            commands: cmd_tx,
            status: status_rx,
            language_id,
        }
    }

    /// Current status of the language server, without blocking
    pub fn status(&self) -> LspStatus {
        self.status.borrow().clone()
    }

    /// The language_id this handle manages( e.g. "rust", "cpp")
    pub fn language_id(&self) -> &str {
        &self.language_id
    }

    /// Check if the LSP is ready to accepts requests
    pub fn is_ready(&self) -> bool {
        matches!(*self.status.borrow(), LspStatus::Ready)
    }

    /// Send a command to the actor, Returns Err if the actor has stopped
    async fn send(&self, cmd: LspCommand) -> Result<()> {
        self.commands
            .send(cmd)
            .await
            .map_err(|_| CoderagError::Lsp("actor stopped".into()))
    }

    // === LSP operations ===

    /// Create a oneshot channel, build the LspCommand via the closure ( which
    /// receives the reply sender), send it to the actor, and await the response.
    async fn send_and_receive<T>(&self, make_cmd: impl FnOnce(oneshot::Sender<Result<T>>) -> LspCommand) -> Result<T> {
        let (tx, rx) = oneshot::channel();
        self.send(make_cmd(tx)).await?;
        rx.await.map_err(|_| CoderagError::Lsp("actor dropped".into()))?
    }

    /// Get all symbols defined in a file (hierarchical)
    pub async fn document_symbols(&self, path: &Path) -> Result<Vec<lsp::DocumentSymbol>> {
        self.send_and_receive(|reply| LspCommand::DocumentSymbols {
            path: path.to_path_buf(),
            reply,
        })
        .await
    }

    /// Go to the definition of the symbol at the given position
    pub async fn goto_definition(&self, path: &Path, line: u32, character: u32) -> Result<Vec<Location>> {
        self.send_and_receive(|reply| LspCommand::GotoDefinition {
            path: path.to_path_buf(),
            line,
            character,
            reply,
        })
        .await
    }

    /// Find all references to the symbol at the given position
    pub async fn references(&self, path: &Path, line: u32, character: u32) -> Result<Vec<Location>> {
        self.send_and_receive(|reply| LspCommand::References {
            path: path.to_path_buf(),
            line,
            character,
            reply,
        })
        .await
    }

    /// Prepare a call hierarchy item at the given position.
    /// Returns the item(s) that can be pased to incoming_calls/outgoing_calls.
    pub async fn prepare_call_hierarchy(
        &self,
        path: &Path,
        line: u32,
        character: u32,
    ) -> Result<Vec<CallHierarchyItem>> {
        self.send_and_receive(|reply| LspCommand::PrepareCallHierarchy {
            path: path.to_path_buf(),
            line,
            character,
            reply,
        })
        .await
    }

    /// Get incoming calls (callers) for a call hierarchy item.
    pub async fn incomming_calls(&self, item: CallHierarchyItem) -> Result<Vec<CallHierarchyIncomingCall>> {
        self.send_and_receive(|reply| LspCommand::IncomingCalls { item, reply })
            .await
    }

    /// Get outgoing calls (callees) for a call hierarchy item.
    pub async fn outgoing_calls(&self, item: CallHierarchyItem) -> Result<Vec<CallHierarchyOutgoingCall>> {
        self.send_and_receive(|reply| LspCommand::OutgoingCalls { item, reply })
            .await
    }

    /// Get the current diagnostics for a file.
    /// Returns whatever the server has most recentrly pushed via publishDiagnostics.
    /// If the file has not been analyzed yet, returns empty vec
    pub async fn diagnostics(&self, path: &Path) -> Result<Vec<Diagnostic>> {
        self.send_and_receive(|reply| LspCommand::Diagnostics {
            path: path.to_path_buf(),
            reply,
        })
        .await
    }
}
