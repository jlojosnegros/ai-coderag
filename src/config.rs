use std::{collections::HashMap, fs::read_to_string, path::Path};

use serde::Deserialize;

use crate::traits::LspServerConfig;

#[derive(Debug, Default, Deserialize)]
pub struct LspConfig {
    #[serde(default)]
    pub enabled: bool,

    /// Per-language server configuration.
    /// Key is the language_id (e.g. "rust", "cpp", "go")
    /// Overrides the defaults from the LanguageLsp trait.
    #[serde(default)]
    pub servers: HashMap<String, LspServerConfigToml>,
}

/// TOML representation of a language server config entry.
///
/// `command` is optional: if absent, the default from the LanguageLsp trait is used.
#[derive(Debug, Clone, Deserialize)]
pub struct LspServerConfigToml {
    pub command: Option<String>,

    #[serde(default)]
    pub args: Option<Vec<String>>,

    #[serde(default = "default_timeout_secs")]
    pub timeout_secs: u64,
}

fn default_timeout_secs() -> u64 {
    30
}

impl LspConfig {
    /// Produces a resolved LspServerConfig by merging coderag.toml
    /// overrides with the trait defaults.
    /// Priority: TOML > trait defaults
    pub fn server_config(&self, language_id: &str, default_command: &str, default_args: &[String]) -> LspServerConfig {
        match self.servers.get(language_id) {
            Some(toml) => LspServerConfig {
                command: toml.command.clone().unwrap_or_else(|| default_command.to_string()),
                args: match &toml.args {
                    Some(a) => a.clone(),
                    None => default_args.to_vec(),
                },
                timeout_secs: toml.timeout_secs,
            },
            None => LspServerConfig {
                command: default_command.to_string(),
                args: default_args.to_vec(),
                timeout_secs: default_timeout_secs(),
            },
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
    /// Returns default config (lsp disabled) if no config file is found
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
        tracing::debug!("No 'coderag.toml' found, using defaults (lsp disabled) ");
        Self::default()
    }

    fn load_file(path: &Path) -> Self {
        let content = match read_to_string(path) {
            Ok(content) => content,
            Err(err) => {
                tracing::warn!(
                    file_path = %&&path.display(),
                    error = %err.to_string(),
                    "Cannot read config file. Using defaults (lsp disabled)"
                );
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
                    "Invalid 'coderag.toml'. Using defaults"
                );
                Self::default()
            },
        }
    }

    /// Load config from an explicit file path (does NOT search upwards)
    pub fn load_from_file(path: &Path) -> Self {
        Self::load_file(path)
    }
}

#[cfg(test)]
mod tests {

    use super::*;

    #[test]
    fn server_config_uses_trait_defaults_when_no_config() {
        let lsp_config = LspConfig::default();
        let resolved = lsp_config.server_config("rust", "rust-analyzer-hey", &[]);

        assert_eq!(resolved.command, "rust-analyzer-hey");
        assert!(resolved.args.is_empty());
        assert_eq!(resolved.timeout_secs, default_timeout_secs());
    }

    #[test]
    fn server_config_overrides_command() {
        let mut servers = HashMap::new();
        servers.insert(
            "rust".to_string(),
            LspServerConfigToml {
                command: Some("/opt/rust-analyzer-nightly".to_string()),
                args: Some(Vec::new()),
                timeout_secs: 60,
            },
        );
        let lsp_config = LspConfig { enabled: true, servers };
        let resolved = lsp_config.server_config("rust", "rust-analyzer", &[]);
        assert_eq!(resolved.command, "/opt/rust-analyzer-nightly");
        assert_eq!(resolved.timeout_secs, 60);
    }

    #[test]
    fn server_config_uses_trait_command_when_toml_has_none() {
        let mut servers = HashMap::new();
        servers.insert(
            "rust".to_string(),
            LspServerConfigToml {
                command: None,
                args: Some(Vec::new()),
                timeout_secs: 45,
            },
        );
        let lsp_config = LspConfig { enabled: true, servers };
        let resolved = lsp_config.server_config("rust", "rust-analyzer", &[]);
        assert_eq!(resolved.command, "rust-analyzer");
        assert_eq!(resolved.timeout_secs, 45);
    }

    #[test]
    fn lsp_config_deserializes_from_toml() {
        let toml_str = r#"
            enabled = true

            [servers.rust]
            command = "rust-analyzer"
            timeout_secs = 30

            [servers.cpp]
            command = "clangd"
            args = ["--background-index"]
            timeout_secs = 60
        "#;
        let config: LspConfig = toml::from_str(toml_str).unwrap();
        assert!(config.enabled);
        assert_eq!(config.servers.len(), 2);
        assert_eq!(config.servers["rust"].command.as_deref(), Some("rust-analyzer"));
        assert_eq!(config.servers["cpp"].command.as_deref(), Some("clangd"));
        assert_eq!(config.servers["cpp"].args, Some(vec!["--background-index".to_owned()]));
        assert_eq!(config.servers["cpp"].timeout_secs, 60);
    }
}
