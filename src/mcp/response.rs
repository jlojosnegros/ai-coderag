//! Types for the response metadata that accompanies every data-returning
//! MCP toml response

use serde_json::json;

/// Which mechanism produced the reponse data
///
/// Represents the three levels of the degradation hierarchy:
///  - LSP: data come from a live language server (highest findelity)
///  - TreeSitter: data came from AST parsing without a language server
///  - Embeddings: data came from vector similarity search (lowest fidelity)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceLevel {
    Lsp,
    TreeSitter,
    Embeddings,
}

impl std::fmt::Display for SourceLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Lsp => f.write_str("lsp"),
            Self::TreeSitter => f.write_str("tree-sitter"),
            Self::Embeddings => f.write_str("embeddings"),
        }
    }
}

/// Whether the returned data is up to date
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Freshness {
    Current,
    Stale,
}

impl std::fmt::Display for Freshness {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Current => f.write_str("current"),
            Self::Stale => f.write_str("stale"),
        }
    }
}

/// Metadata attached to every data-returning MCP tool response
///
/// This will be emitter in two places at the same time.
/// 1. The `_meta` field of CallToolResult for MCP clients to parse. Today most
/// clients drops this part silently but the MCP spec is trending towards making
/// it mandatory.
///
/// 2. The first line of the text content (human readable ), LLMs that still drop
///  _meta will read it from here
/// format: [source: lsp | freshness: current | detail: ... ]
pub struct ResponseMeta {
    pub source: SourceLevel,
    pub freshness: Freshness,
    pub detail: Option<String>,
}

impl ResponseMeta {
    /// Create metadata with the required fields
    pub fn new(source: SourceLevel, freshness: Freshness) -> Self {
        Self {
            source,
            freshness,
            detail: None,
        }
    }

    /// Add an optional detail explaining why the response is degraded.
    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    /// Render the `_meta` JSON object for CallToolResult
    ///
    /// rmcp's MetaObject is essentially a HasMap<String, serde_json::Value>.
    /// Keys are unprefixed alphanumeric strings (the MCP spec reservers
    /// prefixed keys like 'modelcontextprotocol.io/foo' for protocol use)
    ///
    /// Example output: {"source":"lsp", "freshness":"current"}
    /// with detail : {"source":"tree-sitter", "freshness": "stale", "detail": "lsp initializing"}
    pub fn to_meta_object(&self) -> rmcp::model::MetaObject {
        let mut meta = rmcp::model::MetaObject::new();
        meta.insert("source".into(), json!(self.source.to_string()));
        meta.insert("freshness".into(), json!(self.freshness.to_string()));

        if let Some(detail) = &self.detail {
            meta.insert("detail".into(), json!(detail));
        }

        meta
    }

    pub fn to_header_line(&self) -> String {
        match &self.detail {
            Some(detail) => format!(
                "[source: {} | freshness: {} | detail: {}]",
                self.source, self.freshness, detail
            ),
            None => format!("[source: {} | freshness: {}]", self.source, self.freshness),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::mcp::response::{Freshness, ResponseMeta, SourceLevel};

    #[test]
    fn source_level_display_all_variants() {
        assert_eq!(SourceLevel::Lsp.to_string(), "lsp");
        assert_eq!(SourceLevel::TreeSitter.to_string(), "tree-sitter");
        assert_eq!(SourceLevel::Embeddings.to_string(), "embeddings");
    }

    #[test]
    fn freshness_display_all_variants() {
        assert_eq!(Freshness::Current.to_string(), "current");
        assert_eq!(Freshness::Stale.to_string(), "stale");
    }

    #[test]
    fn header_line_current_lsp_no_detail() {
        let meta = ResponseMeta::new(SourceLevel::Lsp, Freshness::Current);
        assert_eq!(meta.to_header_line(), "[source: lsp | freshness: current]");
    }

    #[test]
    fn header_line_stale_tree_sitter_with_detail() {
        let meta = ResponseMeta::new(SourceLevel::TreeSitter, Freshness::Stale).with_detail("lsp initializing");
        assert_eq!(
            meta.to_header_line(),
            "[source: tree-sitter | freshness: stale | detail: lsp initializing]"
        );
    }
    #[test]
    fn header_line_stale_embeddings_with_file_count() {
        let meta = ResponseMeta::new(SourceLevel::Embeddings, Freshness::Stale)
            .with_detail("3 files modified since last indexing");
        assert_eq!(
            meta.to_header_line(),
            "[source: embeddings | freshness: stale | detail: 3 files modified since last indexing]"
        );
    }

    #[test]
    fn meta_object_without_detail_has_two_keys() {
        let meta = ResponseMeta::new(SourceLevel::Embeddings, Freshness::Current);
        let obj = meta.to_meta_object();

        // Only "source" and "freshness", no "detail".
        assert_eq!(obj.get("source").unwrap(), "embeddings");
        assert_eq!(obj.get("freshness").unwrap(), "current");
        assert!(!obj.contains_key("detail"));
    }

    #[test]
    fn meta_object_with_detail_has_three_keys() {
        let meta = ResponseMeta::new(SourceLevel::Lsp, Freshness::Stale).with_detail("3 files modified");
        let obj = meta.to_meta_object();

        assert_eq!(obj.get("source").unwrap(), "lsp");
        assert_eq!(obj.get("freshness").unwrap(), "stale");
        assert_eq!(obj.get("detail").unwrap(), "3 files modified");
    }
}
