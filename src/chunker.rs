use std::path::Path;

use crate::{Chunk, ChunkId, ChunkMetadata, Language};

pub struct LineChunker {
    /// Number of lines per chunk,
    pub chunk_size: usize,

    /// Lines of overlap between consecutive chunks.
    /// must be 0 < overlap < chunk_size
    pub overlap: usize,
}

impl Default for LineChunker {
    fn default() -> Self {
        Self {
            chunk_size: 40,
            overlap: 8,
        }
    }
}

impl LineChunker {
    /// Split file content into overlapping line-based chunks.
    /// Files with fewer than 5 non-empty lines produce at most one chunk
    pub fn chunk_file(&self, path: &Path, content: &str) -> Vec<Chunk> {
        assert!(self.overlap < self.chunk_size, "overlap must be less than chunk_size");

        let language = path
            .extension()
            .and_then(|ext| ext.to_str())
            .map(Language::from_extension)
            .unwrap_or(Language::Unknown);

        let lines = content.lines().collect::<Vec<_>>();
        let mut chunks = Vec::new();

        // Number of lines to advance between chunk start positions
        let step = self.chunk_size - self.overlap;

        let mut start = 0usize;
        while start < lines.len() {
            let end = (start + self.chunk_size).min(lines.len());
            let chunk_content = lines[start..end].join("\n");

            if chunk_content.trim().len() >= 10 {
                let id = ChunkId::compute(path, start as u32, &chunk_content);

                chunks.push(Chunk {
                    id,
                    content: chunk_content,
                    metadata: ChunkMetadata {
                        file_path: path.to_path_buf(),
                        line_start: start as u32,
                        line_end: end as u32,
                        language: language.clone(),
                    },
                    embedding: None,
                });
            }
            start += step;
        }

        chunks
    }
}

#[cfg(test)]
mod tests {


    use std::path::Path;

    use super::*;


    fn make_content(n_lines: usize) -> String {
        (1..=n_lines)
            .map(|idx| format!("let x_{idx} = {idx};"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn single_chunk_for_small_file() {
        let chunker = LineChunker {
            chunk_size: 40,
            overlap: 8,
        };

        let content = make_content(10);
        let chunks = chunker.chunk_file(Path::new("test.rs"), &content);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].metadata.line_start, 0);
        assert_eq!(chunks[0].metadata.line_end, 10);
    }

    #[test]
    fn multiple_chunks_with_overlap() {
        let chunker = LineChunker {
            chunk_size: 5,
            overlap: 1,
        };

        let content = make_content(10);
        let chunks = chunker.chunk_file(Path::new("test.rs"), &content);

        assert!(chunks.len() >= 2);

        // verify overlap: last line of chunk 0 == first line of chunk 1
        let last_line_chunk0 = chunks[0].content.lines().last().unwrap();
        let first_line_chunk1 = chunks[1].content.lines().next().unwrap();
        assert_eq!(last_line_chunk0, first_line_chunk1);
    }

    #[test]
    fn chunk_ids_are_deterministic() {
        let chunker = LineChunker::default();
        let content = "fn main() {\n    println!(\"hello\");\n}";
        let path = Path::new("foo.rs");
        let chunks1 = chunker.chunk_file(path, content);
        let chunks2 = chunker.chunk_file(path, content);
        assert_eq!(chunks1[0].id, chunks2[0].id);
    }

    #[test]
    fn empty_files_produce_no_chunks() {
        let chunker = LineChunker::default();
        let chunks = chunker.chunk_file(Path::new("empty.rs"), "");
        assert!(chunks.is_empty());
    }


    #[test]
    fn different_paths_produce_different_ids() {
        let chunker = LineChunker {
            chunk_size: 5,
            overlap: 0,
        };

        let content = make_content(5);
        let chunks_a = chunker.chunk_file(Path::new("a.rs"), &content);
        let chunks_b = chunker.chunk_file(Path::new("b.rs"), &content);
        assert_ne!(chunks_a[0].id, chunks_b[0].id);
    }
}
