use std::sync::Arc;

use rig_core::message::ToolName;
use rig_core::tool::{DynamicTool, ToolExecutionError, ToolOutput};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::sandbox::declarations::tool_description;
use crate::sandbox::runtime::Sandbox;

const RUN_SCRIPT: &str = "run_script";

/// Makes the `run_script` tool, which runs the code of the model in `sandbox`.
pub fn run_script_tool(sandbox: Arc<Sandbox>) -> DynamicTool {
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
        Box::pin(async move { run_script(&sandbox, args).await })
    })
}

#[derive(Deserialize)]
struct ScriptArgs {
    code: String,
}

/// Runs the script of a tool call. A script error goes back to the model in the result, not as a tool error.
async fn run_script(sandbox: &Sandbox, args: Value) -> Result<ToolOutput, ToolExecutionError> {
    let ScriptArgs { code } = serde_json::from_value(args)
        .map_err(|error| ToolExecutionError::invalid_args(error.to_string()))?;
    let outcome = sandbox.run(&code).await;
    Ok(ToolOutput::text(
        outcome.to_tool_result(sandbox.limits().max_output_bytes),
    ))
}
