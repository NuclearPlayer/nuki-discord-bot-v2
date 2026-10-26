use serde_json::Value;
use wiremock::matchers::any;
use wiremock::{Mock, MockServer, ResponseTemplate};

use crate::sandbox::operation::{Operation, OperationTable};
use crate::sandbox::runtime::ScriptOutcome;

/// Makes the test fail if `server` receives a request.
pub(crate) async fn expect_no_request(server: &MockServer) {
    Mock::given(any())
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(server)
        .await;
}

/// Returns a table with `test.echo` as its only operation.
pub(crate) fn echo_table() -> OperationTable {
    OperationTable::new(vec![echo_operation()])
}

/// Returns `test.echo`, an operation that returns its parameters.
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

/// Returns the outcome of a script that returns `expected` and logs nothing.
pub(crate) fn value(expected: Value) -> ScriptOutcome {
    ScriptOutcome {
        value: Some(expected),
        logs: vec![],
        error: None,
    }
}
