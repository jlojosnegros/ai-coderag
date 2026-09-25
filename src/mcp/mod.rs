use std::sync::Arc;

use rmcp::tool_router;

use crate::{CoderagConfig, registry::LanguageRegistry};

mod response;

/// Central MCP server struct
///
/// Every mcp tool handler is a method of this struct and accesses shared state
/// via `&self`.
/// The `#[tool_router]` macro generates dispatch code that routes incoming `tools/call`
/// requests to the right method.
pub struct CoderagServer {
    // TODO: temporal delete
    #[allow(dead_code)]
    config: CoderagConfig,
    // TODO: temporal delete
    #[allow(dead_code)]
    registry: Arc<LanguageRegistry>,
}

impl CoderagServer {
    pub fn new(config: CoderagConfig) -> Self {
        let registry = Arc::new(LanguageRegistry::with_builtins());
        Self { config, registry }
    }
}

// --- Tool routing ---
//
// `#[tool_router(server_handler)]` does three things:
// 1. For each `#[rmcp::tool]` method, it registers the method name as a tool name and generates the JSON schema from
//    method's Parameters<T> type.
// 2. It generates a `tools/list` response listing all registered tools.
// 3. It implements rmcp's `ServerHandler` trait, which rmcp calls when a `tool/call` request arrives from the client.
//
// IMPORTANT: the tool attribute must use the full path `#[rmcp::tool]`, not just
// `#[tool]`. The macro is re-exported by rmcp but the short form is not available
// inside a `#[tool_router]` block
//
// All tools in coderag live in this single impl block. rmcp does not support
// multiple `#[tool_router]` blocks on the same struct. The tools themselves
// are thin (2 -5 lines each) because they delegate to business logic in other modules
// (lps/, parser/, embed/, store/)


#[tool_router(server_handler)]
impl CoderagServer {
    /// Return server version
    ///
    /// This is a minimal tool to verify the MCP infra works end-to-end.
    /// An Agent can call it to confirm the server is running and check its version.
    /// It returns a plan String, which rmcp wraps into `CallToolResult` with
    /// a text content block automatically
    #[rmcp::tool(description = "Return coderag server version")]
    fn version(&self) -> String {
        format!("coderag {}", env!("CARGO_PKG_VERSION"))
    }
}


/// Convert any displayable error into a tool-level error response.
///
/// Use this in tool handlers when the tool ran but the underlying operation
/// failed. Example
/// ```ignore
/// match self.store.search(&query).await {
///     Ok(results) => Ok(build_success_response(results)),
///     Err(err) => Ok(tool_error(err)),
/// }
/// ```
///
/// The outer Result is Ok because the tool DID run ( it was found, params were valid).
/// The CallToolResult carries `is_error: true` to tell the client that the
/// operation itself failed.
pub fn tool_error(err: impl std::fmt::Display) -> rmcp::model::CallToolResult {
    rmcp::model::CallToolResult::error(vec![rmcp::model::ContentBlock::text(err.to_string())])
}


#[cfg(test)]
mod tests {
    use crate::{
        CoderagConfig,
        mcp::{CoderagServer, tool_error},
    };

    #[test]
    fn server_new_creates_successfully() {
        // Verifies that the constructor initializes without panic.
        // If LanguageRegistry::with_builtins() fails, this test catches it.
        let _server = CoderagServer::new(CoderagConfig::default());
    }

    #[test]
    fn version_tool_returns_package_version() {
        let server = CoderagServer::new(CoderagConfig::default());
        let result = server.version();
        assert!(
            result.starts_with("coderag "),
            "Expected 'coderag ...' but got: {result}"
        );
    }

    #[test]
    fn tool_error_sets_is_error_flag() {
        let result = tool_error("connection refused");
        assert_eq!(result.is_error, Some(true))
    }

    #[test]
    fn tool_error_includes_message_in_content() {
        let result = tool_error("disk full");
        assert_eq!(result.content.len(), 1);

        let text = serde_json::to_string(&result.content[0]).unwrap();
        assert!(
            text.contains("disk full"),
            "Expected error message in content, got: {text}"
        );
    }
}
