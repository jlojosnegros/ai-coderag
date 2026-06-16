use std::{
    fs::read_to_string,
    path::Path,
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

use lsp_types::{DocumentSymbol as LspDocSymbol, Location as LspLocation, SymbolKind as LspSymbolKind};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, ChildStdout, Command},
    time::timeout,
};
use url::Url;

use crate::{CoderagError, Result};

/// A sequential LSP client that talks to a language server over stdio.
///
/// "Sequential" means: send -> skip notifications -> receive.
/// This is enough for batch indexing and much simpler than
/// a multiplexed client with background reader tasks
pub struct LspClient {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: AtomicU64,
    timeout_secs: u64,
    path_filter: String,
}

impl LspClient {
    /// Spawn rust-analyzer and perform the LSP initialize handshake.
    ///
    /// `root_path` must be the directory contaning `Cargo.toml`
    pub async fn new_rust_analyzer(rust_analyzer_bin: &str, root_path: &Path, timeout_secs: u64) -> Result<Self> {
        let root_uri = path_to_file_uri(root_path)?;
        let path_filter = root_path.to_string_lossy().to_string();

        let mut child = Command::new(rust_analyzer_bin)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|err| CoderagError::Lsp(format!("failed to spawn {rust_analyzer_bin}: {err}")))?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| CoderagError::Lsp("child has no stdin".to_string()))?;

        let stdout_raw = child
            .stdout
            .take()
            .ok_or_else(|| CoderagError::Lsp("child has not stdout".to_string()))?;
        let stdout = BufReader::new(stdout_raw);

        let mut client = Self {
            child,
            stdin,
            stdout,
            next_id: AtomicU64::new(0),
            timeout_secs,
            path_filter,
        };

        client.initialize(&root_uri).await?;
        tracing::info!("LSP client initialized for {}", root_path.display());

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

        // Read exactly content_length bytes
        let mut body = vec![0u8; content_length];
        self.stdout
            .read_exact(&mut body)
            .await
            .map_err(|e| CoderagError::Lsp(format!("read body: {e}")))?;

        serde_json::from_slice(&body).map_err(|e| CoderagError::Lsp(format!("deserialize body: {e}")))
    }

    /// Send a request and wait for the response with the matching id.
    /// Discards all notifications that arrive before the response.
    ///
    /// rust-analyzer send proactive notifications at any moment.
    /// - `$/progress` -> indexing in progress ("indexing 15%", "indexing 30%", etc)
    /// - `textDocument/publishDiagnostics` -- compilation errors found
    /// - `window/logMessage` -- server internal logs
    ///
    /// This notifications are sent in between requests and responses
    /// so we need to discard anything that is not a response with the
    /// very same Id of our request
    async fn request(&mut self, method: &str, params: Value) -> Result<Value> {
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

            // If reach here: this is a notification or a response with a
            // different id
            // As this is a sequential client there should not be any other
            // reponse than our own response so we can safely say this is
            // a server notification
            tracing::trace!(
                "Discarding notification: {}",
                msg.get("method").and_then(serde_json::Value::as_str).unwrap_or("?")
            );
        }
    }

    /// Send notification. No response expected
    async fn notify(&mut self, method: &str, params: Value) -> Result<()> {
        self.send(&json!({
            "jsonrpc": "2.0",
            "method" : method,
            "params": params,
        }))
        .await
    }

    // --- LSP lifecycle ---
    async fn initialize(&mut self, root_uri: &str) -> Result<()> {
        let _result = self
            .request(
                "initialize",
                json!({
                    "processId": std::process::id(),
                    "rootUri": root_uri,
                    "capabilities": {
                        "textDocument": {
                            "documentSymbol" : {
                                // true means server return DocumentSymbol[] with selectionRange
                                // and children.
                                // false means server returns SymbolInformation[] without
                                // selectionRange, which is not enough for references_at
                                // positioning
                                "hierarchicalDocumentSymbolSupport": true
                            },
                            "references": {}
                        }
                    }
                }),
            )
            .await?;

        // The "initialized" notification must be sent after receiving the initialize response
        self.notify("initialized", json!({})).await?;

        Ok(())
    }

    /// Cleanly shut down the server.
    /// Must be called before dropping the client
    pub async fn shutdown(&mut self) -> Result<()> {
        let _ = self.request("shutdown", json!(null)).await;
        let _ = self.notify("exit", json!(null)).await;
        Ok(())
    }

    // --- LSP queries ---

    /// Open a document in the server
    /// required before any textDocument request
    async fn open_document(&mut self, file_path: &Path, language_id: &str) -> Result<String> {
        let content = read_to_string(file_path).map_err(CoderagError::Io)?;
        let uri = path_to_file_uri(file_path)?;

        self.notify(
            "textDocument/didOpen",
            json!({
                "textDocument": {
                    "uri": uri,
                    "languageId": language_id,
                    "version": 1,
                    "text": content,
                }
            }),
        )
        .await?;

        Ok(uri)
    }

    async fn close_document(&mut self, uri: &str) -> Result<()> {
        self.notify(
            "textDocument/didClose",
            json!({
                "textDocument":{
                    "uri": uri
                }
            }),
        )
        .await
    }

    /// Get all named symbols in a file.
    /// Returns a list of (name, kond, start_line) tuples
    /// Line numbers are 0-based (lsp convention)
    pub async fn document_symbols(&mut self, file_path: &Path) -> Result<Vec<DocumentSymbol>> {
        let uri = self.open_document(file_path, "rust").await?;

        // give rust-analyzer a moment to parse the file.
        // whitout this it may return an empty result for the first request
        tokio::time::sleep(Duration::from_millis(200)).await;

        let result = self
            .request(
                "textDocument/documentSymbol",
                json!({
                    "textDocument" : {
                        "uri" : uri
                    }
                }),
            )
            .await?;

        let _ = self.close_document(&uri).await;

        parse_document_symbols(result)
    }

    pub async fn references_at(&mut self, file_path: &Path, line: u32, character: u32) -> Result<Vec<String>> {
        let uri = self.open_document(file_path, "rust").await?;

        // Give rust analyzer time to analyze the file.
        tokio::time::sleep(Duration::from_millis(500)).await;

        let result = self
            .request(
                "textDocument/references",
                json!({
                    "textDocument": { "uri": uri},
                    "position" : {"line": line, "character": character},
                    "context" : { "includeDeclaration": false}
                }),
            )
            .await?;

        let _ = self.close_document(&uri).await;

        let locations: Vec<LspLocation> = serde_json::from_value(result).unwrap_or_default();
        let mut callers = Vec::new();
        for loc in locations {
            // filter out references from external crates
            // lsp_types::Uri has no to_file_path(), parse as url::Url first.
            let url = match Url::parse(loc.uri.as_str()) {
                Ok(u) => u,
                Err(e) => {
                    tracing::warn!("references_at: skipping malformed URI '{}': {e}", loc.uri.as_str());
                    continue;
                },
            };

            if !url.path().contains(self.path_filter.as_str()) {
                continue;
            }
            if let Ok(ref_path) = url.to_file_path() {
                let display = format!(
                    "{}:{}",
                    ref_path.file_name().unwrap_or_default().to_string_lossy(),
                    loc.range.start.line + 1
                );
                callers.push(display);
            }
        }
        Ok(callers)
    }
}

