use lsp_types::{ClientCapabilities, DocumentSymbolClientCapabilities, TextDocumentClientCapabilities};

use crate::traits::LanguageLsp;

/// LSP provider for Rust, backed by rust-analyzer
pub struct RustLsp;

impl LanguageLsp for RustLsp {
    fn language_id(&self) -> &str {
        "rust"
    }

    fn file_extensions(&self) -> &[&str] {
        &["rs"]
    }

    fn default_command(&self) -> &str {
        "rust-analyzer"
    }

    fn default_args(&self) -> Vec<String> {
        Vec::new()
    }

    fn find_project_root(&self, start: &std::path::Path) -> Option<std::path::PathBuf> {
        // Walk up from `start` looking for Cargo.toml
        // if `start` is a file, begin from its parent directory
        let mut current = if start.is_file() {
            start.parent()?.to_path_buf()
        } else {
            start.to_path_buf()
        };

        loop {
            if current.join("Cargo.toml").exists() {
                return Some(current);
            }

            match current.parent() {
                Some(p) => current = p.to_path_buf(),
                None => return None,
            }
        }
    }

    /// Declares which LSP features this clients supports.
    ///
    /// The server reads these capabilities during the initialize handshake
    /// and enables or disables endpoints accordingly. If a capability is
    /// not declared here, the server may reject the corresponding request
    /// or return empty results silently
    fn initialize_capabilities(&self) -> lsp_types::ClientCapabilities {
        ClientCapabilities {
            text_document: Some(TextDocumentClientCapabilities {
                // Enables textDocument/documentSymbol with nested children.
                // Without this the server returns flat SymbolInformation[]
                // instead of hierarchical DocumentSymbol[] with selectionRange.
                document_symbol: Some(DocumentSymbolClientCapabilities {
                    hierarchical_document_symbol_support: Some(true),
                    ..Default::default()
                }),
                // Enables textDocument/references ( find all call sites of a symbol)
                references: Some(Default::default()),
                // Enables textDocument/definition (jump to where a symbol is defined)
                definition: Some(Default::default()),
                // Enables three-step call hierarchy protocol:
                //    textDocument/prepareCallHierarchy -> callHierarchy/incomingCalls
                //                                      -> callHierarchy/outgoingCalls
                call_hierarchy: Some(Default::default()),
                // Tells the server to push textDocument/publishDiagnostics
                // notifications (errors , warnings) as it analyzes files.
                publish_diagnostics: Some(Default::default()),
                ..Default::default()
            }),
            ..Default::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    #[test]
    fn find_project_rool_from_project_dir() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let result = RustLsp.find_project_root(root);

        assert_eq!(result, Some(root.to_path_buf()));
    }

    #[test]
    fn find_project_rool_from_src_subdir() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let result = RustLsp.find_project_root(&src);

        assert_eq!(result, Some(root.to_path_buf()));
    }

    #[test]
    fn find_project_rool_from_src_file() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let file = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/lib.rs");
        let result = RustLsp.find_project_root(&file);

        assert_eq!(result, Some(root.to_path_buf()));
    }
    #[test]
    fn find_project_root_returns_none_without_cargo_toml() {
        let result = RustLsp.find_project_root(Path::new("/tmp"));
        assert!(result.is_none());
    }

    #[test]
    fn capabilities_include_hierarchical_symbol_support() {
        let caps = RustLsp.initialize_capabilities();
        let doc_symbol = caps.text_document.as_ref().unwrap().document_symbol.as_ref().unwrap();
        assert_eq!(doc_symbol.hierarchical_document_symbol_support, Some(true));
    }

    #[test]
    fn capabilities_include_references() {
        let caps = RustLsp.initialize_capabilities();
        assert!(
            caps.text_document.as_ref().unwrap().references.is_some(),
            "references capability must be declared"
        );
    }
}
