use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Sender};
use std::time::{Duration, Instant};

use rquickjs::prelude::{Async, Rest};
use rquickjs::{
    AsyncContext, AsyncRuntime, CatchResultExt, CaughtError, Ctx, Exception, Function, Object,
    Promise, Value,
};
use serde::Serialize;

use super::declarations::prelude;
use super::operation::OperationTable;

pub struct SandboxLimits {
    pub timeout: Duration,
    pub max_output_bytes: usize,
    pub memory_limit_bytes: usize,
}

impl SandboxLimits {
    pub fn production() -> Self {
        Self {
            timeout: Duration::from_secs(10),
            max_output_bytes: 20_000,
            memory_limit_bytes: 64 * 1024 * 1024,
        }
    }
}

#[derive(Debug, PartialEq, Serialize)]
pub struct ScriptError {
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stack: Option<String>,
}

#[derive(Debug, PartialEq, Serialize)]
pub struct ScriptOutcome {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub logs: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<ScriptError>,
}

impl ScriptOutcome {
    fn new(result: Result<Option<serde_json::Value>, ScriptError>, logs: Vec<String>) -> Self {
        match result {
            Ok(value) => Self {
                value,
                logs,
                error: None,
            },
            Err(error) => Self {
                value: None,
                logs,
                error: Some(error),
            },
        }
    }

    /// Returns the outcome as JSON, before the output limit.
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("a script outcome always serializes")
    }
}

/// A script run: its outcome, and the names of the operations that the script called, in
/// sequence. The model gets only the outcome. The usage events get both.
#[derive(Debug, PartialEq)]
pub struct ScriptRun {
    pub outcome: ScriptOutcome,
    pub operations: Vec<String>,
}

/// Truncates the JSON of an outcome to at most `max_output_bytes`, for the tool result.
pub fn truncate_tool_result(json: String, max_output_bytes: usize) -> String {
    if json.len() > max_output_bytes {
        format!(
            "{}\n[truncated]",
            &json[..json.floor_char_boundary(max_output_bytes)]
        )
    } else {
        json
    }
}

pub struct Sandbox {
    table: Arc<OperationTable>,
    prelude: String,
    limits: SandboxLimits,
}

impl Sandbox {
    pub fn new(table: Arc<OperationTable>, limits: SandboxLimits) -> Self {
        let prelude = prelude(&table);
        Self {
            table,
            prelude,
            limits,
        }
    }

    pub fn table(&self) -> &OperationTable {
        &self.table
    }

    pub fn limits(&self) -> &SandboxLimits {
        &self.limits
    }

    /// Runs `code` as the body of an async function in a new QuickJS context.
    pub async fn run(&self, code: &str) -> ScriptRun {
        let (log_sender, log_receiver) = mpsc::channel();
        let logs = LogSink::new(log_sender, self.limits.max_output_bytes);
        let (operation_sender, operation_receiver) = mpsc::channel();
        let result = self.evaluate(code, logs, operation_sender).await;
        ScriptRun {
            outcome: ScriptOutcome::new(result, log_receiver.try_iter().collect()),
            operations: operation_receiver.try_iter().collect(),
        }
    }

    async fn evaluate(
        &self,
        code: &str,
        logs: LogSink,
        operations: Sender<String>,
    ) -> Result<Option<serde_json::Value>, ScriptError> {
        let deadline = Instant::now() + self.limits.timeout;
        let runtime = AsyncRuntime::new()?;
        // The interrupt handler stops busy JS. The tokio timeout stops a script that waits for
        // an operation.
        runtime
            .set_interrupt_handler(Some(Box::new(move || Instant::now() >= deadline)))
            .await;
        runtime
            .set_memory_limit(self.limits.memory_limit_bytes)
            .await;
        let context = AsyncContext::full(&runtime).await?;
        let table = Arc::clone(&self.table);
        let prelude = self.prelude.as_str();
        let script = format!("(async () => {{\n{code}\n}})()");
        let globals = Globals {
            table,
            logs,
            operations,
        };
        let evaluation =
            context.async_with(async |ctx| evaluate_in(&ctx, globals, prelude, &script).await);
        match tokio::time::timeout_at(deadline.into(), evaluation).await {
            Ok(Err(_)) if Instant::now() >= deadline => Err(self.timeout_error()),
            Ok(result) => result,
            Err(_) => Err(self.timeout_error()),
        }
    }

