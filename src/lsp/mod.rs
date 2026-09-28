pub(crate) mod actor;
mod client;
pub mod handle;
mod rust_lsp;

use std::path::Path;

pub use handle::LspHandle;
use lsp_types::{DocumentSymbol as LspDocSymbol, SymbolKind as LspSymbolKind, Uri};
pub use rust_lsp::RustLsp;
use serde_json::Value;
use url::Url;

use crate::{CoderagError, Result};
/// State of a language server managed by the actor
///
/// Tracked via a `watch` channel: the actor updates it, tools read it to decide
/// whether to use LSP or fall back to tree-sitter
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LspStatus {
    /// The LSP process is starting and performing the initialize handshake.
    /// Tools should fall back to tree-sitter while this is active
    Initializing,

    /// The LSP is ready to accept requests.
    Ready,

    /// The LSP failed to start or crashed. The string contains the reason.
    /// Tools should fall back to tree-sitter permanently for this session.
    Failed(String),
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

#[derive(Debug, Clone, PartialEq, Eq)]
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
    // from: LSP Specification 3.17 - textDocument/documentSymbol
    //       result: DocumentSymbol[] | SymbolInformation[] | null
    // that means that result can be "null" if the LSP haven't parsed
    // the document yet, so we must check it
    if result.is_null() {
        return Ok(Vec::new());
    }
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

/// Convert a filesystem path to a `file://` URI string as required by LSP.
///
/// Accepts both relative and absolute paths. Relative paths are resolved
/// against `std::env::current_dir()` before conversion, because
/// `Url::from_file_path` rejects non-absolute paths.
///
/// Returns `Ok("file:///home/user/project/src/main.rs")` on success.
///
/// # Errors
///
/// - `CoderagError::Io` if the working directory cannot be read (e.g. it was deleted while the process was running).
/// - `CoderagError::Lsp` if `Url::from_file_path` rejects the absolute path (on Windows this happens for paths like
///   `C:foo` that are drive-relative but not fully qualified).
fn path_to_file_uri(path: &Path) -> Result<Uri> {
    let abs = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().map_err(CoderagError::Io)?.join(path)
    };

    let url = Url::from_file_path(&abs)
        .map_err(|_| CoderagError::Lsp(format!("cannot convert path to URI: {}", abs.display())))?;

    url.as_str()
        .parse::<Uri>()
        .map_err(|err| CoderagError::Lsp(format!("invalid URI '{}' : '{err}", url)))
}

#[cfg(test)]
mod tests {
    use lsp_types::{
        ClientCapabilities, DocumentSymbolClientCapabilities, DocumentSymbolParams, InitializeParams,
        PartialResultParams, Position, ReferenceContext, ReferenceParams, TextDocumentClientCapabilities,
        TextDocumentIdentifier, TextDocumentPositionParams, WorkDoneProgressParams,
    };
    use serde_json::json;

    use super::*;


    #[test]
    fn initialize_params_includes_hierarchical_support() {
        #[allow(deprecated)]
        let params = InitializeParams {
            process_id: Some(42),
            root_uri: Some("file:///tmp/project".parse().unwrap()),
            capabilities: ClientCapabilities {
                text_document: Some(TextDocumentClientCapabilities {
                    document_symbol: Some(DocumentSymbolClientCapabilities {
                        hierarchical_document_symbol_support: Some(true),
                        ..Default::default()
                    }),
                    references: Some(Default::default()),
                    ..Default::default()
                }),
                ..Default::default()
            },
            ..Default::default()
        };

        let json = serde_json::to_value(&params).unwrap();

        assert_eq!(json["processId"], 42);
        assert_eq!(json["rootUri"], "file:///tmp/project");
        assert_eq!(
            json["capabilities"]["textDocument"]["documentSymbol"]["hierarchicalDocumentSymbolSupport"],
            true
        );
        assert!(json["capabilities"]["textDocument"]["references"].is_object());
    }

    #[test]
    fn reference_params_serializes_correctly() {
        let uri: Uri = "file:///tmp/test.rs".parse().unwrap();
        let params = ReferenceParams {
            text_document_position: TextDocumentPositionParams::new(
                TextDocumentIdentifier::new(uri),
                Position::new(10, 5),
            ),
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
            context: ReferenceContext {
                include_declaration: false,
            },
        };

        let json = serde_json::to_value(&params).unwrap();

        assert_eq!(json["textDocument"]["uri"], "file:///tmp/test.rs");
        assert_eq!(json["position"]["line"], 10);
        assert_eq!(json["position"]["character"], 5);
        assert_eq!(json["context"]["includeDeclaration"], false);
        assert!(json.get("workDoneToken").is_none());
        assert!(json.get("partialResultToken").is_none());
    }

