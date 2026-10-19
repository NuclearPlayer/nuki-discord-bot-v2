use serde_json::Value;

use crate::sandbox::operation::{Operation, OperationTable};
use crate::sandbox::runtime::ScriptOutcome;

/// A table with `test.echo` as its only operation.
pub(crate) fn echo_table() -> OperationTable {
    OperationTable::new(vec![echo_operation()])
}

/// `test.echo`, an operation that returns its parameters.
pub(crate) fn echo_operation() -> Operation {
    Operation::typed(
        "test",
        "echo",
        "Returns its parameters.",
        vec![],
        "unknown",
        |params: Value| async { Ok::<_, String>(params) },
    )
}

/// The outcome of a script that returns `expected` and logs nothing.
pub(crate) fn value(expected: Value) -> ScriptOutcome {
    ScriptOutcome {
        value: Some(expected),
        logs: vec![],
        error: None,
    }
}
