use std::sync::Arc;

use crate::{
    RustLsp,
    parser::{CppPlugin, LanguageParser, RustPlugin},
    traits::LanguageLsp,
};

/// Centrar registry of language support: parsers and LSP providers.
///
/// Parsers and LSP providers are registered independently.
/// A language can have a parser without LSP (like C++ today) or
/// LSP without a custom parser
pub struct LanguageRegistry {
    parsers: Vec<Arc<dyn LanguageParser>>,
    lsp_providers: Vec<Arc<dyn LanguageLsp>>,
}

impl Default for LanguageRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl LanguageRegistry {
    pub fn new() -> Self {
        Self {
            parsers: Vec::new(),
            lsp_providers: Vec::new(),
        }
    }

    /// Create a registry with built-in parsers and providers
    /// today: Rust and C++ parsers, plus Rust LSP
    pub fn with_builtins() -> Self {
        let mut reg = Self::new();

        reg.register_parser(RustPlugin::new());
        reg.register_parser(CppPlugin::new());

        reg.register_lsp(RustLsp);

        reg
    }

    pub fn register_parser(&mut self, parser: impl LanguageParser + 'static) {
        self.parsers.push(Arc::new(parser));
    }

    pub fn register_lsp(&mut self, lsp: impl LanguageLsp + 'static) {
        self.lsp_providers.push(Arc::new(lsp));
    }

    /// Find the parser that handles the given file extension.
    /// Returns `None` if no parser is registered for that extension.
    pub fn parser_for_extension(&self, ext: &str) -> Option<Arc<dyn LanguageParser>> {
        self.parsers
            .iter()
            .find(|parser| parser.file_extensions().contains(&ext))
            .cloned()
    }

    /// Find the LSP provider that handles the given file extension.
    /// Returns `None` if no LSP provider is registered for that extension.
    pub fn lsp_for_extension(&self, ext: &str) -> Option<Arc<dyn LanguageLsp>> {
        self.lsp_providers
            .iter()
            .find(|lsp| lsp.file_extensions().contains(&ext))
            .cloned()
    }

    /// All registered LSP providers. Used to initialize LSP clients at startup
    // TODO Revisar esto porque no termina de convencerme
    pub fn lsp_providers(&self) -> &[Arc<dyn LanguageLsp>] {
        &self.lsp_providers
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parser_lookup_by_extension() {
        let reg = LanguageRegistry::with_builtins();

        assert!(reg.parser_for_extension("rs").is_some());
        assert!(reg.parser_for_extension("cpp").is_some());
        assert!(reg.parser_for_extension("cc").is_some());
        assert!(reg.parser_for_extension("h").is_some());
    }

    #[test]
    fn parser_returns_none_for_unknown_extension() {
        let reg = LanguageRegistry::with_builtins();

        assert!(reg.parser_for_extension("py").is_none());
        assert!(reg.parser_for_extension("go").is_none());
    }

    #[test]
    fn lsp_lookup_by_extension() {
        let reg = LanguageRegistry::with_builtins();
        assert!(reg.lsp_for_extension("rs").is_some());
    }

    #[test]
    fn lsp_returns_none_for_cpp() {
        let reg = LanguageRegistry::with_builtins();
        // C++ has parser but no LSP registered
        assert!(reg.lsp_for_extension("cpp").is_none());
    }

    #[test]
    fn lsp_returns_none_for_unknown() {
        let reg = LanguageRegistry::with_builtins();
        assert!(reg.lsp_for_extension("py").is_none());
    }

    #[test]
    fn lsp_providers_list() {
        let reg = LanguageRegistry::with_builtins();
        assert_eq!(reg.lsp_providers().len(), 1);
        assert_eq!(reg.lsp_providers()[0].language_id(), "rust");
    }

    #[test]
    fn empty_registry_returns_none() {
        let reg = LanguageRegistry::new();
        assert!(reg.parser_for_extension("rs").is_none());
        assert!(reg.lsp_for_extension("rs").is_none());
    }
}
