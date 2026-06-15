use std::{
    fs::read_to_string,
    path::{Path, PathBuf},
};

use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct LspConfig {
    #[serde(default)]
    pub enabled: bool,

    #[serde(default = "default_rust_analyzer_path")]
    pub rust_analyzer_path: String,

    #[serde(default = "default_timeout_secs")]
    pub timeout_secs: u64,

    pub project_path_filter: Option<String>,
}

fn default_rust_analyzer_path() -> String {
    "rust-analyzer".to_string()
}

fn default_timeout_secs() -> u64 {
    30
}

impl Default for LspConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            rust_analyzer_path: default_rust_analyzer_path(),
            timeout_secs: default_timeout_secs(),
            project_path_filter: None,
        }
    }
}

#[derive(Debug, Default, Deserialize)]
pub struct IndexerConfig {
    #[serde(default)]
    pub exclude_patterns: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct StoreConfig {
    #[serde(default = "default_store_path")]
    pub path: String,
}
fn default_store_path() -> String {
    ".coderag".to_string()
}

impl Default for StoreConfig {
    fn default() -> Self {
        Self {
            path: default_store_path(),
        }
    }
}

#[derive(Debug, Default, Deserialize)]
pub struct CoderagConfig {
    #[serde(default)]
    pub indexer: IndexerConfig,

    #[serde(default)]
    pub lsp: LspConfig,

    #[serde(default)]
    pub store: StoreConfig,
}

impl CoderagConfig {
    /// Load config from `coderag.toml` in `start_dir` or any ancestor directory
    /// Returns default config (lsp disabled) if no config file is found.
    pub fn load_from_dir(start_dir: &Path) -> Self {
        let mut dir = start_dir.to_path_buf();
        loop {
            let config_path = dir.join("coderag.toml");
            if config_path.exists() {
                return Self::load_file(&config_path);
            }
            match dir.parent() {
                Some(parent) => dir = parent.to_path_buf(),
                None => break,
            }
        }
        tracing::debug!("No coderag.toml found, using defaults (lsp disabled)");
        Self::default()
    }

    fn load_file(path: &Path) -> Self {
        let content = match read_to_string(path) {
            Ok(content) => content,
            Err(err) => {
                tracing::warn!(file_path = %&path.display(),
                    error = %err.to_string(),
                    "Cannot read config file. Use defaults (lsp disabled) ");
                return Self::default();
            },
        };

        match toml::from_str(&content) {
            Ok(cfg) => {
                tracing::info!(file_path = %&path.display(), "Loaded config from file");
                cfg
            },
            Err(err) => {
                tracing::warn!(
                    file_path = %&path.display(),
                    error = %err.to_string(),
                    "Invalid coderag.toml. Using defaults");
                Self::default()
            },
        }
    }

    /// Find the rust project root ( folder with Cargo.toml) starting
    /// from `dir` and searching upwards
    pub fn find_cargo_root(dir: &Path) -> Option<PathBuf> {
        let mut current = dir.to_path_buf();
        loop {
            if current.join("Cargo.toml").exists() {
                return Some(current);
            }
            match current.parent() {
                Some(p) => current = p.to_path_buf(),
                None => return None,
            }
        }
    }
}
