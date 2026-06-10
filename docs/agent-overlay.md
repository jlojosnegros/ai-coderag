---
agentdoc:
  scan: "f7b1beba1fff08bfa2bf81d8e42b0ca01e9181a1"
  freshness: 100
  human_input: 0
  completeness: 85
  inferred_sections:
    - id: architecture-summary
      heading: "## Architecture in One Paragraph"
    - id: module-map
      heading: "## Module Map"
    - id: key-patterns
      heading: "## Key Patterns & Conventions"
    - id: how-to-extend
      heading: "## How to Extend"
    - id: type-conventions
      heading: "## Type Conventions"
    - id: error-handling
      heading: "## Error Handling Pattern"
    - id: testing-pattern
      heading: "## Testing Pattern"
  watch_paths:
    - "src/main.rs"
    - "src/lib.rs"
    - "src/traits.rs"
    - "src/domain.rs"
    - "src/error.rs"
    - "src/chunker.rs"
    - "src/embed/mod.rs"
    - "src/store/mod.rs"
    - "Cargo.toml"
  stale_sections: []
---

# Agent Overlay — coderag

## Architecture in One Paragraph

coderag is a CLI tool for semantic code search using local embeddings. The `index` subcommand walks source files (`.rs`, `.cc`, `.cpp`, `.cxx`, `.c`, `.h`, `.hpp`) via one of three traversal modes, reads each file, splits it into overlapping 40-line chunks with 8-line overlap using `LineChunker` (`src/chunker.rs`), batches those chunks through `FastembedProvider` (`src/embed/mod.rs`) which runs the `JinaEmbeddingsV2BaseCode` ONNX model inside `tokio::task::spawn_blocking` to avoid blocking the async runtime, normalizes each output vector to unit norm, and upserts the embedded chunks into `LanceDbStore` (`src/store/mod.rs`) using LanceDB's `merge_insert` keyed on `ChunkId` for idempotent re-indexing. The `query` subcommand embeds the query text through the same `FastembedProvider`, runs an ANN vector search via `LanceDbStore::search_vector`, converts L2 distances to cosine similarity scores (`1.0 - l2 / 2.0`, valid only for unit-norm vectors), and prints ranked `ScoredChunk` results. Domain types (`Chunk`, `ChunkId`, `ChunkMetadata`, `Language`, `ScoredChunk`) are in `src/domain.rs`; the port interfaces are `EmbeddingProvider` and `ChunkStore` in `src/traits.rs`; `CoderagError` and `Result<T>` are in `src/error.rs`; `src/lib.rs` re-exports everything so both the binary and integration tests import from `coderag::`.

## Module Map

| Path | Role | Notes |
| ---- | ---- | ----- |
| `src/main.rs` | CLI entry point; `index` and `query` orchestration; `collect_files` | All file-traversal logic lives here, not in a separate module |
| `src/lib.rs` | Library root; re-exports all public types, traits, and impls | Binary imports from `coderag::` — keeps CLI and library in one crate |
| `src/traits.rs` | Port layer: `EmbeddingProvider` and `ChunkStore` async traits | Both are `Send + Sync`; `async_trait` macro required for object safety |
| `src/domain.rs` | Core domain: `Chunk`, `ChunkId`, `ChunkMetadata`, `Language`, `ScoredChunk` | `ChunkId` is content-addressed (SHA-256 of path + line_start + content) |
| `src/error.rs` | `CoderagError` enum (`thiserror`) + `Result<T>` alias | Only three variants; `Io` uses `#[from]` for automatic `?` conversion |
| `src/chunker.rs` | `LineChunker`: sliding-window chunker (default: 40 lines, 8-line overlap) | Chunks with fewer than 10 non-whitespace chars are dropped |
| `src/embed/mod.rs` | `FastembedProvider`: ONNX model wrapper; normalizes embeddings to unit norm | Model load and batch embed both use `spawn_blocking`; dimension derived by embedding a probe string at init |
| `src/store/mod.rs` | `LanceDbStore`: Arrow RecordBatch schema, `merge_insert` upsert, ANN search | L2 `_distance` from LanceDB converted to cosine similarity; schema cached in `self.schema` |
| `tests/integration_test.rs` | End-to-end pipeline test (index → query → assert relevance) | Requires model download on first run; run with `--test-threads=1` |
| `tests/fixtures/mini-rust/` | Small Rust source corpus (3 files) used by integration test | Not a real project — fixture only |