impl Drop for LspClient {
    fn drop(&mut self) {
        // best-effort kill if shutdown() was not called explicitly
        let _ = self.child.start_kill();
    }
}

#[derive(Debug, Clone)]
pub struct DocumentSymbol {
    pub name: String,
    pub kind: SymbolKind,
    /// 0-based line number of the symbol name
    pub selection_start_line: u32,
    /// 0-based char offset of the symbol name
    pub selection_start_char: u32,
}

#[derive(Debug, Clone)]
pub enum SymbolKind {
    Function,
    Method,
    Struct,
    Enum,
    Interface, // Trait in Rust
    Other,
}
impl SymbolKind {
    fn from_lsp_kind(kind: LspSymbolKind) -> Self {
        // LspSymbolKind is a newtype struct (not a Rust enum) because the LSP spec
        // allows servers to define extension values. Pattern matching on struct
        // constants is not supported, so we use if/else with associated constants.
        if kind == LspSymbolKind::FUNCTION {
            Self::Function
        } else if kind == LspSymbolKind::METHOD {
            Self::Method
        } else if kind == LspSymbolKind::STRUCT {
            Self::Struct
        } else if kind == LspSymbolKind::ENUM {
            Self::Enum
        } else if kind == LspSymbolKind::INTERFACE {
            Self::Interface
        } else {
            Self::Other
        }
    }
}

// --- Helpers ---

fn parse_document_symbols(result: Value) -> Result<Vec<DocumentSymbol>> {
    let raw: Vec<LspDocSymbol> = serde_json::from_value(result)
        .map_err(|err| CoderagError::Lsp(format!("failed to parse documentSymbol response: {err}")))?;
    let mut out = Vec::new();
    flatten_symbols(raw, &mut out);
    Ok(out)
}

/// Recursively flattens the hierarchical DocumentSymbol tree into a flat list.
/// With hierarchicalDocumentSymbolSupport: true, rust-analyzer nest methods under
/// their parent struct/impl block. This collects all symbols depth-first so that
/// callers can iterate a simple Vec without caring about nesting
fn flatten_symbols(symbols: Vec<LspDocSymbol>, out: &mut Vec<DocumentSymbol>) {
    for sym in symbols {
        let children = sym.children.unwrap_or_default();
        if !sym.name.is_empty() {
            out.push(DocumentSymbol {
                name: sym.name,
                kind: SymbolKind::from_lsp_kind(sym.kind),
                selection_start_line: sym.selection_range.start.line,
                selection_start_char: sym.selection_range.start.character,
            });
        }
        flatten_symbols(children, out);
    }
}

fn path_to_file_uri(path: &Path) -> Result<String> {
    let abs = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().map_err(CoderagError::Io)?.join(path)
    };

    Url::from_file_path(&abs)
        .map(|url| url.to_string())
        .map_err(|_| CoderagError::Lsp(format!("cannot convert path to URI: {}", abs.display())))
}
