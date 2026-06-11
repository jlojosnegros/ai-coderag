use std::{convert::Infallible, path::PathBuf, str::FromStr};

use sha2::{Digest, Sha256};

/// Content-addressed identifier of a chunk.
/// SHA-256 of (canonical file path + line_start + content) encoded as hex.
/// If the content at a given location does not change, the ID dos not change.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ChunkId(pub String);

impl ChunkId {
    pub fn compute(file: &std::path::Path, line_start: u32, content: &str) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(file.to_string_lossy().as_bytes());
        hasher.update(line_start.to_le_bytes());
        hasher.update(content.as_bytes());
        Self(hex::encode(hasher.finalize()))
    }
}

#[derive(Clone, Debug)]
pub enum Language {
    Rust,
    Cpp,
    Unknown,
}

impl Language {
    pub fn from_extension(ext: &str) -> Self {
        match ext {
            "rs" => Self::Rust,
            "cc" | "cpp" | "cxx" | "c" | "h" | "hpp" => Self::Cpp,
            _ => Self::Unknown,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Rust => "Rust",
            Self::Cpp => "Cpp",
            Self::Unknown => "Unknown",
        }
    }
}
impl std::str::FromStr for Language {
    type Err = std::convert::Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s.to_lowercase().as_str() {
            "rust" => Self::Rust,
            "cpp" => Self::Cpp,
            _ => Self::Unknown,
        })
    }
}

/// Semantic classification of a chunk, determined by the parser
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChunkType {
    /// A free function at module scope
    Function,
    /// A method inside an `impl` block
    Method,
    /// A struct or enum definition
    Struct,
    /// A trait definition
    Trait,
    /// An impl block with no extractable methods
    Impl,
    /// Produced by LineChunker when AST parsing was not possible.
    FallbackLines,
}

impl ChunkType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Function => "function",
            Self::Method => "method",
            Self::Struct => "struct",
            Self::Trait => "trait",
            Self::Impl => "impl",
            Self::FallbackLines => "fallback_lines",
        }
    }
}

impl FromStr for ChunkType {
    type Err = Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "function" => Self::Function,
            "method" => Self::Method,
            "struct" => Self::Struct,
            "trait" => Self::Trait,
            "impl" => Self::Impl,
            _ => Self::FallbackLines,
        })
    }
}

#[derive(Clone, Debug)]
pub struct ChunkMetadata {
    pub file_path: PathBuf,
    pub line_start: u32,
    pub line_end: u32,
    pub language: Language,
    /// How this chunk was produced
    pub chunk_type: ChunkType,
    /// Name of the symbol (function name, struct name, etc). None for fallback
    pub symbol_name: Option<String>,
    /// Enclosing scope for methods: type name of the impl block
    pub parent_scope: Option<String>,
}

/// Fundamental unit of storage and retrieval.
/// After indexing, `embedding` is always Some. During chunking, it is None
/// until the embed step runs
#[derive(Clone, Debug)]
pub struct Chunk {
    pub id: ChunkId,
    pub content: String,
    pub metadata: ChunkMetadata,
    pub embedding: Option<Vec<f32>>,
}

/// A chunk returned from a search query, with its relevance score.
pub struct ScoredChunk {
    pub chunk: Chunk,
    /// Cosine similarity in range [0.0, 1.0] Higher means more relevant
    pub score: f32,
}