## Key Patterns & Conventions

### Port/Adapter split via traits

`EmbeddingProvider` and `ChunkStore` in `src/traits.rs` define the ports. `FastembedProvider` and `LanceDbStore` are the adapters. `src/main.rs` wires them together and calls only trait methods; it never references impl-specific types in the orchestration logic. To add a new embedding backend or store backend: implement the relevant trait without touching the orchestration code.

### Content-addressed chunk IDs

`ChunkId::compute` (`src/domain.rs`) hashes (file path + `line_start` + content) with SHA-256 and hex-encodes the result. This makes upserts idempotent: unchanged lines at the same location produce the same ID, and `merge_insert` updates in place. **Caveat:** if a code block moves to a different file or line number, the old `ChunkId` becomes an orphan in the store; there is no garbage-collection mechanism — rebuild the store to remove orphans.

### `spawn_blocking` for CPU-bound ONNX work

`fastembed::TextEmbedding::try_new` and `TextEmbedding::embed` are synchronous and CPU-heavy. Both are called inside `tokio::task::spawn_blocking` in `src/embed/mod.rs`. The model is wrapped in `Arc<Mutex<TextEmbedding>>` so the `Arc` clone can be moved into the blocking closure while `FastembedProvider` retains `Send + Sync`.

### Unit-norm vector contract

`FastembedProvider::embed` normalizes every output vector to unit norm before returning (see `FastembedProvider::normalize` in `src/embed/mod.rs`). `LanceDbStore::search_vector` assumes unit-norm query vectors — the score formula `1.0 - (l2_distance / 2.0)` is only correct for unit-norm pairs (max L2 = 2.0 for opposite unit vectors). Passing a non-normalized query vector produces silently wrong scores.

### Three-mode file traversal

`collect_files` in `src/main.rs` supports three mutually exclusive modes, enforced by clap's `conflicts_with` / `conflicts_with_all` annotations on the CLI flags:
1. `--include <DIR>` (one or many): walks only the listed subdirectories via plain `WalkDir`.
2. `--exclude-mode git-ignore`: uses `ignore::WalkBuilder` (respects `.gitignore`, global gitignore, `.git/info/exclude`); optional `--exclude` entries are applied on top.
3. Default (no flags): plain `WalkDir` with manual `--exclude` entries only.

The file extension allow-list (`["rs", "cc", "cpp", "cxx", "c", "h", "hpp"]`) is hard-coded in `collect_files` as a `const` slice.

### Arrow schema built once, cached

`LanceDbStore::build_schema` constructs the Arrow `Schema` once in `LanceDbStore::open` and stores it in `self.schema: Arc<Schema>`. The embedding column uses `DataType::FixedSizeList` with the dimension as the fixed size — mismatched dimensions cause a runtime error in `to_record_batch`, not a compile-time error.

## How to Extend

### Add a new embedding backend

1. Create `src/embed/<name>.rs`. Define a struct holding the client state and `dimension: usize`.
2. Implement `EmbeddingProvider` from `src/traits.rs`. The `embed` method **must** return unit-norm vectors — add a normalization step identical to `FastembedProvider::normalize`, or incorrect scores will result silently.
3. Add `pub mod <name>;` in `src/embed/mod.rs` and re-export the struct in the `pub use embed::…` line in `src/lib.rs`.
4. In `src/main.rs`, add a CLI flag (e.g., `--embedder <backend>`) and match on it in `run_index` and `run_query` to instantiate the right provider.

### Add a new CLI subcommand

