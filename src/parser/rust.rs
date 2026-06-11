use std::path::Path;

use crate::{
    Chunk, ChunkId, ChunkMetadata, ChunkType, Language,
    parser::{LanguagePlugin, field},
};

pub struct RustPlugin;

impl RustPlugin {
    pub fn new() -> Self {
        Self
    }
}
// ```
// source_file                          ← root
//   ├── use_declaration                ← ignored
//   ├── function_item                  ← extracted as Function
//   │     ├── fn (token)              ← is_named() = false ->  ignored
//   │     ├── name: identifier "foo"  ← field "name" → symbol_name
//   │     ├── parameters: (...)
//   │     └── body: block { ... }
//   ├── struct_item                    ← extracted as Struct
//   └── impl_item
//         ├── type: type_identifier "Foo"  ← field "type" → parent_scope
//         └── body: declaration_list
//               ├── function_item "new"    ← extracted as Method
//               └── function_item "run"   ← extracted as Method
// ```
impl LanguagePlugin for RustPlugin {
    fn file_extensions(&self) -> &[&str] {
        &["rs"]
    }

    fn chunk_file(&self, path: &std::path::Path, source: &str) -> Vec<Chunk> {
        let source_bytes = source.as_bytes();

        // Initialize a parser for Rust. LANGUAGE is a LanguageFn constant
        let language: tree_sitter::Language = tree_sitter_rust::LANGUAGE.into();
        let mut parser = tree_sitter::Parser::new();
        if parser.set_language(&language).is_err() {
            tracing::warn!(
                file_path = %&path.display(),
                "failed to initialize tree-sitter Rust parser"
            );
            return Vec::new();
        }

        let tree = match parser.parse(source, None) {
            Some(t) => t,
            None => {
                tracing::warn!(
                    file_path = %&&path.display(),
                    "tree-sitter parse returned None"
                );
                return Vec::new();
            },
        };

        let root = tree.root_node();

        let mut chunks = Vec::new();
        let mut cursor = root.walk();

        for child in root.children(&mut cursor) {
            // Skip unnamed nodes
            if !child.is_named() {
                continue;
            }
            // Skip nodes that tree-sitter could not parse
            if child.is_error() {
                continue;
            }

            match child.kind() {
                "function_item" => {
                    if let Some(chunk) = extract_named_node(&child, source_bytes, path, None, ChunkType::Function) {
                        chunks.push(chunk);
                    }
                },
                "impl_item" => {
                    chunks.extend(extract_impl(&child, source_bytes, path));
                },
                "struct_item" | "enum_item" => {
                    if let Some(chunk) = extract_named_node(&child, source_bytes, path, None, ChunkType::Struct) {
                        chunks.push(chunk);
                    }
                },
                "trait_item" => {
                    if let Some(chunk) = extract_named_node(&child, source_bytes, path, None, ChunkType::Trait) {
                        chunks.push(chunk);
                    }
                },
                // use_declaration, mod_item, const_item, type_item, attribute_item, etc.
                // are not chunked individually.
                _ => {},
            }
        }

        chunks
    }
}

/// Extract a chunk from a named AST node.
/// `name_field` is the field name in the grammar that holds the symbol name
/// (e.g., "name" for function_item, struct_item, trait_item).
fn extract_named_node(
    node: &tree_sitter::Node<'_>,
    source_bytes: &[u8],
    path: &Path,
    parent_scope: Option<String>,
    chunk_type: ChunkType,
) -> Option<Chunk> {
    let text = node.utf8_text(source_bytes).ok()?.trim().to_string();

    if text.is_empty() {
        return None;
    }

    let line_start = node.start_position().row as u32;
    let line_end = node.end_position().row as u32;

    // the "name" field is defined in the tree-sitter-rust grammar for
    // function_item, struct_item, enum_item, trait_item
    let symbol_name = node
        .child_by_field_name(field::NAME)
        .and_then(|node| node.utf8_text(source_bytes).ok())
        .map(|s| s.to_string());

    Some(Chunk {
        id: ChunkId::compute(path, line_start, &text),
        content: text,
        metadata: ChunkMetadata {
            file_path: path.to_path_buf(),
            line_start,
            line_end,
            language: Language::Rust,
            chunk_type,
            symbol_name,
            parent_scope,
        },
        embedding: None,
    })
}

