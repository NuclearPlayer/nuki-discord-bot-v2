# AGENTS.md

Nuki is a Rust Discord bot for the server of the Nuclear music player. `README.md` tells what Nuki does. `src/main.rs` calls `nuki::run()` in `src/lib.rs`. `run()` loads `Config`, makes the `App` (`src/app.rs`), starts the background tasks, and starts the Serenity client.

## From a message to a reply

1. `Handler::message` (`src/discord/handler.rs`) discards the messages from bots, the system messages, and the direct messages.
2. `Debouncer` (`src/routing/debounce.rs`) collects the messages of one author in one channel into one unit. `process_unit` (`src/discord/pipeline.rs`) processes each unit in its own task.
3. `trigger_of_unit` (`src/routing/trigger.rs`) gives the trigger: `Direct` (mention, managed role, reply to Nuki) or `Unsolicited`.
4. `fetch_unit_context` (`src/discord/history.rs`) gets the history. In a thread, it adds the starter message and the parent channel's messages.
5. `Classifier::classify` (`src/routing/classify.rs`) asks Jev for the category.
6. `decide_with_cooldown` (`src/routing/dispatch.rs`) applies the thresholds and the chat cooldown to an `Unsolicited` unit.
7. `run_category` (`src/discord/pipeline.rs`) runs `ChatWorkflow` (`src/workflows/chat.rs`) with the Help or the Chat workflow prompt.
8. `turn_context` (`src/discord/turn_context.rs`) makes the `TurnContext`. `llm_turn` (`src/workflows/mod.rs`) makes the history and the system prompt.
9. `LlmClient::run` (`src/llm/agent.rs`) runs the agent on OpenRouter. Its `run_script` tool (`src/llm/script_tool.rs`) runs JS in the QuickJS sandbox.
10. `short_reply` (`src/workflows/reply_length.rs`) does the length check.
11. `apply_outcome` (`src/discord/reply.rs`) sends the reply and stores the tool turn in `tool_turns`.

## Design facts

- Jev only classifies a unit as `help`, `chat`, or `ignore`. Nuki decides in the Help workflow (`prompts/workflows/help.md`) if a problem is a bug, a feature request, or a question.
- `reports.send` (`src/sandbox/ops/reports.rs`) sends the report to the admin log channel and opens a thread. If the run sent a report, Nuki's reply goes to that thread.
- `ExamplesHook` (`src/llm/examples_hook.rs`) adds `prompts/examples.md` to the last message of each model request. The examples are not in the system prompt or in `tool_turns`.
- Jev judges the length of each reply (`prompts/decisions/reply-length.md`). If the reply is too long, Nuki rewrites it in a request with no tools (`prompts/tools/too-long.md`).
- `src/prompts/files.rs` compiles each prompt file into the binary. For each new prompt file, add a constant there. The `## ` headings of `prompts/decisions/category.md` must agree with `Category::name` (`src/routing/category.rs`).
- The operation table in `src/sandbox/ops/` is the only source of the sandbox functions and of their TypeScript declarations. Do not write them by hand. To add an operation, copy an existing operation in the namespace file.
- If you change `WEB_TIMEOUT` (`src/app.rs`) or `SandboxLimits::production`, keep 4 × `WEB_TIMEOUT` below the sandbox timeout.
- If a step fails, `AdminLog::report_error` (`src/admin/log.rs`) sends the error to the admin log channel. Nuki sends nothing in the channel. If the reply text is empty, Nuki also sends nothing.
- State in SQLite: `DATA_DIR/nuki.sqlite` (`src/store/`), with the tables `settings`, `tool_turns`, and `events`. To change the schema, add a file in `migrations/` and an `M::up` line in `Store::migrated`.
- State on disk: the bare clones of the repositories in `DATA_DIR/repos/<owner>/<name>.git`.
- State in memory: the debounced messages, the chat cooldowns, and the active model. Nuki loses the debounced messages and the cooldowns when it stops.

## Modules

| Module | Contents |
|---|---|
| `src/config.rs` | `Config`, with the environment variables. Add new variables here and in `.env.template`. |
| `src/discord/` | The handler, the pipeline, `/change-model` (`commands.rs`), the history, the context for the LLM and for Jev (`context.rs`), the attachments, the authors, the reply sender. |
| `src/routing/` | The trigger rules, the debounce, the classification, the categories, the thresholds, the chat cooldown. |
| `src/decisions/` | The client of the Jev Decisions API on OpenRouter. |
| `src/llm/` | The agent and its limit of model calls, `run_script`, the hooks, the model list, the active model and `DEFAULT_MODEL_ID` (`selection.rs`). |
| `src/workflows/` | `TurnContext`, `Outcome`, `llm_turn`, the Chat workflow, the length check. |
| `src/prompts/`, `prompts/` | The prompt files, the system prompt builder, the parser of the Jev question file. |
| `src/sandbox/` | The QuickJS runtime and its limits, the operation table, the declarations, the operations in `ops/`. The `run_script` description is in `prompts/tools/run-script.md`. |
| `src/repos/` | The repository catalog (with `catalog` in `src/app.rs`), the clone and fetch manager, the `git` wrapper. |
| `src/github/` | The read-only GitHub client. |
| `src/web/` | The keyless web providers and `WebChain`. `web_chain` in `src/app.rs` sets their sequence. |
| `src/stats/` | The usage event types (`event.rs`) and the recorder. Record each new event with `RunStats::record`. |
| `src/admin/` | The report embeds and the error messages for the admin log channel. |
| `src/tasks.rs` | The background task that fetches the clones. |

## Verification

Before you report that a change is complete, run these commands. All three must pass. CI (`.github/workflows/ci.yml`) runs the same commands.

```sh
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
```

## Code rules

- Write clear code in a functional style. If an immutable abstraction is possible, use it.
- Do not write an `if` statement on one line. Do not put an `if` expression in the position of an argument.
- Give each module and each function one purpose. If a file has more than one purpose, divide the file.
- Add only the features, options, checks, and abstractions that the task tells you to add.

## Test rules

- Test the behavior, not the implementation. Examine the item that the test name tells, not for example the length of a list that contains it.
- Do not write defensive checks or null checks. Let the test fail when the code fails. Assert on the full value, not on a part of a known string.
- Do not call external APIs. Test the HTTP clients with `wiremock`. Load the recorded API responses in `tests/fixtures/` with `include_str!`.
- Test the git operations with the `git` CLI, on local repositories in a `tempfile` directory. Test the store with `Store::open_in_memory()`.
- Put the shared test helpers of a module in its `test_support` module, declared with `#[cfg(test)] pub(crate) mod test_support;`. Put the helpers that more than one module uses in `src/test_support.rs`.

## Comments and docs

Write comments, doc comments, and Markdown docs in ASD-STE100 Simplified Technical English. Load the `asd-ste100` skill for the full rules. These rules matter most:

- Give each sentence a real subject that does the action: a function, a type, Nuki, Discord, Jev, the user. Use the active voice.
- Start the doc of a function with a verb in the simple present: "Sends the reply." Start the doc of a type or a field with a noun phrase: "The thread of the report."
- Put a condition first, in an `If` or `When` clause. Do not write docs that only repeat the signature.
- Use the possessive when it is clear: "Nuki's message". Do not stack `of` phrases.
- Write one fact in each sentence. Keep each sentence below 25 words. Write a procedure as steps in the imperative.
- Do not write a behavior as a general rule about an indefinite item. Write "If the run fails, records the outcome `error`", not "A failed run records the outcome `error`".

## Commits

Write the commit message on one short line, with no body and no prefix. Example: "Send Nuki's reply to the report thread".
