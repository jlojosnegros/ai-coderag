use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
};

use clap::{Parser, Subcommand};
use coderag::{
    AstChunker, CandleProvider, Chunk, ChunkStore, ChunkType, CoderagConfig, EmbeddingProvider, LanceDbStore,
    LspClient, registry::LanguageRegistry,
};
use tracing::instrument;
use tracing_subscriber::fmt::format::FmtSpan;

#[derive(Parser)]
#[command(
    name = "coderag",
    about = "Semantic code search powered by local embeddings",
    version
)]
struct Cli {
    /// Path to the LanceDB index directory
    #[arg(long, global = true, default_value = ".coderag")]
    db: String,

    /// Path to coderag.toml config file.
    /// If NOT specified, searches upward from the current working directory.
    #[arg(long, global = true)]
    config: Option<PathBuf>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Index a directory of source files
    Index {
        /// Directory containing source files to index
        path: PathBuf,

        /// Exclusion strategy. 'git-ignore' respects .gitignore, the user's
        /// global gitignore, and .git/info/exclude, the same files git ifself ignores.
        #[arg(long = "exclude-mode", value_name = "MODE", conflicts_with = "include")]
        exclude_mode: Option<ExcludeMode>,

        /// Exclude a directory from indexing. Can be specified multiple times.
        /// Matches any directory component of the path (e.g. --exclude target
        /// excludes both ./target/ and ./subdir/target/).
        /// Can be combined with --exclude-mode git-ignore (effects are additive).
        #[arg(long, value_name = "DIR", conflicts_with = "include")]
        exclude: Vec<String>,

        /// Index only this directory. Can be specified multiple times.
        /// Mutually exclusive with --exclude and --exclude-mode.
        #[arg(long, value_name = "DIR", conflicts_with_all = ["exclude", "exclude_mode"])]
        include: Vec<String>,
    },
    /// Search indexed code by semantic similarity
    Query {
        /// Natural language description of what you are looking for
        text: String,
        /// Number of results to return
        #[arg(short = 'n', long, default_value = "5")]
        top: usize,
    },
}
#[derive(Debug, Clone)]
enum ExcludeMode {
    GitIgnore,
}

impl std::str::FromStr for ExcludeMode {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "git-ignore" => Ok(Self::GitIgnore),
            other => Err(format!("Unknown exclude mode '{other}'. Valid value: 'git-ignore'")),
        }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env().add_directive("coderag=info".parse()?))
        .with_span_events(FmtSpan::NEW | FmtSpan::CLOSE)
        .init();

    let cli = Cli::parse();

    // Resolve config once for all subcommands.
    // Priority: --config flag > coderag.toml found from cwd upward > defaults
    let mut config = match &cli.config {
        Some(path) => CoderagConfig::load_from_file(path),
        None => CoderagConfig::load_from_dir(&std::env::current_dir()?),
    };

    // --db overrides store.path from config file
    if cli.db != ".coderag" {
        config.store.path = cli.db.clone();
    }

    match cli.command {
        Commands::Index {
            path,
            exclude_mode,
            exclude,
            include,
        } => run_index(path, &config, exclude_mode, exclude, include).await?,
        Commands::Query { text, top } => run_query(text, top, &config).await?,
    }

    Ok(())
}

async fn init_lsp_clients(
    registry: &LanguageRegistry,
    config: &CoderagConfig,
    path: &Path,
) -> HashMap<String, LspClient> {
    let mut clients = HashMap::new();
    if !config.lsp.enabled {
        return clients;
    }

    for lsp_provider in registry.lsp_providers() {
        let lang_id = lsp_provider.language_id().to_string();

        if let Some(root) = lsp_provider.find_project_root(path) {
            let default_args = lsp_provider.default_args();
            let default_command = lsp_provider.default_command();
            let server_config = config.lsp.server_config(&lang_id, default_command, &default_args);

            match LspClient::new(lsp_provider.as_ref(), &server_config, &root).await {
                Ok(client) => {
                    if clients.insert(lang_id.clone(), client).is_some() {
                        tracing::warn!("Duplicate language_id '{lang_id}' : replaced existing LSP client");
                    }
                },
                Err(err) => {
                    tracing::warn!("LSP init for {lang_id}: {err}. Indexing without LSP");
                },
            }
        } else {
            tracing::warn!(
                "No project root found for {} from {}. Indexing without LSP",
                lang_id,
                path.display(),
            )
        }
    }
    clients
}

