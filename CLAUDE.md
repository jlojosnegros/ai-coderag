# CLAUDE.md

> For architecture, patterns, and how to extend this codebase,
> read [`docs/agent-overlay.md`](docs/agent-overlay.md) before touching code.

## Task runner

`just` — see `Justfile` for all recipes. Most-used: `just dev`, `just test`, `just check`, `just ci`.

## Commands

| Goal | Command |
| ---- | ------- |
| Debug build | `just dev` |
| Release build | `just build` |
| Run all tests | `just test` |
| Lint + fmt check | `just check` |
| Format (nightly) | `just format` |
| Coverage | `just coverage` |
| Full CI gate | `just ci` |
| Docs | `just docs` |
| Release | `just release <version>` |

> `just check` requires nightly Rust (`cargo +nightly fmt --check`). Install with `rustup toolchain install nightly`.

## Pre-commit hooks

Managed by [lefthook](https://github.com/evilmartians/lefthook) (`lefthook.yaml`). Hooks run automatically on `git commit`.

| Hook | Enforces |
| ---- | -------- |
| `check-readme-no-mermaid` | README.md must not contain inline `` ```mermaid `` blocks; use SVG files from `docs/diagrams/` |
| `check-diagram-svgs` | When a `.mmd` source is staged, its rendered `.svg` must also be staged (`just diagrams`) |
| `check-readme-diagram-refs` | README references to SVGs must match files on disk; removed references must be deleted |
| `validate-conventional-commit` | Commit messages must follow Conventional Commits: `type(scope): description` |

Skipping hooks (`--no-verify`) bypasses the Conventional Commits check and diagram consistency — both are enforced by CI too, so a push will still fail.

## Source layout

| Path | Contents |
| ---- | -------- |
| `src/main.rs` | CLI entry point: `index` and `query` subcommands, file traversal, orchestration |
| `src/lib.rs` | Library root; re-exports all public types, traits, and impls |
| `src/traits.rs` | `EmbeddingProvider` and `ChunkStore` traits (the port layer) |
| `src/domain.rs` | Core domain types: `Chunk`, `ChunkId`, `ChunkMetadata`, `Language`, `ScoredChunk` |
| `src/error.rs` | `CoderagError` enum + `Result<T>` alias |
| `src/chunker.rs` | `LineChunker`: sliding-window line-based chunker |
| `src/embed/` | `FastembedProvider`: fastembed/ONNX embedding, blocks on model load |
| `src/store/` | `LanceDbStore`: Arrow schema, `merge_insert` upsert, ANN vector search |
| `tests/` | Integration tests (no external services needed) |
| `tests/fixtures/` | Small source file fixtures for integration tests |
| `docs/diagrams/` | Mermaid `.mmd` sources and rendered `.svg` files |

## Tests

```bash
just test         # unit + integration (cargo nextest run)
just coverage     # full coverage report (requires cargo-llvm-cov)
just coverage-ci  # fails if coverage < 80%
```

Unit tests live inside `#[cfg(test)]` modules in the source files. Integration tests are in `tests/`. No external cluster, database server, or Docker is required.

## Error handling

`CoderagError` (in `src/error.rs`, via `thiserror`). Three variants: `Embedding(String)`, `Store(String)`, `Io(#[from] io::Error)`. `Result<T>` is a type alias for `std::result::Result<T, CoderagError>`. Errors bubble up through `?` to `main`, which returns `anyhow::Result`.

## Release

```bash
just release <version>     # requires cargo-release and git-cliff
just check-release <version>  # dry-run
```

Never release manually. `cargo release` handles version bump, changelog, and tag.
