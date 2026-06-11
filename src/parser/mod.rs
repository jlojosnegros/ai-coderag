mod cpp;
mod rust;

use std::{path::Path, sync::Arc};

use crate::{
    Chunk, LineChunker,
    parser::{cpp::CppPlugin, rust::RustPlugin},
};

/// Grammar field name constants for tree-sitter child_by_field_name() calls.
pub(super) mod field {
    pub const NAME: &str = "name";
    pub const BODY: &str = "body";
    pub const TYPE: &str = "type";
    pub const DECLARATOR: &str = "declarator";
}

/// Trait implemented by each language-specific parser.
/// A plugin receives raw source text and return semantic chunks.
pub trait LanguagePlugin: Send + Sync {
    /// File extensions handled by this plugin(without the leading dot)
    fn file_extensions(&self) -> &[&str];

    /// Parse source into chunks. Return an empty Vec if parsing fails
    fn chunk_file(&self, path: &Path, source: &str) -> Vec<Chunk>;
}

/// Routes each file to the appropiate LanguagePlugin based on extension.
/// fall back to LineChunker for unknown or unparseable files.
pub struct AstChunker {
    plugins: Vec<Arc<dyn LanguagePlugin>>,
    fallback: LineChunker,
}

impl AstChunker {
    /// Create an AstChunker with the built-in Rust and C++ plugins registered
    pub fn new() -> Self {
        let mut chunker = Self {
            plugins: Vec::new(),
            fallback: LineChunker::default(),
        };
        chunker.register(RustPlugin::new());
        chunker.register(CppPlugin::new());
        chunker
    }

    pub fn register<P: LanguagePlugin + 'static>(&mut self, plugin: P) {
        self.plugins.push(Arc::new(plugin));
    }

    /// Chunk a source file using the appropiate plugin.
    /// Falls back to LineChunker if no plugin handles the extension or
    /// if the plugin returns and empty result.
    pub fn chunk_file(&self, path: &Path, source: &str) -> Vec<Chunk> {
        let ext = path.extension().and_then(|ext| ext.to_str()).unwrap_or("");

        for plugin in &self.plugins {
            if plugin.file_extensions().contains(&ext) {
                let chunks = plugin.chunk_file(path, source);
                if !chunks.is_empty() {
                    return chunks;
                }
                // plugin produced nothing
                // fall through to the fallback chunker
                tracing::debug!(
                    file_path = %&path.display(),
                    "pluging returned no chunks, falling to LineChunker",
                );
                break;
            }
        }
        self.fallback.chunk_file(path, source)
    }
}
impl Default for AstChunker {
    fn default() -> Self {
        Self::new()
    }
}
