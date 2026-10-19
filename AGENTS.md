# AGENTS.md

Nuki is a Rust Discord bot for the server of the Nuclear music player. `README.md` tells what the bot does, how to run it, and which environment variables it reads. This file tells you how to work in the code.

## Module layout

The bot is one crate, `nuki`. `src/main.rs` only calls `nuki::run()` in `src/lib.rs`.

| Module | Responsibility |
|---|---|
| `src/lib.rs` | `run()`: loads the configuration, builds the services, starts the background tasks and the Discord client. |
| `src/app.rs` | `App`: builds and holds the services that the handler and the background tasks share, and builds the sandbox for the server of a turn. |
| `src/config.rs` | The typed `Config` from the environment variables, and the load of `.env`. |
| `src/logging.rs` | The `tracing` subscriber, filtered by `RUST_LOG`. |
| `src/tasks.rs` | The background tasks: the periodic `git fetch` and the intake timeout. |
| `src/discord/` | The Serenity handler, the message pipeline, `/change-model`, the history and context builder, attachments, and the reply sender. |
| `src/routing/` | The trigger rules in code, the debounce, the Jev classification, the thresholds and the chat cooldown, and the `Decision`. |
| `src/decisions/` | The client and the types of the Jev Decisions API on OpenRouter. |
| `src/llm/` | The OpenRouter LLM agent with its limits, the `run_script` tool, the models list, and the active model. |
| `src/workflows/` | The `Workflow` trait, `TurnContext`, `Outcome`, and the Help, Chat, and intake workflows. |
| `src/prompts/` | The compiled prompt files, the system prompt builder, and the parser of the Jev question files. |
| `src/sandbox/` | The QuickJS runtime, the operation table, the TypeScript declarations, and the operations in `ops/`. |
| `src/repos/` | The repository catalog, the clone and fetch manager, the `git` CLI wrapper, and the read operations on the clones. |
| `src/github/` | The read-only GitHub client: REST for releases and pull requests, GraphQL for Discussions. |
| `src/store/` | SQLite: the stored tool turns, the intake records, and the settings. |
| `src/admin/` | The report embeds and the error messages for the admin log channel. |
| `src/test_support.rs` | Test helpers that more than one module uses, for example `json_response` for `wiremock`. |

`migrations/` contains the SQL migrations. `tests/fixtures/` contains the recorded API responses that the tests load with `include_str!`.

## Verification

Run these commands before you say that a change is complete. All three must pass:

```sh
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
```

CI runs the same commands in `.github/workflows/ci.yml`.

## Code rules

- Write elegant, readable, functional code. Use an immutable abstraction where one is possible.
- Do not write one-line `if` statements. Keep `if` expressions out of argument positions.
- Give each module and each function one responsibility. Divide a file when it has more than one concern.
- Do not add a feature, an option, a check, or an abstraction that the task does not specify.

## Test rules

- Tests examine behavior, not implementation details.
- A test examines the item that its name tells. A test does not examine a property of a different item, for example the length of a list that contains the item.
- Tests do not contain defensive checks or null checks. A test fails when the code fails.
- Tests assert on full values. A test does not assert on a part of a string when the full string is known.
- Tests do not call a live API. Test the HTTP clients with `wiremock`.
- Test the git operations with real `git` against local repositories in a temporary directory. Use `tempfile` for the directory.
- Test the store with SQLite in memory. Use `Store::open_in_memory()`.
- Put the test helpers that a module shares in the `test_support` module of that module, for example `src/store/test_support.rs`. Declare it with `#[cfg(test)] pub(crate) mod test_support;`.

## Prompts

The prompts are Markdown files in `prompts/`. `src/prompts/files.rs` compiles each file into the binary with `include_str!`, as one constant. To add a prompt file, add a constant for it in `src/prompts/files.rs`.

The files in `prompts/decisions/` have formats that `src/prompts/questions.rs` parses:

- `category.md`: the text before the first `## ` heading is the question. Each `## ` heading is one option, and the text below it is the description of that option. The headings must agree with the `Category` enum in `src/routing/category.rs`.
- `bug-fields.md` and `feature-fields.md`: each `## ` heading is the name of one yes-or-no question, and the text below it is the question. The parser rejects text before the first heading.
- `help-quality.md`: all of the file is one yes-or-no question.

## Add a sandbox operation

The operation table is the only source of the sandbox functions. Each `Operation` entry gives the JS function `namespace.name(params)` in the sandbox. It also gives the TypeScript declaration that the LLM reads in the `run_script` tool description. Do not write the JS function or the declaration by hand.

Do these steps:

1. Open the file of the namespace in `src/sandbox/ops/`: `discord/mod.rs`, `repo.rs`, or `github.rs`.
2. Write a parameters struct with `#[derive(Deserialize)]` and `#[serde(rename_all = "camelCase")]`.
3. Write a function that returns an `Operation`.
4. In that function, call `Operation::typed`. If the operation needs the shared client of the namespace, call `Operation::with_handle` instead. All current operations use `Operation::with_handle`.
5. Give the namespace constant, the name in camelCase, a description of one line, and the `Param` list.
6. Give the return type as a TypeScript type expression, and give the async Rust function.
7. Add the function to the `vec!` of the table function: `discord_table`, `repo_table`, or `github_table`.

For a new namespace, add a new table function. Then add it to `full_table` in `src/sandbox/ops/mod.rs`.

## Commits

A commit message is one short line. It has no body and no prefix. It tells what changed, for example "Add the background tasks for git fetch and the intake timeout".