    #[test]
    fn document_symbol_params_serializes_correctly() {
        let uri: Uri = "file:///tmp/lib.rs".parse().unwrap();
        let params = DocumentSymbolParams {
            text_document: TextDocumentIdentifier::new(uri),
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
        };

        let json = serde_json::to_value(&params).unwrap();

        assert_eq!(json["textDocument"]["uri"], "file:///tmp/lib.rs");
        assert!(json.get("workDoneToken").is_none());
        assert!(json.get("partialResultToken").is_none());
    }

    #[test]
    fn parse_document_symbols_flattens_hierarchy() {
        let response = json!([
            {
                "name": "filter_items",
                "kind": LspSymbolKind::FUNCTION,
                "selectionRange": {
                    "start": { "line": 4, "character": 3 },
                    "end": { "line": 4, "character": 15 }
                },
                "range": {
                    "start": { "line": 3, "character": 0 },
                    "end": { "line": 8, "character": 1 }
                }
            },
            {
                "name": "Item",
                "kind": LspSymbolKind::STRUCT,
                "selectionRange": {
                    "start": { "line": 0, "character": 7 },
                    "end": { "line": 0, "character": 11 }
                },
                "range": {
                    "start": { "line": 0, "character": 0 },
                    "end": { "line": 2, "character": 1 }
                },
                "children": [
                    {
                        "name": "process",
                        "kind": LspSymbolKind::METHOD,
                        "selectionRange": {
                            "start": { "line": 10, "character": 11 },
                            "end": { "line": 10, "character": 18 }
                        },
                        "range": {
                            "start": { "line": 10, "character": 4 },
                            "end": { "line": 12, "character": 5 }
                        }
                    }
                ]
            },
            {
                "name": "Status",
                "kind": LspSymbolKind::ENUM,
                "selectionRange": {
                    "start": { "line": 15, "character": 5 },
                    "end": { "line": 15, "character": 11 }
                },
                "range": {
                    "start": { "line": 15, "character": 0 },
                    "end": { "line": 18, "character": 1 }
                }
            },
            {
                "name": "Processor",
                "kind": LspSymbolKind::INTERFACE,
                "selectionRange": {
                    "start": { "line": 20, "character": 6 },
                    "end": { "line": 20, "character": 15 }
                },
                "range": {
                    "start": { "line": 20, "character": 0 },
                    "end": { "line": 22, "character": 1 }
                }
            },
            {
                "name": "MAGIC",
                "kind": 99,
                "selectionRange": {
                    "start": { "line": 24, "character": 6 },
                    "end": { "line": 24, "character": 11 }
                },
                "range": {
                    "start": { "line": 24, "character": 0 },
                    "end": { "line": 24, "character": 20 }
                }
            }
        ]);

        let symbols = parse_document_symbols(response).unwrap();

        assert_eq!(symbols.len(), 6);

        assert_eq!(symbols[0].name, "filter_items");
        assert_eq!(symbols[0].kind, SymbolKind::Function);
        assert_eq!(symbols[0].selection_start_line, 4);
        assert_eq!(symbols[0].selection_start_char, 3);

        assert_eq!(symbols[1].name, "Item");
        assert_eq!(symbols[1].kind, SymbolKind::Struct);

        assert_eq!(symbols[2].name, "process");
        assert_eq!(symbols[2].kind, SymbolKind::Method);
        assert_eq!(symbols[2].selection_start_line, 10);

        assert_eq!(symbols[3].name, "Status");
        assert_eq!(symbols[3].kind, SymbolKind::Enum);

        assert_eq!(symbols[4].name, "Processor");
        assert_eq!(symbols[4].kind, SymbolKind::Interface);

        assert_eq!(symbols[5].name, "MAGIC");
        assert_eq!(symbols[5].kind, SymbolKind::Other);
    }

    #[test]
    fn parse_document_symbols_handles_null() {
        let symbols = parse_document_symbols(Value::Null).unwrap();
        assert!(symbols.is_empty());
    }

    #[test]
    fn parse_document_symbols_returns_error_for_invalid_json() {
        let result = parse_document_symbols(json!({"not": "an array"}));
        assert!(result.is_err());
    }

    #[test]
    fn path_to_file_uri_returns_valid_uri() {
        let path = Path::new("/home/user/project/src/lib.rs");
        let uri = path_to_file_uri(path).unwrap();
        assert!(uri.as_str().starts_with("file:///"));
        assert!(uri.as_str().contains("lib.rs"));
    }
}