/// Process an impl block: extract each method as a separate chunk.
/// If the impl has no methods, extract the whole block as a single Impl chunk.
///
/// La lógica tiene dos caminos:
///
/// Camino A — impl con métodos (el caso habitual):
///   `impl Counter { fn new() {...} fn increment() {...} }`
///   → Produce 2 chunks de tipo Method con parent_scope = "Counter"
///   → Cada método tiene su propio embedding → búsqueda precisa
///
/// Camino B — impl sin métodos (trait markers, type aliases, empty):
///   `impl Send for Foo {}`  o  `impl Default for Foo { type Item = ...; }`
///   → Un solo chunk del impl completo con tipo Impl
///   → Evita producir cero chunks (lo que activaría el fallback a LineChunker)
fn extract_impl(impl_node: &tree_sitter::Node<'_>, source_bytes: &[u8], path: &Path) -> Vec<Chunk> {
    let mut chunks = Vec::new();

    // "type" is the concrete type implemented
    // e.g: `impl Foo` -> type = "Foo"
    let type_name = impl_node
        .child_by_field_name(field::TYPE)
        .and_then(|node| node.utf8_text(source_bytes).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "<unknown>".to_string());

    // "body" is the "declaration_list" ( the block `{ ... }` )
    let body = match impl_node.child_by_field_name(field::BODY) {
        Some(b) => b,
        None => return chunks,
    };

    let mut method_cursor = body.walk();
    let mut method_count = 0usize;

    for child in body.children(&mut method_cursor) {
        if !child.is_named() || child.is_error() {
            continue;
        }
        if child.kind() == "function_item" {
            if let Some(chunk) =
                extract_named_node(&child, source_bytes, path, Some(type_name.clone()), ChunkType::Method)
            {
                chunks.push(chunk);
                method_count += 1;
            }
        }
    }

    if method_count == 0 {
        if let Some(chunk) = extract_named_node(impl_node, source_bytes, path, None, ChunkType::Impl) {
            chunks.push(chunk);
        }
    }

    chunks
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use crate::{
        ChunkType,
        parser::{LanguagePlugin, rust::RustPlugin},
    };

    fn path(name: &str) -> PathBuf {
        PathBuf::from(name)
    }

    #[test]
    fn extract_top_level_functions() {
        let plugin = RustPlugin::new();
        let source = r#"
fn add(a: i32, b: i32) -> i32 {
    a + b
}

fn subtract(a: i32, b: i32) -> i32 {
    a - b
}
"#;

        let chunks = plugin.chunk_file(&path("math.rs"), source);

        assert_eq!(chunks.len(), 2);
        assert!(
            chunks
                .iter()
                .all(|chunk| chunk.metadata.chunk_type == ChunkType::Function)
        );
        assert!(
            chunks
                .iter()
                .any(|chunk| chunk.metadata.symbol_name.as_deref() == Some("add"))
        );
        assert!(
            chunks
                .iter()
                .any(|chunk| chunk.metadata.symbol_name.as_deref() == Some("subtract"))
        );
    }

    #[test]
    fn extracts_methods_with_parent_scope() {
        let plugin = RustPlugin::new();
        let source = r#"
struct Counter {
    value: i32,
}

impl Counter {
    fn new() -> Self {
        Counter { value: 0 }
    }

    fn increment(&mut self) {
        self.value += 1;
    }
}
"#;
        let chunks = plugin.chunk_file(&path("counter.rs"), source);

        let struct_chunks = chunks
            .iter()
            .filter(|chunk| chunk.metadata.chunk_type == ChunkType::Struct)
            .collect::<Vec<_>>();
        assert_eq!(struct_chunks.len(), 1);
        assert_eq!(struct_chunks[0].metadata.symbol_name.as_deref(), Some("Counter"));

        let method_chunks = chunks
            .iter()
            .filter(|chunk| chunk.metadata.chunk_type == ChunkType::Method)
            .collect::<Vec<_>>();

        assert_eq!(method_chunks.len(), 2);
        assert!(
            method_chunks
                .iter()
                .all(|chunk| chunk.metadata.parent_scope.as_deref() == Some("Counter"))
        );
    }

    #[test]
    fn handles_empty_impl_as_single_chunk() {
        let plugin = RustPlugin::new();
        let source = r#"
impl std::fmt::Display for MyType {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "MyType")
    }
}
"#;
        let chunks = plugin.chunk_file(&path("display.rs"), source);
        // The single `fmt` method should be extracted as a Method chunk.
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].metadata.chunk_type, ChunkType::Method);
        assert_eq!(chunks[0].metadata.symbol_name.as_deref(), Some("fmt"));
    }

    #[test]
    fn tolerates_syntax_errors() {
        let plugin = RustPlugin::new();
        // Deliberately broken Rust — tree-sitter should still produce some output
        // or return an empty Vec (triggering LineChunker fallback).
        let broken_source = "fn broken( { let x = ; }";
        // We only assert it doesn't panic.
        let _ = plugin.chunk_file(&path("broken.rs"), broken_source);
    }
}
