# AGENTS.md

Nuki is a Rust Discord bot for the server of the Nuclear music player. `README.md` tells what Nuki does. `src/config.rs` and `.env.template` show the environment variables that Nuki reads. This file tells you how the parts of the code work together, and where to find each part.

## Architecture

`src/main.rs` calls `nuki::run()` in `src/lib.rs`. `run()` loads the configuration, makes the `App` with `App::build`, starts the background tasks, and starts the Serenity client. `App` (`src/app.rs`) holds the services that the handler, the pipeline, and the background tasks share.

### From a message to a reply

1. `Handler::message` (`src/discord/handler.rs`) receives each message. It discards the messages from bots, the system messages, and the direct messages. It gives all other messages to the `Debouncer`.
2. `Debouncer` (`src/routing/debounce.rs`) collects the messages of one author in one channel. When the author sends no message for `DEBOUNCE_SECONDS`, it sends the messages as one unit.
3. `spawn_unit_reader` (`src/discord/pipeline.rs`) starts `process_unit` in a new task for each unit. Each unit is one run, with one `RunStats`.
4. `trigger_of_unit` (`src/routing/trigger.rs`) gives the trigger of the unit:
   - `IntakeThread`: the unit is in the thread of an open intake. `route_in_intake` (`src/discord/pipeline.rs`) sends the unit to its intake. Refer to "Intakes".
   - `Direct`: a message mentions Nuki or Nuki's managed role, or replies to Nuki.
   - `Unsolicited`: all other units.
5. `fetch_unit_context` (`src/discord/history.rs`) gets the last `CONTEXT_MESSAGES` messages of the channel. The unit is at the end.
6. `Classifier::classify` (`src/routing/classify.rs`) sends the history and the unit to Jev with the question in `prompts/decisions/category.md`. Jev gives a probability for each `Category`. For a `Direct` unit, the question has no `ignore` category.
7. `decide_with_cooldown` (`src/routing/dispatch.rs`) gives the `Decision`. For a `Direct` unit, Nuki runs the top category. For an `Unsolicited` unit, Nuki skips the unit in these conditions:
   - The top category is `ignore`.
   - The probability of the top category is below its threshold, `THRESHOLD_HELP`, `THRESHOLD_BUG`, `THRESHOLD_FEATURE`, or `THRESHOLD_CHAT`.
   - The top category is `chat`, and the chat cooldown of the channel is active. If Nuki runs an unsolicited chat, the cooldown starts.
8. `run_category` (`src/discord/pipeline.rs`) starts the workflow of the category: `HelpWorkflow`, `ChatWorkflow`, or `start_intake` for a bug report or a feature request.
9. `turn_context` (`src/discord/turn_context.rs`) makes the `TurnContext`: the history, the stored tool turns, the runtime context, and the sandbox for the server.
10. The workflow calls `llm_turn` (`src/workflows/mod.rs`). `llm_turn` makes the LLM history with `build_history` (`src/discord/context.rs`) and the system prompt with `SystemPrompt::build` (`src/prompts/system.rs`).
11. `LlmClient::run` (`src/llm/agent.rs`) runs the agent on OpenRouter with the active model. The agent can call the `run_script` tool 50 times at most. Then it must give its text.
12. `run_script` (`src/llm/script_tool.rs`) runs the JS of the model in the QuickJS `Sandbox` (`src/sandbox/runtime.rs`). The script calls the `discord.*`, `repo.*`, `github.*`, and `web.*` operations. Tavily does the web requests of `web.*`. `run_script` adds the reminder of `prompts/tools/reminder.md` after each result that the model gets.
13. The workflow returns an `Outcome`. `apply_outcome` (`src/discord/reply.rs`) sends the reply in parts of 2000 characters or less. It stores the tool turn in `tool_turns`, with the ID of the first part.

If a step fails, `process_unit` sends the error to the admin log channel with `AdminLog::report_error` (`src/admin/log.rs`). Nuki sends nothing in the channel of the unit.

