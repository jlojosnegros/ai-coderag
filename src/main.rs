use std::{
    collections::HashMap,
    fs::read_to_string,
    path::{Component, Path, PathBuf},
};

use clap::{Parser, Subcommand};
use coderag::{AstChunker, ChunkStore, EmbeddingProvider, FastembedProvider, LanceDbStore, ScoredChunk};
use tracing::instrument;
use tracing_subscriber::fmt::format::FmtSpan;
use walkdir::WalkDir;

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

    match cli.command {
        Commands::Index {
            path,
            exclude_mode,
            exclude,
            include,
        } => run_index(path, cli.db, exclude_mode, exclude, include).await?,
        Commands::Query { text, top } => run_query(text, top, cli.db).await?,
    }

    Ok(())
}

#[instrument(name = "index")]
async fn run_index(
    path: PathBuf,
    db: String,
    exclude_mode: Option<ExcludeMode>,
    exclude: Vec<String>,
    include: Vec<String>,
) -> anyhow::Result<()> {
    let embedder = FastembedProvider::new().await?;
    let store = LanceDbStore::open(&db, embedder.dimension()).await?;
    let chunker = AstChunker::default();

    let files = collect_files(&path, exclude_mode.as_ref(), &exclude, &include);

    let mut total_files = 0usize;
    let mut total_chunks = 0usize;

    for file_path in &files {
        tracing::trace!(file_path = %file_path.display(), "Processing ... ");

        let content = match read_to_string(file_path) {
            Ok(c) => c,
            Err(err) => {
                tracing::warn!("Skipping {} : {}", file_path.display(), err);
                continue;
            },
        };

        let mut chunks = chunker.chunk_file(file_path, &content);
        if chunks.is_empty() {
            tracing::debug!(file_path = %file_path.display(), "No chunks. Skipping");
            continue;
        }

        // Embed all chunks from this file in a single batch call.
        let texts = chunks.iter().map(|chunk| chunk.content.as_str()).collect::<Vec<_>>();
        let embeddings = embedder.embed(&texts).await?;

        for (chunk, embedding) in chunks.iter_mut().zip(embeddings) {
            chunk.embedding = Some(embedding);
        }

        let n = chunks.len();
        store.upsert(&chunks).await?;

        let type_summary = {
            let mut counts: HashMap<&str, usize> = HashMap::new();
            for chunk in &chunks {
                *counts.entry(chunk.metadata.chunk_type.as_str()).or_insert(0) += 1;
            }
            counts
                .iter()
                .map(|(k, v)| format!("{k}:{v}"))
                .collect::<Vec<_>>()
                .join(",")
        };

        tracing::info!(file_path = %&file_path.display(), chunks=n, summary = type_summary,  "File Indexed");

        total_files += 1;
        total_chunks += n;
    }

    if total_files == 0 {
        tracing::warn!(path = %&path.display(), "No source files found in path");
    } else {
        tracing::info!(path = %&path.display(), total_files, total_chunks, "Done. files Indexed");
    }

    Ok(())
}

#[instrument(name = "query")]
async fn run_query(text: String, top: usize, db: String) -> anyhow::Result<()> {
    let embedder = FastembedProvider::new().await?;
    let store = LanceDbStore::open(&db, embedder.dimension()).await?;

    let embeddings = embedder.embed(&[text.as_str()]).await?;
    let query_vec = &embeddings[0];

    let results = store.search_vector(query_vec, top).await?;

    if results.is_empty() {
        tracing::warn!("No results found. Have you run `coderag index <path>` first?");
        return Ok(());
    }

    display_results(&results, &mut std::io::stdout())?;
    Ok(())
}

fn display_results(results: &[ScoredChunk], writer: &mut impl std::io::Write) -> anyhow::Result<()> {
    for (idx, scored) in results.iter().enumerate() {
        let meta = &scored.chunk.metadata;

        writeln!(writer, "\n--- Result {} (score: {:.3}) ---", idx + 1, scored.score)?;

        // File + line range + symbol name on one line, matching phase-02 expected output format
        write!(
            writer,
            "{} [lines {}-{}]",
            meta.file_path.display(),
            meta.line_start,
            meta.line_end
        )?;
        if let Some(name) = &meta.symbol_name {
            write!(writer, "  {} {name}", meta.chunk_type.as_str())?;
        }
        writeln!(writer)?;

        writeln!(writer, "\n{}", scored.chunk.content)?;
    }
    Ok(())
}

fn collect_files(
    path: &Path,
    exclude_mode: Option<&ExcludeMode>,
    exclude: &[String],
    include: &[String],
) -> Vec<PathBuf> {
    const EXTENSIONS: &[&str] = &["rs", "cc", "cpp", "cxx", "c", "h", "hpp"];

    let has_source_ext = |path: &Path| -> bool {
        path.extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| EXTENSIONS.contains(&ext))
            .unwrap_or(false)
    };

    // check whether any directory component of `path` matches an excluded name.
    let is_excluded = |path: &Path| -> bool {
        path.components().any(|component| {
            if let Component::Normal(name) = component {
                exclude.iter().any(|ex| name.to_string_lossy().as_ref() == ex.as_str())
            } else {
                false
            }
        })
    };

    let mut files = Vec::new();

    if !include.is_empty() {
        // Mode 1: "--include" => walk only the listed subdirectories.
        for dir in include {
            for entry in WalkDir::new(path.join(dir))
                .follow_links(false)
                .into_iter()
                .filter_map(|entry| entry.ok())
                .filter(|entry| entry.file_type().is_file())
            {
                if has_source_ext(entry.path()) {
                    files.push(entry.path().to_path_buf());
                }
            }
        }
    } else if matches!(exclude_mode, Some(ExcludeMode::GitIgnore)) {
        // Mode 2 : --exclude-mode git-ignore
        // hidden(false) => do NOT skip hidden files by default
        // Manual --exclude entries are applied on top
        for entry in ignore::WalkBuilder::new(path)
            .hidden(false)
            .build()
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_type().map(|file_type| file_type.is_file()).unwrap_or(false))
        {
            let entry_path = entry.path();
            if has_source_ext(entry_path) && !is_excluded(entry_path) {
                files.push(entry_path.to_path_buf());
            }
        }
    } else {
        // Mode 3: plain Walkdir with manual --exclude
        for entry in WalkDir::new(path)
            .follow_links(false)
            .into_iter()
            .filter_entry(|entry| {
                if entry.file_type().is_dir() {
                    let name = entry.file_name().to_string_lossy();
                    !exclude.iter().any(|ex| name.as_ref() == ex.as_str())
                } else {
                    true
                }
            })
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_type().is_file())
        {
            if has_source_ext(entry.path()) {
                files.push(entry.path().to_path_buf());
            }
        }
    }
    files
}
