use std::{
    fs::read_to_string,
    path::Path,
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

use async_trait::async_trait;
use lsp_types::{
    ClientCapabilities, DidCloseTextDocumentParams, DidOpenTextDocumentParams, InitializeParams,
    TextDocumentIdentifier, TextDocumentItem, Uri,
};
use serde::Serialize;
use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, ChildStdout},
    time::timeout,
};

use crate::{
    CoderagError, Result,
    lsp::{LspTransport, path_to_file_uri},
    traits::{LanguageLsp, LspServerConfig},
};

/// A sequential LSP client that talks to a language server over stdio.
///
/// "Sequential" means: send -> skip notifications -> receive.
/// This is enough for batch indexing and much simpler than
/// a multiplexed client with background reader tasks
pub(crate) struct LspClient {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: AtomicU64,
    timeout_secs: u64,
    path_filter: String,

    /// LSP language identifier (e.g. "rust", "cpp", "go"), used in didOpen
    language_id: String,
}

impl LspClient {
    /// Spawn a language server and perform the LSP initialize handshake.
    ///
    /// - `lsp` provides language-specific knowledge( capabilities, language id, etc)
    /// - `config` provides deployment details ( binary path, args, timeout, etc)
    /// - `root_path` must be the project root for the language (e.g. directory containing Cargo.toml for Rust)
    #[deprecated(note = "use new_from_parts; this wrapper will be removed when all callers migrate")]
    pub async fn new(lsp: &dyn LanguageLsp, config: &LspServerConfig, root_path: &Path) -> Result<Self> {
        let capabilities = lsp.initialize_capabilities();
        let language_id = lsp.language_id();
        Self::new_from_parts(config, root_path, &capabilities, language_id).await
    }

    /// Creates an LspClient by spawning the process and performing the
    /// initialize handshake.
    pub(crate) async fn new_from_parts(
        config: &LspServerConfig,
        root_path: &Path,
        capabilities: &ClientCapabilities,
        language_id: &str,
    ) -> Result<Self> {
        let root_uri = path_to_file_uri(root_path)?;
        let path_filter = root_path.to_string_lossy().to_string();

        let mut child = tokio::process::Command::new(&config.command)
            .args(&config.args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|err| CoderagError::Lsp(format!("failed to spawn {}: {err}", config.command)))?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| CoderagError::Lsp("child has no stdin".into()))?;

        let stdout_raw = child
            .stdout
            .take()
            .ok_or_else(|| CoderagError::Lsp("child has no stdout".into()))?;

        let mut client = Self {
            child,
            stdin,
            stdout: tokio::io::BufReader::new(stdout_raw),
            next_id: std::sync::atomic::AtomicU64::new(0),
            timeout_secs: config.timeout_secs,
            path_filter,
            language_id: language_id.to_string(),
        };

        client.initialize(&root_uri, capabilities).await?;

