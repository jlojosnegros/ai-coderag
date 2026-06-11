use std::path::Path;

use crate::{
    Chunk, ChunkId, ChunkMetadata, ChunkType, Language,
    parser::{LanguagePlugin, field},
};

pub struct CppPlugin;

impl CppPlugin {
    pub fn new() -> Self {
        Self
    }
}

impl LanguagePlugin for CppPlugin {
    fn file_extensions(&self) -> &[&str] {
        &["cc", "cpp", "cxx", "c", "h", "hpp"]
    }

    fn chunk_file(&self, path: &std::path::Path, source: &str) -> Vec<crate::Chunk> {
        let source_bytes = source.as_bytes();

        let language: tree_sitter::Language = tree_sitter_cpp::LANGUAGE.into();
        let mut parser = tree_sitter::Parser::new();

        if parser.set_language(&language).is_err() {
            tracing::warn!(
                file_path = %&path.display(),
                "failed to initialize tree-sitter C++ parser"
            );
            return Vec::new();
        }

        let tree = match parser.parse(source, None) {
            Some(t) => t,
            None => return Vec::new(),
        };

        let root = tree.root_node();
        let mut chunks = Vec::new();
        let mut cursor = root.walk();

        for child in root.children(&mut cursor) {
            if !child.is_named() || child.is_error() {
                continue;
            }

            match child.kind() {
                "function_definition" => {
                    if let Some(chunk) = extract_cpp_function(&child, source_bytes, path) {
                        chunks.push(chunk);
                    }
                },
                "class_specifier" | "struct_specifier" => {
                    if let Some(chunk) = extract_cpp_class(&child, source_bytes, path) {
                        chunks.push(chunk);
                    }
                },
                // namespace_definition, template_declaration, preprocessor directives, etc.
                // are not chunked individually ( yet)
                _ => {},
            }
        }

        chunks
    }
}

fn extract_cpp_function(node: &tree_sitter::Node<'_>, source_bytes: &[u8], path: &Path) -> Option<Chunk> {
    let text = node.utf8_text(source_bytes).ok()?.trim().to_string();
    if text.is_empty() {
        return None;
    }

    let line_start = node.start_position().row as u32;
    let line_end = node.end_position().row as u32;

    // Navigate to the innermost declarator to find the function name.
    // The declarator  field can be nested
    // ( e.g pointer_declarator wrapping a function_declarator)
    let symbol_name = extract_cpp_function_name(node, source_bytes);

    Some(Chunk {
        id: ChunkId::compute(path, line_start, &text),
        content: text,
        metadata: ChunkMetadata {
            file_path: path.to_path_buf(),
            line_start,
            line_end,
            language: Language::Cpp,
            chunk_type: ChunkType::Function,
            symbol_name,
            parent_scope: None,
        },
        embedding: None,
    })
}

/// Walk the declarator chain to find the identifier that names the function.
///
/// C++ has many declarator forms:
///   - `int foo(...)` → function_declarator { declarator: identifier }
///   - `int *foo(...)` → pointer_declarator { declarator: function_declarator { ... } }
///   - `Foo::bar(...)` → function_declarator { declarator: qualified_identifier }
///
/// We walk until we find an identifier or give up.
fn extract_cpp_function_name(node: &tree_sitter::Node<'_>, source_bytes: &[u8]) -> Option<String> {
    let declarator = node.child_by_field_name(field::DECLARATOR)?;

    // Walk down the declarator chain until we reach the terminal node that holds the name.
    // C++ allows arbitrarily nested declarator forms, e.g.:
    //   `int *(*foo)(int)` → pointer_declarator → pointer_declarator → function_declarator → "foo"
    let mut current = declarator;
    loop {
        match current.kind() {
            // Wrapper nodes: they all carry the real name inside their own `declarator` field.
            // The three forms look different in C++ source but share the same CST shape:
            //   function_declarator  → `foo(int, int)`      — signature wraps the name
            //   pointer_declarator   → `*foo` or `*foo(..)` — `*` wraps what follows
            //   reference_declarator → `&foo`               — `&` wraps what follows
            // In all three cases we descend one level and keep looking.
            "function_declarator" | "pointer_declarator" | "reference_declarator" => {
                current = current.child_by_field_name(field::DECLARATOR)?;
            },
            // Terminal nodes: the text of the node IS the function name. We stop here.
            //   identifier         → plain name: `foo`
            //   field_identifier   → member name in a struct context: `bar`
            //   qualified_identifier → scoped name: `Foo::bar`
            //   destructor_name    → destructor:   `~Foo`
            "identifier" | "field_identifier" | "qualified_identifier" | "destructor_name" => {
                return current.utf8_text(source_bytes).ok().map(|s| s.to_string());
            },
            _ => return None,
        }
    }
}

/// Extract a C++ class or struct as a single chunk.
fn extract_cpp_class(node: &tree_sitter::Node<'_>, source_bytes: &[u8], path: &Path) -> Option<Chunk> {
    let text = node.utf8_text(source_bytes).ok()?.trim().to_string();
    if text.is_empty() {
        return None;
    }

    let line_start = node.start_position().row as u32;
    let line_end = node.end_position().row as u32;

    // the class/struct name is in the "name" field
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
            language: Language::Cpp,
            chunk_type: ChunkType::Struct,
            symbol_name,
            parent_scope: None,
        },
        embedding: None,
    })
}
