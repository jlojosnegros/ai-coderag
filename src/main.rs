//! coderag binary entry point
//!
//! Two subcommands:
//! - 'serve' : start the MCP server on stdio
//! - 'index' : pre-warm the embedding index

use std::path::PathBuf;

use clap::{Parser, Subcommand};
use coderag::{CoderagConfig, CoderagServer};
use rmcp::ServiceExt;
use tracing_subscriber::fmt::format::FmtSpan;

/// Code intelligence MCP server for AI agents
#[derive(Parser)]
#[command(name = "coderag", version)]
struct Cli {
    /// Path to coderag.toml config file.
    /// If NOT specified, searches upward from the current working directory.
    #[arg(long, global = true)]
    config: Option<PathBuf>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Start the MCP server (stdio transport)
    Serve,
    /// Index a directory of source files
    Index {
        /// Directory containing source files to index
        path: PathBuf,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Init tracing BEFORE config loading so that config parse warnings are visible
    // to the user on stderr.
    // stdout is RESERVED for MCP stdio transport
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env().add_directive("coderag=info".parse()?))
        .with_span_events(FmtSpan::NEW | FmtSpan::CLOSE)
        .init();

    let cli = Cli::parse();

    // Resolve config once for all subcommands.
    // Priority: --config flag > coderag.toml found from cwd upward > defaults
    let config = match &cli.config {
        Some(path) => CoderagConfig::load_from_file(path),
        None => CoderagConfig::load_from_dir(&std::env::current_dir()?),
    };

    match cli.command {
        Commands::Serve => run_serve(config).await,
        Commands::Index { .. } => {
            anyhow::bail!("The index command is not yet available")
        },
    }
}
async fn run_serve(config: CoderagConfig) -> anyhow::Result<()> {
    tracing::info!("Starting coderag MCP server v{}", env!("CARGO_PKG_VERSION"));

    let server = CoderagServer::new(config);

    // serve(stdio()) does two things:
    // 1. Performs the MCP initialize handshake with the client
    // 2. Returns a service handle that dispatches incoming tool calls
    //
    // stdio() creates a transport that reads JSON-RPC from stdin and writes responses to stdout
    let service = server.serve(rmcp::transport::stdio()).await?;

    // Block until the client closes the connection (stdin EOF)
    service.waiting().await?;

    Ok(())
}