        tracing::info!("LSP client initialized for {} ({})", root_path.display(), language_id);
        Ok(client)
    }

    fn next_id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::SeqCst)
    }

    /// Send a JSON-RPC message with Content-Length framing.
    async fn send(&mut self, msg: &Value) -> Result<()> {
        let body = serde_json::to_string(msg).map_err(|e| CoderagError::Lsp(format!("serialize error: {e}")))?;

        let header = format!("Content-Length: {}\r\n\r\n", body.len());

        self.stdin
            .write_all(header.as_bytes())
            .await
            .map_err(|e| CoderagError::Lsp(format!("write header: {e}")))?;

        self.stdin
            .write_all(body.as_bytes())
            .await
            .map_err(|e| CoderagError::Lsp(format!("write body: {e}")))?;

        self.stdin
            .flush()
            .await
            .map_err(|e| CoderagError::Lsp(format!("flush: {e}")))?;

        Ok(())
    }

    /// Read one JSON-RPC message from server;s stdout
    /// Reads the Content-Length header, then the body
    async fn read_message(&mut self) -> Result<Value> {
        let mut content_length = 0usize;

        // Read headers line by line until a blank line
        loop {
            let mut line = String::new();
            self.stdout
                .read_line(&mut line)
                .await
                .map_err(|e| CoderagError::Lsp(format!("read header line: {e}")))?;

            let trimmed = line.trim();
            if trimmed.is_empty() {
                break;
            }
            if let Some(len_str) = trimmed.strip_prefix("Content-Length: ") {
                content_length = len_str
                    .parse()
                    .map_err(|_| CoderagError::Lsp(format!("bad content-length: {len_str}")))?;
            }
        }

        if content_length == 0 {
            return Err(CoderagError::Lsp(
                "missing Content-Length header in server response".to_string(),
            ));
        }

        // Read exactly content_length bytes
        let mut body = vec![0u8; content_length];
        self.stdout
            .read_exact(&mut body)
            .await
            .map_err(|e| CoderagError::Lsp(format!("read body: {e}")))?;

        serde_json::from_slice(&body).map_err(|e| CoderagError::Lsp(format!("deserialize body: {e}")))
    }

    async fn request_core(
        &mut self,
        method: &str,
        params: Value,
        mut on_notification: impl FnMut(Value),
    ) -> Result<Value> {
        let id = self.next_id();

        self.send(&json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        }))
        .await?;

        let duration = Duration::from_secs(self.timeout_secs);

        loop {
            let msg = timeout(duration, self.read_message())
                .await
                .map_err(|_| CoderagError::Lsp(format!("timeout waiting for {method} response")))??;

            let msg_id = msg.get("id").and_then(Value::as_u64);
            if msg_id == Some(id) {
                // This is the response to our request
                if let Some(error) = msg.get("error") {
                    return Err(CoderagError::Lsp(format!("{method} error: {error}")));
                }

                return Ok(msg.get("result").cloned().unwrap_or(Value::Null));
            }

            on_notification(msg);
        }
    }

    /// Send a request and wait for the response with the matching id.
    /// Discards all notifications that arrive before the response.
    pub(crate) async fn request(&mut self, method: &str, params: impl Serialize) -> Result<Value> {
        let params_value =
            serde_json::to_value(params).map_err(|err| CoderagError::Lsp(format!("serialize params: {err}")))?;
        self.request_core(method, params_value, |msg| {
            tracing::trace!(
                "Discarding notification: {}",
                msg.get("method").and_then(serde_json::Value::as_str).unwrap_or("?")
            );
        })
        .await
    }

    /// Send notification. No response expected
    async fn notify(&mut self, method: &str, params: impl Serialize) -> Result<()> {
        let params_value =
            serde_json::to_value(params).map_err(|err| CoderagError::Lsp(format!("serialize params {err}")))?;

        self.send(&json!({
            "jsonrpc": "2.0",
            "method" : method,
            "params": params_value,
        }))
        .await
    }

    // --- LSP lifecycle ---
    #[allow(deprecated)]
    async fn initialize(&mut self, root_uri: &Uri, capabilities: &ClientCapabilities) -> Result<()> {
        // root_uri deprecated in LSP 3.6 (use workspace_folders)
        // but most LSP servers still require it for workspace init
        let params = InitializeParams {
            process_id: Some(std::process::id()),
            root_uri: Some(root_uri.clone()),
            capabilities: capabilities.clone(),
            ..Default::default()
        };

        let _result = self.request("initialize", params).await?;

        // The "initialized" notification must be sent after receiving the initialize response
        self.notify("initialized", json!({})).await?;

        Ok(())
    }
}

#[async_trait]
impl LspTransport for LspClient {
    /// Send a request and wait for the response with the matching id.
    /// Stores all notifications that arrive before the reponse in `captured`
    /// for later process
    async fn request_capturing(&mut self, method: &str, params: Value, captured: &mut Vec<Value>) -> Result<Value> {
        self.request_core(method, params, |msg| captured.push(msg)).await
    }

    /// Open a document in the server
    /// required before any textDocument request
    async fn open_document(&mut self, path: &Path) -> Result<Uri> {
        let content = read_to_string(path).map_err(CoderagError::Io)?;
        let uri = path_to_file_uri(path)?;

        let params = DidOpenTextDocumentParams {
            text_document: TextDocumentItem::new(uri.clone(), self.language_id.clone(), 1, content),
        };

        self.notify("textDocument/didOpen", params).await?;

        Ok(uri)
    }

    async fn close_document(&mut self, uri: &Uri) -> Result<()> {
        let params = DidCloseTextDocumentParams {
            text_document: TextDocumentIdentifier::new(uri.clone()),
        };
        self.notify("textDocument/didClose", params).await
    }

    /// Cleanly shut down the server.
    /// Must be called before dropping the client
    async fn shutdown(&mut self) -> Result<()> {
        let _ = self.request("shutdown", json!(null)).await;
        let _ = self.notify("exit", json!(null)).await;
        Ok(())
    }
}

impl Drop for LspClient {
    fn drop(&mut self) {
        // best-effort kill if shutdown() was not called explicitly
        let _ = self.child.start_kill();
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use crate::{LspServerConfig, RustLsp, lsp::client::LspClient};

    #[tokio::test]
    async fn new_returns_error_for_missing_binary() {
        let config = LspServerConfig {
            command: "nonexistent-lsp-binary".to_string(),
            args: Vec::new(),
            timeout_secs: 1,
        };
        let result = LspClient::new(&RustLsp, &config, Path::new("/tmp")).await;
        assert!(result.is_err());
    }
}