The Help workflow sends Nuki's draft to Jev with `prompts/decisions/help-quality.md`. If Jev's probability is below `THRESHOLD_HELP_QUALITY`, Nuki sends nothing for an unsolicited unit. For a direct unit, Nuki sends a fallback text from `prompts/workflows/help-fallback.md`. If the text of the Chat workflow is empty, Nuki sends nothing.

### Usage events

`Stats::record` (`src/stats/recorder.rs`) writes usage events to the `events` table. In a run, `RunStats::record` gives each event the run ID, the server, and the channel. `src/stats/event.rs` contains the event types. These functions record the events:

- `record_received` and `report_decision` in `src/discord/pipeline.rs`: `message.received` and `unit.routed`. `src/discord/events.rs` makes their data.
- `JevClient::decide` (`src/decisions/client.rs`): `jev.call`.
- `StatsHook` (`src/llm/stats_hook.rs`): `llm.call`, for each model call.
- `run_script`: `tool.call`.
- The `discord.*` operations (`src/sandbox/ops/discord/client.rs`): `reaction.added` and `emoji.created`.
- `timed` (`src/workflows/timed.rs`): `workflow.finished`.
- `src/discord/reply.rs`: `reply.sent` and `intake.reported`.
- `src/discord/intake_thread.rs`: `thread.created` and `intake.opened`.
- `src/discord/commands.rs`: `model.changed`.
- `AdminLog::report_error`: `error`.

### Intakes

An intake collects a bug report or a feature request in a thread:

1. `start_intake` (`src/discord/intake_thread.rs`) finds the thread. If the unit is in a thread, it uses that thread. If not, it makes a public thread from the last message of the unit.
2. `start_intake` stores the intake in `intakes` and runs the first turn.
3. When the next unit comes in the thread, `route_in_intake` (`src/discord/pipeline.rs`) examines the unit:
   - If a message of the unit mentions Nuki or Nuki's managed role, or replies to Nuki, Nuki does not classify the unit. `continue_intake` runs the next turn.
   - If not, Jev classifies the unit with the question of an `Unsolicited` unit, with all the categories. `decide_in_intake` (`src/routing/dispatch.rs`) gives the `Decision`. If `ignore` is the top category, the decision is `Skip`: Nuki skips the unit and does not change the intake. For example, Nuki skips the messages of a maintainer who works on the bug with the reporter. If another category is on top, the decision is `ContinueIntake`, and `continue_intake` runs the next turn. The thresholds and the chat cooldown do not apply.

   Nuki makes this decision again for each unit, so Nuki can continue the intake after a skipped unit. `continue_intake` gets the history of the intake with `fetch_thread_history`.
4. `IntakeWorkflow::run_turn` (`src/workflows/intake/mod.rs`) sends the conversation to Jev with the questions in `bug-fields.md` or `feature-fields.md`. If fields are missing, the system prompt of the turn gives their names.
5. When all fields are present, `LlmClient::extract` makes the `Report` (`src/workflows/intake/report.rs`).
6. `apply_intake_outcome` sends the report as an embed to the admin log channel, and closes the intake.

If the reporter sends no message for `INTAKE_TIMEOUT_HOURS`, the timeout task completes the intake with `complete_inactive_intake`. The report gives the names of the missing fields. If the thread does not exist, Nuki closes the intake with no report.

### Background tasks

`src/tasks.rs` starts two tasks:

- `spawn_repo_fetch` updates all clones at intervals of `REPO_FETCH_INTERVAL_MINUTES`.
- `spawn_intake_timeout` examines the open intakes at intervals of 10 minutes. It completes the intakes that are past the intake timeout.

### `/change-model`

`Handler::ready` registers `/change-model`. `Handler::interaction_create` sends the command to `src/discord/commands.rs`. The autocomplete shows the OpenRouter models from `ModelsClient::list` (`src/llm/models_list.rs`). `ActiveModel::set` (`src/llm/selection.rs`) saves the model ID in `settings`. If no ID is in `settings`, Nuki uses `DEFAULT_MODEL_ID`.

