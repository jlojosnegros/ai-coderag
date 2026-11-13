use std::path::Path;

use async_trait::async_trait;
use lsp_types::Uri;
use serde_json::Value;

use crate::Result;

/// Abstraction over the LSP client transport layer
///
/// The `LspActor` uses this trait instead of LspClient directly,
/// enabling mock injection in tests.
#[async_trait]
pub(crate) trait LspTransport: Send {
    /// Send a request and wait for the response with the matching id.
    /// Stores all notifications that arrive before the reponse in `captured`
    /// for later process
    async fn request_capturing(&mut self, method: &str, params: Value, captured: &mut Vec<Value>) -> Result<Value>;

    /// Open a document in the server
    /// required before any textDocument request
    async fn open_document(&mut self, path: &Path) -> Result<Uri>;

    async fn close_document(&mut self, uri: &Uri) -> Result<()>;

    /// Cleanly shut down the server.
    /// Must be called before dropping the client
    async fn shutdown(&mut self) -> Result<()>;
}
