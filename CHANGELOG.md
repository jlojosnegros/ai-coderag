# Changelog

All notable changes to this project will be documented in this file.
Versions follow [Semantic Versioning](https://semver.org/).

## [0.3.0] - 2026-06-11

### Bug Fixes

#### Release
- Stage CHANGELOG.md before cargo-release commit



### Documentation
- Agentdoc maintain
- Improve agent-overlay
- Update README


### Features

#### Cli,test
- Integrate AstChunker and add Phase 1 integration tests


#### Domain
- Add ChunkType enum and semantic metadata fields


#### Parser
- Add AST-aware chunker with Rust and C++ plugins


#### Store
- Expand LanceDB schema for AST metadata and add column constants



## [0.2.0] - 2026-06-10

### Documentation
- Improve agent-overlay
- Add README, CLAUDE.md and agentdoc overlay scaffold


### Features

#### Chunker
- Add LineChunker with sliding-window overlap and unit tests


#### Cli
- Implement index and query commands with directory filtering


#### Core
- Add CoderagError, domain types and async provider traits


#### Embed
- Add FastembedProvider with JinaEmbeddingsV2BaseCode


#### Store
- Add LanceDbStore with Arrow schema, merge_insert upsert and ANN search