### State

Nuki keeps its state in these locations:

- SQLite, in `DATA_DIR/nuki.sqlite` (`src/store/`). The tables are `settings`, `intakes`, `tool_turns`, and `events`. `Store::open` applies the migrations in `migrations/`.
- Bare clones of the repositories, in `DATA_DIR/repos/<owner>/<name>.git` (`src/repos/manager.rs`). The catalog contains `nukeop/nuclear` and the public repositories of `NuclearPlayer`. `App::build` clones `nukeop/nuclear`. `RepoManager::ensure_cloned` clones each other repository when a script reads it for the first time.
- Memory: the debounced messages, the chat cooldowns (`Cooldowns`), and the active model. When Nuki stops, it loses the debounced messages and the cooldowns.

## Where to look

| Task | Files |
|---|---|
| Change which messages reach Nuki | `trigger_of`, `trigger_of_unit`, `unit_addresses_nuki` in `src/routing/trigger.rs`. `facts_of` in `src/discord/handler.rs`. |
| Add a trigger rule | A field in `MessageFacts` and a rule in `trigger_of`, in `src/routing/trigger.rs`. Fill the field in `facts_of`. If the rule tells that a message addresses Nuki, put it in `addresses_nuki`. Then `unit_addresses_nuki` also uses it in an intake thread. |
| Change the debounce | `src/routing/debounce.rs`. `DEBOUNCE_SECONDS` in `src/config.rs`. |
| Change the categories | `prompts/decisions/category.md`, `src/routing/category.rs`, `run_category` in `src/discord/pipeline.rs`. |
| Change the thresholds or the chat cooldown | `src/routing/dispatch.rs`, `src/routing/cooldown.rs`, `src/config.rs`. In an intake thread, `decide_in_intake` in `src/routing/dispatch.rs` decides with no thresholds and no cooldown. |
| Change which messages are in the history | `fetch_unit_context` and `fetch_thread_history` in `src/discord/history.rs`. |
| Change how the chat history is shown to the LLM | `build_history` and `header` in `src/discord/context.rs`. `ThreadMarker` in `src/discord/thread_marker.rs`. |
| Change how the history is shown to Jev | `jev_lines` in `src/discord/context.rs`. |
| Change how attachments go to the LLM | `src/discord/attachments.rs`, `src/discord/modality.rs`. |
| Change the author names and roles | `src/discord/authors.rs`. |
| Change the system prompt | `prompts/*.md`, `prompts/workflows/`. `SystemPrompt::build` and `RuntimeContext` in `src/prompts/system.rs`. |
| Change a workflow | `src/workflows/help.rs`, `src/workflows/chat.rs`, `src/workflows/intake/`. |
| Change the fields of an intake | `prompts/decisions/bug-fields.md`, `prompts/decisions/feature-fields.md`. |
| Change the report or its embed | `src/workflows/intake/report.rs`, `src/admin/reports.rs`. |
| Change how Nuki sends a reply | `src/discord/reply.rs`. |
| Change the limit of model calls | `MAX_MODEL_CALLS` in `src/llm/agent.rs`. |
| Change the sandbox limits | `SandboxLimits::production` in `src/sandbox/runtime.rs`. |
| Add a sandbox operation | `src/sandbox/ops/`. Refer to "Add a sandbox operation". |
| Change the description of `run_script` | `prompts/tools/run-script.md`, `src/sandbox/declarations.rs`. |
| Change the model list of `/change-model` | `ModelsClient::list` and `choices` in `src/llm/models_list.rs`. |
| Change the default model | `DEFAULT_MODEL_ID` in `src/llm/selection.rs`. |
| Add a usage event | `src/stats/event.rs`. Record the event with `RunStats::record`. |
| Change the database schema | A new file in `migrations/`, and an `M::up` line in `Store::migrated` in `src/store/mod.rs`. |
| Add an environment variable | `Config` in `src/config.rs`. |
| Change the repositories that Nuki can read | `catalog` in `src/app.rs`, `src/repos/catalog.rs`. |
| Change the GitHub requests | `src/github/client.rs`. |
| Change the Tavily requests of `web.search` and `web.fetch` | `src/web/client.rs`, `src/web/types.rs`. |
| Change the Jev requests | `src/decisions/`. |
| Change the intake timeout or the fetch interval | `src/tasks.rs`, `src/config.rs`. |

