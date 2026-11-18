//! Pure formatting functions for MCP tool responses
//!
//! Each funcion takes LSP result types and produces LLM-readable text.
//! No async, no side effects, no state.

use std::path::Path;

use lsp_types::{
    CallHierarchyIncomingCall, CallHierarchyOutgoingCall, Diagnostic, DocumentSymbol, Location, SymbolKind, Uri,
};

pub fn format_outline(symbols: &[DocumentSymbol], path: &Path, depth: Option<u32>) -> String {
    let mut out = format!("{}:\n", path.display());
    format_symbols_recursive(symbols, 0, depth, &mut out);
    out
}

fn format_symbols_recursive(symbols: &[DocumentSymbol], current_depth: u32, max_depth: Option<u32>, out: &mut String) {
    if let Some(max) = max_depth {
        if current_depth > max {
            return;
        }
    }

    for sym in symbols {
        out.push_str(&format_symbol_entry(sym, current_depth));

        if let Some(children) = &sym.children {
            format_symbols_recursive(children, current_depth + 1, max_depth, out);
        }
    }
}

fn format_symbol_entry(sym: &DocumentSymbol, depth: u32) -> String {
    let indent = "  ".repeat(depth as usize + 1);
    let kind = format!("{:?}", sym.kind);
    let start = sym.range.start.line + 1;
    let end = sym.range.end.line + 1;
    format!("{indent}{} ({kind}) L{start}-L{end}\n", sym.name)
}

pub fn format_locations(locations: &[Location], project_root: &Path) -> String {
    if locations.is_empty() {
        return "No results found.".into();
    }

    let mut out = String::new();
    for loc in locations {
        out.push_str(&format_location_entry(loc, project_root));
    }
    out
}

fn format_location_entry(loc: &Location, project_root: &Path) -> String {
    let path = uri_to_relative_path(&loc.uri, project_root);
    let line = loc.range.start.line + 1;
    let col = loc.range.start.character + 1;

    format!("{path}:{line}:{col}\n")
}

fn uri_to_relative_path(uri: &Uri, project_root: &Path) -> String {
    let Ok(url) = url::Url::parse(uri.as_str()) else {
        return uri.as_str().to_string();
    };

    let Ok(abs) = url.to_file_path() else {
        return uri.as_str().to_string();
    };

    abs.strip_prefix(project_root).unwrap_or(&abs).display().to_string()
}

pub fn format_incoming_calls(calls: &[CallHierarchyIncomingCall], target_name: &str, project_root: &Path) -> String {
    if calls.is_empty() {
        return format!("No incoming calls to `{target_name}`.");
    }

    let mut out = format!("Incoming calls to `{target_name}`:\n");
    for call in calls {
        out.push_str(&format_incoming_call_entry(call, project_root));
    }

    out
}

fn format_incoming_call_entry(call: &CallHierarchyIncomingCall, project_root: &Path) -> String {
    let path = uri_to_relative_path(&call.from.uri, project_root);
    let line = call.from.selection_range.start.line + 1;

    format!("  {} ({path}:{line})\n", call.from.name)
}

pub fn format_outgoing_calls(calls: &[CallHierarchyOutgoingCall], source_name: &str, project_root: &Path) -> String {
    if calls.is_empty() {
        return format!("No outgoing calls from `{source_name}`.");
    }

    let mut out = format!("Outgoing calls from `{source_name}`: \n");
    for call in calls {
        out.push_str(&format_outgoing_call_entry(call, project_root));
    }
    out
}

fn format_outgoing_call_entry(call: &CallHierarchyOutgoingCall, project_root: &Path) -> String {
    let path = uri_to_relative_path(&call.to.uri, project_root);
    let line = call.to.selection_range.start.line + 1;

    format!("  {} ({path}:{line})\n", call.to.name)
}

pub fn format_diagnostics(diagnostics: &[Diagnostic], path: &Path) -> String {
    if diagnostics.is_empty() {
        return format!("No diagnostics for {}.", path.display());
    }

    let mut out = format!("{}:\n", path.display());
    for diag in diagnostics {
        out.push_str(&format_diagnostic_entry(diag));
    }

    out
}

