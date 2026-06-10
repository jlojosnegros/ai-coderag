# coderag

> Semantic code search powered by local embeddings — no API keys, no servers, no cloud.

**Status:** Phase 0 MVP — v0.1.0 | **Language:** Rust 1.85+ (edition 2024) | **Embedding model:** JinaEmbeddingsV2BaseCode (768-dim, local ONNX)

---

coderag indexes your Rust and C++ source code into a local vector database and lets you search it with natural language queries. Everything runs in-process: the embedding model executes locally via ONNX, and the index lives in a directory on disk.

```
$ coderag index ./src --exclude-mode git-ignore
[INFO] Model loaded. Embedding dimension: 768
[INFO] Indexed src/server.rs (4 chunks)
[INFO] Indexed src/handler.rs (6 chunks)
[INFO] Done. Indexed 12 files, 38 chunks.

$ coderag query "how are HTTP connections pooled"

--- Result 1 (score: 0.871) ---
src/connection_pool.rs [lines 14-54]

pub struct ConnectionPool {
    max_size: usize,
    connections: Mutex<Vec<TcpStream>>,
    ...
```

## Features

- **Local-first** — model and index run entirely on your machine; source code never leaves your environment
- **Semantic search** — understands meaning, not just keywords: "parse command line arguments" finds `clap` setup code
- **Rust and C++** — indexes `.rs`, `.cc`, `.cpp`, `.cxx`, `.c`, `.h`, `.hpp`
- **Flexible filtering** — respect `.gitignore`, exclude directories manually, or restrict indexing to specific subdirectories
- **Fast re-runs** — model is cached after first download; subsequent runs start in seconds

## Prerequisites

| Requirement | Notes |
|---|---|
| Rust 1.85+ (edition 2024) | `rustup update stable` |
| ~600 MB disk (first run) | Model downloads to `~/.cache/fastembed/` on first `index` |
| ~1 GB RAM | Model is held in memory while running |

No API keys. No Docker. No database server.

## Installation

```bash
git clone https://github.com/jlojosnegros/coderag
cd coderag
cargo build --release
# Binary at ./target/release/coderag
```

To make it available system-wide:

```bash
cargo install --path .
```

## Usage

### Index a codebase

```bash
# Index current directory, respecting .gitignore
coderag index . --exclude-mode git-ignore

# Index a specific path, manually excluding build artifacts
coderag index /path/to/project --exclude target --exclude .git

# Index only specific subdirectories
coderag index . --include src --include tests

# Use a custom index location (default: .coderag/)
coderag index . --db /tmp/my-index --exclude-mode git-ignore
```

> **First run**: the embedding model (`JinaEmbeddingsV2BaseCode`, ~600 MB) is downloaded
> from Hugging Face on first use and cached in `~/.cache/fastembed/`. Subsequent runs
> load it from cache and start in a few seconds.

### Search

```bash
# Basic query (returns top 5 results)
coderag query "error handling for network timeouts"

# Return more results
coderag query "parse command line arguments" -n 10

# Query against a custom index location
coderag query "connection pooling" --db /tmp/my-index
```

### All options

```
coderag [--db <PATH>] <COMMAND>

Options:
  --db <PATH>   Path to the LanceDB index directory [default: .coderag]

Commands:
  index <PATH> [OPTIONS]
    --exclude-mode <MODE>   Exclusion strategy. Valid value: git-ignore
    --exclude <DIR>         Exclude a directory (repeatable; combines with --exclude-mode)
    --include <DIR>         Index only this directory (repeatable; mutually exclusive with --exclude*)

  query <TEXT> [OPTIONS]
    -n, --top <N>           Number of results to return [default: 5]
```

## How it works

1. **Chunking** — source files are split into overlapping windows of ~40 lines
2. **Embedding** — each chunk is converted to a 768-dimensional vector using `JinaEmbeddingsV2BaseCode` (a model trained specifically on source code)
3. **Storage** — vectors and metadata are stored in a [LanceDB](https://lancedb.com) embedded database (a directory of `.lance` files)
4. **Search** — a query is embedded with the same model, and the nearest vectors are retrieved using approximate nearest-neighbor search

The index is a plain directory (`.coderag/` by default). You can copy it, back it up, or delete and rebuild it at any time.

## Logging

Set `RUST_LOG` to control verbosity:

```bash
RUST_LOG=debug coderag index ./src    # verbose: shows every file visited
RUST_LOG=warn  coderag index ./src    # quiet: only warnings and errors
```

## Roadmap

- [ ] AST-aware chunking via tree-sitter (functions, structs, traits as chunk boundaries)
- [ ] Call graph enrichment via rust-analyzer / clangd (LSP)
- [ ] MCP server for Claude Code integration
- [ ] Incremental indexing (skip unchanged files by content hash)
- [ ] Hybrid search (BM25 + vector with Reciprocal Rank Fusion)
- [ ] Qdrant backend for team shared indexes

## License

MIT

---

_coderag v0.1.0 — Rust 1.85+ (edition 2024) / fastembed 5.x / lancedb 0.30_