    fn timeout_error(&self) -> ScriptError {
        ScriptError {
            message: format!(
                "Script timed out after {} seconds",
                self.limits.timeout.as_secs_f64()
            ),
            stack: None,
        }
    }
}

/// The data that the globals of a script use: the operations, the log sink, and the sink for the
/// names of the called operations.
struct Globals {
    table: Arc<OperationTable>,
    logs: LogSink,
    operations: Sender<String>,
}

async fn evaluate_in<'js>(
    ctx: &Ctx<'js>,
    globals: Globals,
    prelude: &str,
    script: &str,
) -> Result<Option<serde_json::Value>, ScriptError> {
    define_globals(ctx, globals)
        .and_then(|()| ctx.eval::<(), _>(prelude))
        .catch(ctx)
        .map_err(|caught| script_error(ctx, caught))?;
    let promise: Promise = ctx
        .eval(script)
        .catch(ctx)
        .map_err(|caught| script_error(ctx, caught))?;
    let value: Value = promise
        .into_future()
        .await
        .catch(ctx)
        .map_err(|caught| script_error(ctx, caught))?;
    to_json(ctx, value)
        .catch(ctx)
        .map_err(|caught| script_error(ctx, caught))
}

fn define_globals<'js>(ctx: &Ctx<'js>, globals: Globals) -> rquickjs::Result<()> {
    let console = Object::new(ctx.clone())?;
    console.set("log", console_log(ctx, globals.logs)?)?;
    ctx.globals().set("console", console)?;
    ctx.globals()
        .set("__op", op_function(ctx, globals.table, globals.operations)?)
}

/// Collects the `console.log` lines until their total size passes the output limit.
struct LogSink {
    sender: Sender<String>,
    max_bytes: usize,
    collected_bytes: AtomicUsize,
}

impl LogSink {
    fn new(sender: Sender<String>, max_bytes: usize) -> Self {
        Self {
            sender,
            max_bytes,
            collected_bytes: AtomicUsize::new(0),
        }
    }

    fn collect(&self, line: String) {
        let collected_before = self
            .collected_bytes
            .fetch_add(line.len(), Ordering::Relaxed);
        if collected_before <= self.max_bytes {
            self.sender
                .send(line)
                .expect("log receiver lives until run returns");
        }
    }
}

/// Makes `console.log`, which collects each call as one line.
fn console_log<'js>(ctx: &Ctx<'js>, logs: LogSink) -> rquickjs::Result<Function<'js>> {
    Function::new(
        ctx.clone(),
        move |ctx: Ctx<'js>, args: Rest<Value<'js>>| -> rquickjs::Result<()> {
            let parts = args
                .0
                .into_iter()
                .map(|arg| display_text(&ctx, arg))
                .collect::<rquickjs::Result<Vec<_>>>()?;
            logs.collect(parts.join(" "));
            Ok(())
        },
    )
}

/// Formats a value as text. Keeps a string as it is, and formats all other values as JSON.
fn display_text<'js>(ctx: &Ctx<'js>, arg: Value<'js>) -> rquickjs::Result<String> {
    match arg.as_string() {
        Some(text) => text.to_string(),
        None => ctx
            .json_stringify(arg)?
            .map_or(Ok("undefined".to_owned()), |json| json.to_string()),
    }
}

/// Makes `__op(name, json)`, which calls an operation and resolves to its result as JSON. The
/// list in `operations` also contains the calls that fail or name an unknown operation, because
/// `__op` sends each name to `operations` before the call.
fn op_function<'js>(
    ctx: &Ctx<'js>,
    table: Arc<OperationTable>,
    operations: Sender<String>,
) -> rquickjs::Result<Function<'js>> {
    Function::new(
        ctx.clone(),
        Async(move |ctx: Ctx<'js>, name: String, json: String| {
            let table = Arc::clone(&table);
            operations
                .send(name.clone())
                .expect("operation receiver lives until run returns");
            async move {
                call_operation(&table, &name, &json)
                    .await
                    .map_err(|message| Exception::throw_message(&ctx, &message))
            }
        }),
    )
}

async fn call_operation(table: &OperationTable, name: &str, json: &str) -> Result<String, String> {
    tracing::info!("{name}({json})");
    let operation = table
        .find(name)
        .ok_or_else(|| format!("unknown operation: {name}"))?;
    let params = serde_json::from_str(json).map_err(|error| error.to_string())?;
    let result = (operation.handler)(params).await?;
    Ok(result.to_string())
}

