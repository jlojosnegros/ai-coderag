use std::path::PathBuf;

use lsp_types::{CallHierarchyIncomingCall, CallHierarchyItem, CallHierarchyOutgoingCall, Diagnostic, Location};
use tokio::sync::oneshot;

use crate::{Result, lsp::DocumentSymbol};


/// A command sent from an LspHandle to the LspActor.
///
/// Each variant carries the parameters for one LSP operacion and
/// a oneshot sender to return the result.
pub(crate) enum LspCommand {
    DocumentSymbols {
        path: PathBuf,
        reply: oneshot::Sender<Result<Vec<DocumentSymbol>>>,
    },
    GotoDefinition {
        path: PathBuf,
        line: u32,
        character: u32,
        reply: oneshot::Sender<Result<Vec<Location>>>,
    },
    References {
        path: PathBuf,
        line: u32,
        character: u32,
        reply: oneshot::Sender<Result<Vec<Location>>>,
    },
    PrepareCallHierarchy {
        path: PathBuf,
        line: u32,
        character: u32,
        reply: oneshot::Sender<Result<Vec<CallHierarchyItem>>>,
    },
    IncomingCalls {
        item: CallHierarchyItem,
        reply: oneshot::Sender<Result<Vec<CallHierarchyIncomingCall>>>,
    },
    OutgoingCalls {
        item: CallHierarchyItem,
        reply: oneshot::Sender<Result<Vec<CallHierarchyOutgoingCall>>>,
    },
    Diagnostics {
        path: PathBuf,
        reply: oneshot::Sender<Result<Vec<Diagnostic>>>,
    },
    Shutdown,
}