1. Add a variant to the `Commands` enum in `src/main.rs`.
2. Add the corresponding `match` arm in `main()` calling a new `async fn run_<name>`.
3. Add clap fields (args, options) inside the variant's struct body.
4. No changes to traits or domain types are needed unless the command introduces a new storage or embedding operation.

### Add support for a new language

1. Add the file extension to the `EXTENSIONS` constant in `collect_files` (`src/main.rs`).
2. Add a variant to `Language` in `src/domain.rs` and update `from_extension`, `as_str`, and `from_str`. The `Infallible` error type on `from_str` means unknown strings map to `Language::Unknown` — the new variant requires an explicit `match` arm in both `from_extension` and `as_str`.
3. The compiler will emit exhaustiveness errors in any `match Language { … }` if a variant is missed — use that as a checklist.

## Type Conventions

**No serialization layer.** `Chunk`, `ChunkMetadata`, `ChunkId`, and `ScoredChunk` are plain Rust structs with `#[derive(Clone, Debug)]`. They are never serialized to JSON or TOML. Storage serialization happens only at the Arrow column level inside `LanceDbStore::to_record_batch`.

**`embedding` is `Option<Vec<f32>>`.** `Chunk.embedding` is `None` after chunking and `Some(vec)` after the embed step. The store enforces this: `to_record_batch` returns `CoderagError::Store` if any chunk has `embedding: None`. Never pass un-embedded chunks to `upsert`.

**`ChunkId` is a newtype.** The inner `String` is a hex-encoded SHA-256 hash. Construct via `ChunkId::compute` only — do not build it from arbitrary strings.

**`Language` is a closed, infallible enum.** Unknown file extensions map to `Language::Unknown` at parse time; there is no parse error. `Language::from_str` has `type Err = std::convert::Infallible`.

**`Result<T>` alias.** `coderag::Result<T>` expands to `std::result::Result<T, CoderagError>`. Use this alias throughout the library. `main.rs` uses `anyhow::Result` at the top level — anyhow wraps `CoderagError` via blanket `Into`.

## Error Handling Pattern

`CoderagError` is defined in `src/error.rs` using `thiserror`. Three variants:

- `Embedding(String)` — fastembed errors, mutex poison, and `spawn_blocking` join errors, all converted to `String` at the boundary.
- `Store(String)` — LanceDB errors, Arrow errors, schema errors; always `format!`-stringified.
- `Io(#[from] io::Error)` — automatic conversion from `std::io::Error` via `?`.

Errors propagate with `?` throughout the library. At `main`, `anyhow::Result` wraps `CoderagError` via the blanket `Into` impl. There is no structured error logging: errors bubble to `main` and are formatted by anyhow's default formatter. Non-fatal file conditions — unreadable files, empty-after-filtering — are handled with `tracing::warn!` or `tracing::debug!` in `run_index` and silently skipped without returning an error.

## Testing Pattern

**Unit tests** live in `#[cfg(test)]` modules inside the source files. The canonical example is `src/chunker.rs`: a `make_content(n_lines)` helper generates synthetic Rust-like content; individual test functions verify chunk count, overlap correctness, ID determinism, empty-file behavior, and path-sensitivity of IDs. Run with `cargo nextest run` (or `just test`).

**Integration test** is in `tests/integration_test.rs`. It indexes three fixture files from `tests/fixtures/mini-rust/src/` (`.rs` files: `io.rs`, `config.rs`, `processor.rs`) into a `tempfile::TempDir`-backed LanceDB store, queries for natural-language descriptions, and asserts that the top result comes from the semantically correct file. No external services or infrastructure required. The model (`JinaEmbeddingsV2BaseCode`, ~600 MB) is downloaded on first run and cached in `~/.cache/fastembed/`. Run integration tests with `--test-threads=1` to avoid concurrent model-load contention (noted in the test file comment).

**No mocks.** The `EmbeddingProvider` and `ChunkStore` traits are designed to be implemented directly by real adapters; no mock implementations exist in the current test suite. Tests use real `FastembedProvider` and `LanceDbStore` instances.

## What NOT to Do

_[Human-authored section — fill this after using the codebase. Not in inferred_sections.]_