fn to_json<'js>(ctx: &Ctx<'js>, value: Value<'js>) -> rquickjs::Result<Option<serde_json::Value>> {
    ctx.json_stringify(value)?
        .map(|json| {
            let text = json.to_string()?;
            Ok(serde_json::from_str(&text).expect("JSON.stringify gives valid JSON"))
        })
        .transpose()
}

fn script_error<'js>(ctx: &Ctx<'js>, caught: CaughtError<'js>) -> ScriptError {
    match caught {
        CaughtError::Exception(exception) => ScriptError {
            message: exception.message().unwrap_or_default(),
            stack: exception.stack(),
        },
        CaughtError::Value(value) => ScriptError {
            message: display_text(ctx, value).unwrap_or_else(|error| error.to_string()),
            stack: None,
        },
        other => ScriptError {
            message: other.to_string(),
            stack: None,
        },
    }
}

impl From<rquickjs::Error> for ScriptError {
    fn from(error: rquickjs::Error) -> Self {
        Self {
            message: error.to_string(),
            stack: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::*;
    use crate::sandbox::operation::Operation;
    use crate::sandbox::test_support::{echo_operation, value};

    fn test_table() -> OperationTable {
        OperationTable::new(vec![
            echo_operation(),
            Operation::typed(
                "test",
                "fail",
                "Fails.",
                vec![],
                "never",
                |_: Value| async { Err::<Value, _>("boom") },
            ),
            Operation::typed(
                "test",
                "slow",
                "Waits 5 seconds.",
                vec![],
                "null",
                |_: Value| async {
                    tokio::time::sleep(Duration::from_secs(5)).await;
                    Ok::<_, String>(Value::Null)
                },
            ),
        ])
    }

    fn test_limits() -> SandboxLimits {
        SandboxLimits {
            timeout: Duration::from_millis(300),
            max_output_bytes: 20_000,
            memory_limit_bytes: 64 * 1024 * 1024,
        }
    }

    fn sandbox() -> Arc<Sandbox> {
        sandbox_with(test_limits())
    }

    fn sandbox_with(limits: SandboxLimits) -> Arc<Sandbox> {
        Arc::new(Sandbox::new(Arc::new(test_table()), limits))
    }

    async fn run_script(sandbox: &Arc<Sandbox>, code: &str) -> ScriptRun {
        let sandbox = Arc::clone(sandbox);
        let code = code.to_owned();
        tokio::spawn(async move { sandbox.run(&code).await })
            .await
            .unwrap()
    }

    async fn run(sandbox: &Arc<Sandbox>, code: &str) -> ScriptOutcome {
        run_script(sandbox, code).await.outcome
    }

    fn timed_out() -> ScriptOutcome {
        ScriptOutcome {
            value: None,
            logs: vec![],
            error: Some(ScriptError {
                message: "Script timed out after 0.3 seconds".to_owned(),
                stack: None,
            }),
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn run_returns_the_value_of_the_script() {
        let outcome = run(&sandbox(), "return { answer: 42, list: [1, 'two'] };").await;

        assert_eq!(outcome, value(json!({ "answer": 42, "list": [1, "two"] })));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn run_collects_console_log_lines() {
        let outcome = run(
            &sandbox(),
            "console.log('hello', 'world');\nconsole.log('count', 3, { a: [true] });",
        )
        .await;

        assert_eq!(
            outcome,
            ScriptOutcome {
                value: None,
                logs: vec![
                    "hello world".to_owned(),
                    r#"count 3 {"a":[true]}"#.to_owned()
                ],
                error: None,
            }
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn run_reports_a_thrown_error_with_its_message_and_stack() {
        let outcome = run(&sandbox(), "throw new Error('bad input');").await;

        assert_eq!(
            outcome,
            ScriptOutcome {
                value: None,
                logs: vec![],
                error: Some(ScriptError {
                    message: "bad input".to_owned(),
                    stack: Some(
                        "    at <anonymous> (eval_script:2:11)\n    at <eval> (eval_script:3:1)\n"
                            .to_owned(),
                    ),
                }),
            }
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn run_reports_a_thrown_string() {
        let outcome = run(&sandbox(), "throw 'not found';").await;

        assert_eq!(
            outcome,
            ScriptOutcome {
                value: None,
                logs: vec![],
                error: Some(ScriptError {
                    message: "not found".to_owned(),
                    stack: None,
                }),
            }
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn run_stops_a_script_that_exceeds_the_memory_limit() {
        let sandbox = sandbox_with(SandboxLimits {
            memory_limit_bytes: 4 * 1024 * 1024,
            ..test_limits()
        });

        let outcome = run(
            &sandbox,
            "const chunks = [];\nwhile (true) { chunks.push('x'.repeat(100000)); }",
        )
        .await;

        assert_eq!(
            outcome,
            ScriptOutcome {
                value: None,
                logs: vec![],
                error: Some(ScriptError {
                    message: "out of memory".to_owned(),
                    stack: Some(
                        "    at <anonymous> (eval_script:3:32)\n    at <eval> (eval_script:4:1)\n"
                            .to_owned(),
                    ),
                }),
            }
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn run_stops_collecting_logs_after_the_output_limit() {
        let sandbox = sandbox_with(SandboxLimits {
            max_output_bytes: 8,
            ..test_limits()
        });

        let outcome = run(
            &sandbox,
            "console.log('12345');\nconsole.log('67890');\nconsole.log('abc');",
        )
        .await;

        assert_eq!(
            outcome,
            ScriptOutcome {
                value: None,
                logs: vec!["12345".to_owned(), "67890".to_owned()],
                error: None,
            }
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn run_calls_an_operation_with_its_params() {
        let outcome = run(
            &sandbox(),
            "return await test.echo({ text: 'hi', count: 2 });",
        )
        .await;

        assert_eq!(outcome, value(json!({ "text": "hi", "count": 2 })));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn an_operation_error_rejects_with_its_message() {
        let outcome = run(
            &sandbox(),
            "try { await test.fail(); } catch (e) { return e.message; }",
        )
        .await;

        assert_eq!(outcome, value(json!("boom")));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn an_undefined_global_is_a_reference_error() {
        let outcome = run(
            &sandbox(),
            "try { client.channels; } catch (e) { return `${e.name}: ${e.message}`; }",
        )
        .await;

        assert_eq!(
            outcome,
            value(json!("ReferenceError: client is not defined"))
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn run_stops_a_busy_loop_after_the_timeout() {
        let outcome = run(&sandbox(), "while (true) {}").await;

        assert_eq!(outcome, timed_out());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn run_stops_a_script_that_waits_on_a_slow_operation() {
        let outcome = run(&sandbox(), "return await test.slow();").await;

        assert_eq!(outcome, timed_out());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn each_run_uses_a_new_context() {
        let sandbox = sandbox();
        run(&sandbox, "globalThis.x = 1;").await;

        let outcome = run(&sandbox, "return typeof x;").await;

        assert_eq!(outcome, value(json!("undefined")));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn an_unknown_operation_rejects_with_its_name() {
        let outcome = run(
            &sandbox(),
            "try { await __op('test.nope', '{}'); } catch (e) { return e.message; }",
        )
        .await;

        assert_eq!(outcome, value(json!("unknown operation: test.nope")));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn the_run_lists_the_called_operations() {
        let run = run_script(
            &sandbox(),
            "await test.echo({ a: 1 });\n\
             try { await test.fail(); } catch (e) {}\n\
             try { await __op('test.nope', '{}'); } catch (e) {}\n\
             return await test.echo({ b: 2 });",
        )
        .await;

        assert_eq!(
            run,
            ScriptRun {
                outcome: value(json!({ "b": 2 })),
                operations: vec![
                    "test.echo".to_owned(),
                    "test.fail".to_owned(),
                    "test.nope".to_owned(),
                    "test.echo".to_owned(),
                ],
            }
        );
    }

    #[test]
    fn to_json_omits_empty_parts() {
        let outcome = ScriptOutcome {
            value: None,
            logs: vec![],
            error: Some(ScriptError {
                message: "boom".to_owned(),
                stack: None,
            }),
        };

        assert_eq!(outcome.to_json(), r#"{"error":{"message":"boom"}}"#);
    }

    #[test]
    fn truncate_tool_result_truncates_long_output() {
        let json = value(json!("abcdef")).to_json();

        assert_eq!(
            truncate_tool_result(json, 12),
            "{\"value\":\"ab\n[truncated]"
        );
    }

    #[test]
    fn truncate_tool_result_cuts_at_a_character_boundary() {
        let json = value(json!("aé")).to_json();

        assert_eq!(
            truncate_tool_result(json, 12),
            "{\"value\":\"a\n[truncated]"
        );
    }
}
