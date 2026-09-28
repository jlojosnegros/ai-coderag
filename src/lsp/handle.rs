//! Public interface to an LSP language server managed by a background actor
//!
//! Tools interact with the LSP through LspHandle, NEVER directly with LspClient.
//! The handle sends commands to the actor via a channel and receives responses
//! via oneshot channels.

use std::path::Path;

use tokio::sync::{mpsc, watch};

use crate::lsp::{LspStatus, actor::LspCommand};


/// Maximum number of concurrently open documents in a single LSP server.
/// When this limit is reached, the actor closes the least recentrly used
/// document before opening a new one.
const DEFAULT_MAX_OPEN_DOCUMENTS: usize = 50;

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
    /// Launches the language server process in a background task and performs
    /// the initialize handshake. The handle is returned immmediately with
    /// status = Initializing.
    /// Check `is_ready()` or `status()` to know when the LSP is available
    // pub fn spawn (
    //     lsp: &(dyn crate::traits::LanguageLsp + Send + Sync),
    //     config: &crate::config::LspServerConfig,
    //     root_path: &Path,
    // ) -> Self {
    //     Self {
    //         super::actor::LspActor::spawn(lsp, config, root_path)
    //     }
    // }

    pub fn status(&self) -> LspStatus {
        self.status.borrow().clone()
    }
}