fn format_diagnostic_entry(diag: &Diagnostic) -> String {
    let line = diag.range.start.line + 1;
    let severity = diag.severity.map_or("diagnostic".to_string(), |s| {
        let label = format!("{s:?}");
        if label.contains('(') {
            "unknown diagnostic".to_string()
        } else {
            label.to_lowercase()
        }
    });

    format!("  L{line} [{severity}] {}\n", diag.message)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use lsp_types::{
        CallHierarchyIncomingCall, CallHierarchyItem, Diagnostic, DiagnosticSeverity, DocumentSymbol, Location,
        Position, Range, SymbolKind, Uri,
    };

    use crate::mcp::format::{
        format_diagnostic_entry, format_diagnostics, format_incoming_calls, format_locations, format_outgoing_calls,
        format_outline, uri_to_relative_path,
    };

    fn make_range(start_line: u32, end_line: u32) -> Range {
        Range {
            start: Position {
                line: start_line,
                character: 0,
            },
            end: Position {
                line: end_line,
                character: 0,
            },
        }
    }

    fn make_symbol(
        name: &str,
        kind: SymbolKind,
        range: Range,
        children: Option<Vec<DocumentSymbol>>,
    ) -> DocumentSymbol {
        #[allow(deprecated)]
        DocumentSymbol {
            name: name.into(),
            detail: None,
            kind,
            tags: None,
            deprecated: None,
            range,
            selection_range: range,
            children,
        }
    }

    #[test]
    fn outline_empty_file() {
        let result = format_outline(&[], Path::new("src/empty.rs"), None);
        assert_eq!(result, "src/empty.rs:\n");
    }

    #[test]
    fn outline_flat_symbols() {
        let symbols = vec![
            make_symbol("main", SymbolKind::FUNCTION, make_range(0, 10), None),
            make_symbol("Config", SymbolKind::STRUCT, make_range(12, 20), None),
        ];

        let result = format_outline(&symbols, Path::new("src/main.rs"), None);
        assert_eq!(
            result,
            "src/main.rs:\n\
            \x20\x20main (Function) L1-L11\n\
            \x20\x20Config (Struct) L13-L21\n"
        );
    }

    #[test]
    fn outline_nested_symbols() {
        let methods = vec![
            make_symbol("new", SymbolKind::METHOD, make_range(14, 16), None),
            make_symbol("load", SymbolKind::METHOD, make_range(17, 19), None),
        ];

        let symbols = vec![make_symbol(
            "Config",
            SymbolKind::STRUCT,
            make_range(12, 20),
            Some(methods),
        )];

        let result = format_outline(&symbols, Path::new("src/lib.rs"), None);
        assert_eq!(
            result,
            "src/lib.rs:\n\
             \x20\x20Config (Struct) L13-L21\n\
             \x20\x20\x20\x20new (Method) L15-L17\n\
             \x20\x20\x20\x20load (Method) L18-L20\n"
        );
    }

    #[test]
    fn outline_depth_zero_shows_top_level_only() {
        let methods = vec![make_symbol("new", SymbolKind::METHOD, make_range(14, 16), None)];

        let symbols = vec![make_symbol(
            "Config",
            SymbolKind::STRUCT,
            make_range(12, 20),
            Some(methods),
        )];

        let result = format_outline(&symbols, Path::new("src/lib.rs"), Some(0));
        assert_eq!(
            result,
            "src/lib.rs:\n\
             \x20\x20Config (Struct) L13-L21\n"
        );
    }

    #[test]
    fn symbol_kind_debug_procudes_pascal_case() {
        assert_eq!(format!("{:?}", SymbolKind::FUNCTION), "Function");
        assert_eq!(format!("{:?}", SymbolKind::TYPE_PARAMETER), "TypeParameter");
    }

    #[test]
    fn locations_empty() {
        let result = format_locations(&[], Path::new("/project"));
        assert_eq!(result, "No results found.");
    }

    #[test]
    fn locations_relative_to_project_root() {
        let locations = vec![Location {
            uri: "file:///project/src/main.rs".parse().unwrap(),
            range: make_range(41, 41),
        }];

        let result = format_locations(&locations, Path::new("/project"));
        assert_eq!(result, "src/main.rs:42:1\n");
    }

    #[test]
    fn uri_strip_project_prefix() {
        let uri: Uri = "file:///home/user/project/src/lib.rs".parse().unwrap();
        let result = uri_to_relative_path(&uri, Path::new("/home/user/project"));
        assert_eq!(result, "src/lib.rs");
    }

    #[test]
    fn uri_outside_project_returns_absolute() {
        let uri: Uri = "file:///other/place/lib.rs".parse().unwrap();
        let result = uri_to_relative_path(&uri, Path::new("/home/user/project"));
        assert_eq!(result, "/other/place/lib.rs");
    }

    #[test]
    fn uri_invalid_returns_raw_string() {
        let uri: Uri = "not-an-uri".parse().unwrap();
        let result = uri_to_relative_path(&uri, Path::new("/project"));
        assert_eq!(result, "not-an-uri");
    }

    #[test]
    fn incoming_calls_empty() {
        let result = format_incoming_calls(&[], "process", Path::new("/project"));
        assert_eq!(result, "No incoming calls to `process`.");
    }

    #[test]
    fn incoming_calls_formatted() {
        let calls = vec![CallHierarchyIncomingCall {
            from: CallHierarchyItem {
                name: "main".into(),
                kind: SymbolKind::FUNCTION,
                tags: None,
                detail: None,
                uri: "file:///project/src/main.rs".parse().unwrap(),
                range: make_range(0, 20),
                selection_range: make_range(0, 0),
                data: None,
            },
            from_ranges: vec![make_range(14, 14)],
        }];

        let result = format_incoming_calls(&calls, "process", Path::new("/project"));
        assert_eq!(result, "Incoming calls to `process`:\n  main (src/main.rs:1)\n");
    }

    #[test]
    fn outgoing_calls_empty() {
        let result = format_outgoing_calls(&[], "main", Path::new("/project"));
        assert_eq!(result, "No outgoing calls from `main`.");
    }

    #[test]
    fn diagnostics_empty() {
        let result = format_diagnostics(&[], Path::new("src/lib.rs"));
        assert_eq!(result, "No diagnostics for src/lib.rs.");
    }

    #[test]
    fn diagnostics_with_severity_levels() {
        let diags = vec![
            Diagnostic {
                range: make_range(11, 11),
                severity: Some(DiagnosticSeverity::ERROR),
                message: "type mismatch".into(),
                ..Default::default()
            },
            Diagnostic {
                range: make_range(22, 22),
                severity: Some(DiagnosticSeverity::WARNING),
                message: "unused variable".into(),
                ..Default::default()
            },
            Diagnostic {
                range: make_range(33, 33),
                severity: Some(DiagnosticSeverity::INFORMATION),
                message: "empty line".into(),
                ..Default::default()
            },
            Diagnostic {
                range: make_range(44, 44),
                severity: Some(DiagnosticSeverity::HINT),
                message: "change type".into(),
                ..Default::default()
            },
        ];

        let result = format_diagnostics(&diags, Path::new("src/main.rs"));
        assert_eq!(
            result,
            "src/main.rs:\n\
            \x20\x20L12 [error] type mismatch\n\
            \x20\x20L23 [warning] unused variable\n\
            \x20\x20L34 [information] empty line\n\
            \x20\x20L45 [hint] change type\n"
        );
    }

    #[test]
    fn diagnostics_severity_known_values() {
        let make = |severity| {
            let diag = Diagnostic {
                range: make_range(0, 0),
                severity,
                message: "msg".into(),
                ..Default::default()
            };
            format_diagnostic_entry(&diag)
        };

        assert!(make(Some(DiagnosticSeverity::ERROR)).contains("[error]"));
        assert!(make(Some(DiagnosticSeverity::WARNING)).contains("[warning]"));
        assert!(make(Some(DiagnosticSeverity::INFORMATION)).contains("[information]"));
        assert!(make(Some(DiagnosticSeverity::HINT)).contains("[hint]"));
        assert!(make(None).contains("[diagnostic]"));
    }
}