#[instrument(name = "index")]
async fn run_index(
    path: PathBuf,
    config: &CoderagConfig,
    exclude_mode: Option<ExcludeMode>,
    exclude: Vec<String>,
    include: Vec<String>,
) -> anyhow::Result<()> {
    let embedder = CandleProvider::new().await?;
    let store = LanceDbStore::open(&config.store.path, embedder.dimension()).await?;
    let registry = Arc::new(LanguageRegistry::with_builtins());
    let chunker = AstChunker::new(registry.clone());

    // Initialize LSP client if enabled
    let mut lsp_clients = init_lsp_clients(&registry, config, &path).await;

    let extensions = ["rs", "cc", "cpp", "cxx", "c", "h", "hpp"];
    let mut total_files = 0usize;
    let mut total_chunks = 0usize;

    for entry in walkdir::WalkDir::new(path)
        .follow_links(false)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
    {
        let file_path = entry.path();
        let ext = file_path.extension().and_then(|ext| ext.to_str()).unwrap_or("");
        if !extensions.contains(&ext) {
            continue;
        }

        let content = match std::fs::read_to_string(file_path) {
            Ok(c) => c,
            Err(err) => {
                tracing::warn!("Skipping {}:{}", file_path.display(), err);
                continue;
            },
        };

        let mut chunks = chunker.chunk_file(file_path, &content);
        if chunks.is_empty() {
            continue;
        }

        // Enrich with callers via LSP if available and the file is Rust
        if let Some(lsp_provider) = registry.lsp_for_extension(ext) {
            if let Some(lsp_client) = lsp_clients.get_mut(lsp_provider.language_id()) {
                enrich_with_lsp(lsp_client, file_path, &mut chunks).await;
            }
        }

        // Embed all chunks in a single batch call
        let texts = chunks.iter().map(|c| c.content.as_str()).collect::<Vec<_>>();
        let embeddings = embedder.embed(&texts).await?;
        for (chunk, embedding) in chunks.iter_mut().zip(embeddings) {
            chunk.embedding = Some(embedding);
        }

        let n = chunks.len();
        store.upsert(&chunks).await?;

        let type_summary = build_type_summary(&chunks);
        tracing::info!("Indexed {} ({} chunks [{}]", file_path.display(), n, type_summary);
        total_files += 1;
        total_chunks += n;
    }

    // Cleanly shut down the LSP server
    for (lang_id, mut client) in lsp_clients {
        tracing::debug!("Shutting down LSP for {lang_id}");
        let _ = client.shutdown().await;
    }

    println!("Done. Indexed {total_files} files, {total_chunks} chunks");
    Ok(())
}

/// Query LSP for callers of each chunk's symbol and store them in the chunk
async fn enrich_with_lsp(lsp: &mut LspClient, file_path: &std::path::Path, chunks: &mut [Chunk]) {
    // Get document symbols: maps symbol name -> position (line, character)
    let symbols = match lsp.document_symbols(file_path).await {
        Ok(s) => s,
        Err(err) => {
            tracing::debug!("document_symbols failed for {}: {err}", file_path.display());
            return;
        },
    };

    for chunk in chunks.iter_mut() {
        // only query callers for named function/method chunks.
        let symbol_name = match &chunk.metadata.symbol_name {
            Some(name) => name.clone(),
            None => continue,
        };

        if !matches!(chunk.metadata.chunk_type, ChunkType::Function | ChunkType::Method) {
            continue;
        }

        // Find the LSP symbol that matches this chunk;s name
        let symbol = symbols.iter().find(|symbol| symbol.name == symbol_name);

        let sym = match symbol {
            Some(s) => s,
            None => continue,
        };

        match lsp
            .references_at(file_path, sym.selection_start_line, sym.selection_start_char)
            .await
        {
            Ok(callers) => {
                let n = callers.len();
                chunk.metadata.callers = callers;
                if n > 0 {
                    tracing::info!("Enriched {symbol_name}: {n} caller(s)");
                }
            },
            Err(err) => {
                tracing::debug!("references_at failed for {symbol_name}: {err}");
            },
        }
    }
}

fn build_type_summary(chunks: &[Chunk]) -> String {
    let mut counts: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    for chunk in chunks {
        *counts.entry(chunk.metadata.chunk_type.as_str()).or_insert(0) += 1;
    }

    let mut parts = counts.iter().map(|(k, v)| format!("{k}:{v}")).collect::<Vec<_>>();
    parts.sort();
    parts.join(", ")
}

#[instrument(name = "query")]
async fn run_query(text: String, top: usize, config: &CoderagConfig) -> anyhow::Result<()> {
    let embedder = CandleProvider::new().await?;
    let store = LanceDbStore::open(&config.store.path, embedder.dimension()).await?;

    let embeddings = embedder.embed(&[text.as_str()]).await?;
    let results = store.search_vector(&embeddings[0], top).await?;

    if results.is_empty() {
        println!("No results found. Have you run `coderag index <path>` first?");
        return Ok(());
    }

    for (idx, scored) in results.iter().enumerate() {
        let meta = &scored.chunk.metadata;
        print!(
            "\n--- Result {} (score: {:.3}) ---\n{}  [lines {}-{}]",
            idx + 1,
            scored.score,
            meta.file_path.display(),
            meta.line_start,
            meta.line_end,
        );

        if let Some(name) = &meta.symbol_name {
            print!("  fn {name}");
        }
        println!();

        // Show callers if available.
        if !meta.callers.is_empty() {
            println!("  called by: {}", meta.callers.join(", "));
        }

        println!("\n{}", scored.chunk.content);
    }

    Ok(())
}