## Modules

The bot is one crate, `nuki`.

| Module | Contents |
|---|---|
| `src/config.rs` | `Config`, with the values of the environment variables. `load_dotenv` loads `.env`. |
| `src/logging.rs` | The `tracing` subscriber. `RUST_LOG` sets its filter. |
| `src/tasks.rs` | The background tasks. |
| `src/discord/` | The Serenity handler, the pipeline, `/change-model`, the history and the context builder, the attachments, the reply sender, and the usage events of the pipeline. |
| `src/routing/` | The trigger rules, the debounce, the Jev classification, the thresholds, the chat cooldown, and the `Decision`. |
| `src/decisions/` | The client and the types of the Jev Decisions API on OpenRouter. |
| `src/llm/` | The OpenRouter LLM agent and its limits, the `run_script` tool, the model list, and the active model. |
| `src/workflows/` | The `Workflow` trait, `TurnContext`, `Outcome`, and the Help, Chat, and intake workflows. |
| `src/prompts/` | The compiled prompt files, the system prompt builder, and the parser of the Jev question files. |
| `src/sandbox/` | The QuickJS runtime, the operation table, the TypeScript declarations, and the operations in `ops/`. |
| `src/repos/` | The repository catalog, the clone and fetch manager, the `git` CLI wrapper, and the read operations on the clones. |
| `src/github/` | The read-only GitHub client. It uses REST for releases and pull requests, and GraphQL for Discussions. |
| `src/web/` | The Tavily client for web search and page text. It uses Tavily's keyless access, with no API key. |
| `src/stats/` | The usage event types, and the recorder that writes the events of each run to `events`. |
| `src/store/` | The SQLite store: the tool turns, the intakes, the settings, and the usage events. |
| `src/admin/` | The report embeds and the error messages for the admin log channel. |
| `src/test_support.rs` | The test helpers that more than one module uses, for example `json_response` for `wiremock`. |

`tests/fixtures/` contains the recorded API responses. The tests load them with `include_str!`.

## Verification

Run these commands before you report that a change is complete. All three commands must pass:

```sh
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
```

CI runs the same commands in `.github/workflows/ci.yml`.

## Code rules

- Write clear code that is easy to read, in a functional style. If an immutable abstraction is possible, use it.
- Do not write an `if` statement on one line. Do not put an `if` expression in the position of an argument.
- Give each module and each function one purpose. If a file has more than one purpose, divide the file.
- Add only the features, options, checks, and abstractions that the task tells you to add.

## Test rules

- Test the behavior of the code, not its implementation.
- In each test, examine the item that the name of the test tells. For example, do not examine the length of a list that contains the item.
- Do not put defensive checks or null checks in a test. Let the test fail when the code fails.
- Assert on the full value. If you know the full string, do not assert on a part of it.
- Do not call an external API in a test. Test the HTTP clients with `wiremock`.
- Test the git operations with the `git` CLI, on local repositories in a temporary directory. Make the directory with `tempfile`.
- Test the store with SQLite in memory. Use `Store::open_in_memory()`.
- Put the test helpers that the tests of a module share in the `test_support` module of that module, for example `src/store/test_support.rs`. Declare it with `#[cfg(test)] pub(crate) mod test_support;`.

## Prompts

The prompts are Markdown files in `prompts/`. `src/prompts/files.rs` compiles each file into the binary as one constant, with `include_str!`. To add a prompt file, add a constant for it in `src/prompts/files.rs`.

