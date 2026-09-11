mod cpp;
mod rust;

use std::{path::Path, sync::Arc};

pub use cpp::CppPlugin;
pub use rust::RustPlugin;

use crate::{Chunk, LineChunker, registry::LanguageRegistry};

/// Grammar field name constants for tree-sitter child_by_field_name() calls.
pub(super) mod field {
    pub const NAME: &str = "name";
    pub const BODY: &str = "body";
    pub const TYPE: &str = "type";
    pub const DECLARATOR: &str = "declarator";
}

/// Trait implemented by each language-specific parser.
/// A plugin receives raw source text and return semantic chunks.
pub trait LanguageParser: Send + Sync {
    /// File extensions handled by this plugin(without the leading dot)
    fn file_extensions(&self) -> &[&str];

    /// Parse source into chunks. Return an empty Vec if parsing fails
    fn chunk_file(&self, path: &Path, source: &str) -> Vec<Chunk>;
}

/// Routes each file to the appropiate LanguageParser based on extension.
/// fall back to LineChunker for unknown or unparseable files.
pub struct AstChunker {
    registry: Arc<LanguageRegistry>,
    fallback: LineChunker,
}

impl AstChunker {
    /// Create an AstChunker backed by the given registry
    pub fn new(registry: Arc<LanguageRegistry>) -> Self {
        Self {
            registry,
            fallback: LineChunker::default(),
        }
    }

    /// Chunk a source file using the appropiate plugin.
    /// Falls back to LineChunker if no plugin handles the extension or
    /// if the plugin returns and empty result.
    pub fn chunk_file(&self, path: &Path, source: &str) -> Vec<Chunk> {
        let ext = path.extension().and_then(|ext| ext.to_str()).unwrap_or("");

        if let Some(parser) = self.registry.parser_for_extension(ext) {
            let chunks = parser.chunk_file(path, source);
            if !chunks.is_empty() {
                return chunks;
            }
        }
        tracing::debug!(
            file_path = %&path.display(),
            "parser returned no chunks, failing to LineChunker",
        );
        self.fallback.chunk_file(path, source)
    }
}
impl Default for AstChunker {
    fn default() -> Self {
        Self::new(Arc::new(LanguageRegistry::with_builtins()))
    }
}
