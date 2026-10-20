use std::sync::Arc;
use std::time::Instant;

use rig_core::message::ToolName;
use rig_core::tool::{DynamicTool, ToolExecutionError, ToolOutput};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::sandbox::declarations::tool_description;
use crate::sandbox::runtime::{Sandbox, ScriptRun, truncate_tool_result};
use crate::stats::duration_ms;
use crate::stats::event::{Event, ToolCall};
use crate::stats::recorder::RunStats;

const RUN_SCRIPT: &str = "run_script";

/// Makes the `run_script` tool. The tool runs the model's code in `sandbox`, and records each
/// script as a `tool.call` event.
pub fn run_script_tool(sandbox: Arc<Sandbox>, stats: RunStats) -> DynamicTool {
    let name = ToolName::new(RUN_SCRIPT).expect("the tool name is not empty");
    let description = tool_description(sandbox.table());
    let parameters = json!({
        "type": "object",
        "properties": {
            "code": { "type": "string", "description": "The body of an async JS function." }
        },
        "required": ["code"]
    });
    DynamicTool::new(name, description, parameters, move |args: Value| {
        let sandbox = Arc::clone(&sandbox);
        let stats = stats.clone();
        Box::pin(async move { run_script(&sandbox, &stats, args).await })
    })
}

#[derive(Deserialize)]
struct ScriptArgs {
    code: String,
}

/// Runs the script of a tool call. If the script fails, gives the error to the model in the
/// result, not as a tool error. If the arguments do not parse, returns a tool error and runs no
/// script.
async fn run_script(
    sandbox: &Sandbox,
    stats: &RunStats,
    args: Value,
) -> Result<ToolOutput, ToolExecutionError> {
    let code = match serde_json::from_value::<ScriptArgs>(args) {
        Ok(ScriptArgs { code }) => code,
        Err(error) => {
            let message = error.to_string();
            let call = invalid_arguments_call(message.clone());
            stats.record(None, Event::ToolCall(call)).await;
            return Err(ToolExecutionError::invalid_args(message));
        }
    };
    let started = Instant::now();
    let ScriptRun {
        outcome,
        operations,
    } = sandbox.run(&code).await;
    let elapsed = started.elapsed();
    let json = outcome.to_json();
    let result_bytes = json.len() as u64;
    let result = truncate_tool_result(json, sandbox.limits().max_output_bytes);
    let call = ToolCall {
        code,
        operations,
        duration_ms: duration_ms(elapsed),
        error: outcome.error.map(|error| error.message),
        result: result.clone(),
        result_bytes,
    };
    stats.record(None, Event::ToolCall(call)).await;
    Ok(ToolOutput::text(result))
}

/// Returns the `tool.call` for a call with arguments that do not parse. No code ran, and no
/// result exists.
fn invalid_arguments_call(error: String) -> ToolCall {
    ToolCall {
        code: String::new(),
        operations: vec![],
        duration_ms: 0,
        error: Some(error),
        result: String::new(),
        result_bytes: 0,
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use chrono::Utc;
    use serde_json::json;

    use super::*;
    use crate::sandbox::operation::{Operation, OperationTable};
    use crate::sandbox::runtime::SandboxLimits;
    use crate::sandbox::test_support::echo_table;
    use crate::stats::test_support::{duration_of, events_between, run_event, test_run};

    fn sandbox() -> Sandbox {
        Sandbox::new(Arc::new(echo_table()), SandboxLimits::production())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_script_that_throws_records_its_error() {
        let (store, stats) = test_run().await;
        let code = "throw new Error('bad input');";
        let before = Utc::now();

        run_script(&sandbox(), &stats, json!({ "code": code }))
            .await
            .unwrap();

        let after = Utc::now();
        let result = r#"{"error":{"message":"bad input","stack":"    at <anonymous> (eval_script:2:11)\n    at <eval> (eval_script:3:1)\n"}}"#;
        assert_eq!(
            events_between(&store, before, after).await,
            vec![run_event(
                "tool.call",
                json!({
                    "code": code,
                    "operations": [],
                    "error": "bad input",
                    "result": result,
                    "result_bytes": result.len(),
                })
            )]
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_tool_call_records_the_duration_of_the_script() {
        let (store, stats) = test_run().await;
        let slow = Operation::typed(
            "test",
            "slow",
            "Waits 50 ms.",
            vec![],
            "null",
            |_: Value| async {
                tokio::time::sleep(Duration::from_millis(50)).await;
                Ok::<_, String>(Value::Null)
            },
        );
        let sandbox = Sandbox::new(
            Arc::new(OperationTable::new(vec![slow])),
            SandboxLimits::production(),
        );

        run_script(&sandbox, &stats, json!({ "code": "await test.slow();" }))
            .await
            .unwrap();

        assert!(duration_of(&store, "tool.call").await >= 50);
    }
}
