use std::{fs::read_to_string, path::Path};

use coderag::{
    CandleProvider, ChunkStore, ChunkType, EmbeddingProvider, LanceDbStore, LineChunker, parser::AstChunker,
};
use tempfile::TempDir;

/// Full pipeline test: index the mini-rust fixture, query it, verify results.
/// Uses a temporary directory for the LanceDB database (cleaned up after the test)
///
/// Run with: cargo test -- --test-threads=1
#[tokio::test]
async fn index_and_query_returns_relevant_results() {
    let tmp = TempDir::new().expect("failed to create tmp dir");
    let db_path = tmp.path().to_str().unwrap().to_string();

    let embedder = CandleProvider::new().await.expect("failed to load embedding model");

    let store = LanceDbStore::open(&db_path, embedder.dimension())
        .await
        .expect("failed to open store");

    let chunker = LineChunker::default();

    // Index al fixture files
    let fixture_dir = Path::new("tests/fixtures/mini-rust/src");
    for file_name in &["io.rs", "config.rs", "processor.rs"] {
        let file_path = fixture_dir.join(file_name);
        let content = read_to_string(&file_path).unwrap_or_else(|_| panic!("cannot read fixture {file_name}"));

        let mut chunks = chunker.chunk_file(&file_path, &content);
        let texts = chunks.iter().map(|chunk| chunk.content.as_str()).collect::<Vec<_>>();
        let embeddings = embedder.embed(&texts).await.expect("embed failed");

        for (chunk, emb) in chunks.iter_mut().zip(embeddings) {
            chunk.embedding = Some(emb);
        }
        store.upsert(&chunks).await.expect("upsert failed");
    }

    // query for the file I/O -> should return results from io.rs
    let io_query = embedder
        .embed(&["read file from disk"])
        .await
        .expect("embed query failed");

    let io_results = store.search_vector(&io_query[0], 3).await.expect("search failed");

    assert!(!io_results.is_empty(), "should return at least one result");
    assert!(
        io_results[0].score >= 0.0 && io_results[0].score <= 1.0,
        "score out of valid range: {}",
        io_results[0].score
    );

    assert!(
        io_results[0].chunk.metadata.file_path.to_str().unwrap().contains("io"),
        "top results should come from io.rs, got {}",
        io_results[0].chunk.metadata.file_path.display()
    );

    // Query for configuration — should return results from config.rs.
    let config_query = embedder
        .embed(&["load configuration from environment variables"])
        .await
        .expect("embed query failed");
    let config_results = store.search_vector(&config_query[0], 3).await.expect("search failed");

    assert!(
        config_results[0]
            .chunk
            .metadata
            .file_path
            .to_str()
            .unwrap()
            .contains("config"),
        "top result for config query should come from config.rs, got {}",
        config_results[0].chunk.metadata.file_path.display()
    );
}

#[tokio::test]
async fn rust_chunks_have_correct_types_and_symbols() {
    let tmp = TempDir::new().unwrap();
    let db_path = tmp.path().to_str().unwrap().to_string();

    let embedder = CandleProvider::new().await.unwrap();
    let store = LanceDbStore::open(&db_path, embedder.dimension()).await.unwrap();
    let chunker = AstChunker::new();

    let fixture = Path::new("tests/fixtures/mini-rust/src/processor.rs");
    let content = read_to_string(fixture).unwrap();

    let mut chunks = chunker.chunk_file(fixture, &content);

    // processor.rs has a struct Item and three functions.
    assert!(
        chunks.iter().any(|chunk| {
            chunk.metadata.chunk_type == ChunkType::Struct && chunk.metadata.symbol_name.as_deref() == Some("Item")
        }),
        "Should have a struct chunk for Item"
    );

    assert!(
        chunks.iter().any(|chunk| {
            chunk.metadata.chunk_type == ChunkType::Function
                && chunk.metadata.symbol_name.as_deref() == Some("filter_items")
        }),
        "Should have a Function chunk for filter_items"
    );

    // Embed an store to test the full round trip
    let texts = chunks.iter().map(|chunk| chunk.content.as_str()).collect::<Vec<_>>();
    let embeddings = embedder.embed(&texts).await.unwrap();
    for (chunk, emb) in chunks.iter_mut().zip(embeddings) {
        chunk.embedding = Some(emb);
    }
    store.upsert(&chunks).await.unwrap();

    // Query and verify that symbol_name survives the LanceDB round-trip
    let query_emb = embedder.embed(&["filter a list of items by length"]).await.unwrap();
    let results = store.search_vector(&query_emb[0], 3).await.unwrap();

    assert!(!results.is_empty());
    assert!(
        results[0].chunk.metadata.symbol_name.is_some(),
        "Returned chunk should have a symbol name"
    );
}
#[tokio::test]
async fn query_returns_relevant_results_after_ast_indexing() {
    let tmp = TempDir::new().unwrap();
    let db_path = tmp.path().to_str().unwrap().to_string();

    let embedder = CandleProvider::new().await.unwrap();
    let store = LanceDbStore::open(&db_path, embedder.dimension()).await.unwrap();
    let chunker = AstChunker::new();

    // Index all Rust fixture files.
    for file_name in &["io.rs", "config.rs", "processor.rs"] {
        let file_path = Path::new("tests/fixtures/mini-rust/src").join(file_name);
        let content = std::fs::read_to_string(&file_path).unwrap();
        let mut chunks = chunker.chunk_file(&file_path, &content);
        let texts: Vec<&str> = chunks.iter().map(|c| c.content.as_str()).collect();
        let embeddings = embedder.embed(&texts).await.unwrap();
        for (chunk, emb) in chunks.iter_mut().zip(embeddings) {
            chunk.embedding = Some(emb);
        }
        store.upsert(&chunks).await.unwrap();
    }

    // "read a file" should return results from io.rs.
    let io_emb = embedder.embed(&["read file from disk"]).await.unwrap();
    let io_results = store.search_vector(&io_emb[0], 3).await.unwrap();
    assert!(
        io_results[0].chunk.metadata.file_path.to_str().unwrap().contains("io"),
        "Top result for file I/O query should come from io.rs"
    );
}