`SystemPrompt::build` puts these parts in sequence: `personality.md`, `nuclear.md`, `knowledge.md`, the workflow prompt from `prompts/workflows/`, the extra text of the turn, and the "Current context" section.

`src/prompts/questions.rs` parses the files in `prompts/decisions/`. Each file has its format:

- `category.md`: the text before the first `## ` heading is the question. Each `## ` heading is the name of one category, and the text below it is the description of that category. The headings must agree with `Category::name` in `src/routing/category.rs`.
- `bug-fields.md` and `feature-fields.md`: each `## ` heading is the name of one yes-or-no question, and the text below it is the question. If the file has text before the first heading, the parser rejects the file.
- `help-quality.md`: all the text of the file is one yes-or-no question.

## Add a sandbox operation

The operation table is the only source of the sandbox functions. Each `Operation` gives the JS function `namespace.name(params)` in the sandbox. It also gives the TypeScript declaration that the LLM reads in the description of the `run_script` tool. Do not write the JS function or the declaration by hand.

Do these steps:

1. Open the file of the namespace in `src/sandbox/ops/`: `discord/mod.rs`, `repo.rs`, `github.rs`, or `web/mod.rs`.
2. Write a struct for the parameters, with `#[derive(Deserialize)]` and `#[serde(rename_all = "camelCase")]`.
3. Write a function that returns an `Operation`.
4. If the operation uses the shared client of the namespace, call `Operation::with_handle` in that function. If not, call `Operation::typed`. All current operations use `Operation::with_handle`.
5. Give the namespace constant, the name in camelCase, a description of one line, and the `Param` list.
6. Give the return type as a TypeScript type expression.
7. Give the async Rust function.
8. Add the function to the `vec!` of the table function: `discord_table`, `repo_table`, `github_table`, or `web_table`.

For a new namespace, write a new table function. Add it to `full_table` in `src/sandbox/ops/mod.rs`.

## Comments and docs

Write comments, doc comments, and Markdown docs in ASD-STE100 Simplified Technical English. Load the `asd-ste100` skill for the full rules. These rules matter most in this code:

- Give each sentence a real subject that does the action: a function, a type, Nuki, Discord, Jev, the user. Use the active voice.
- Start the doc of a function with a verb in the simple present: "Sends the reply." "Returns the thread of the intake."
- Start the doc of a type or a field with a noun phrase that tells what the item is: "The intake that is open in this thread."
- Put a condition first, in an `If` or `When` clause: "If the run fails, records the outcome `error` and returns the error."
- Use the possessive when it is clear: "Nuki's message", "Jev's answer". Do not stack `of` phrases.
- Write one fact in each sentence. Keep each sentence below 25 words.
- Write a procedure as steps in the imperative: "Add the function to `repo_table`."

Do not write a behavior as a general rule about an indefinite item. People do not talk like that:

| Stiff | Correct |
|---|---|
| A failed run records the outcome `error`, and then returns the error. | If the run fails, records the outcome `error` and returns the error. |
| An unsolicited message gets no reply. A direct question gets the fallback. | Sends nothing for an unsolicited message. Sends the fallback for a direct question. |
| A unit in a thread uses that thread. Else the bot makes a public thread. | If the unit is in a thread, uses that thread. If not, makes a public thread from the last message of the unit. |
| A message of Nuki is an assistant turn. | Nuki's messages become assistant turns. |
| A push to `master` deploys to Fly.io. | Push to `master` to deploy to Fly.io. |
| `Http` is not `Clone`. Thus the gateway client needs its own `Http`. | The gateway client needs its own `Http`, because `Http` is not `Clone`. |
| Returns the facts of `message`. | (Delete it. The signature already tells this.) |

## Commits

Write the commit message on one short line, with no body and no prefix. Tell what changed, for example "Add the background tasks for git fetch and the intake timeout".
