# Nuki

Nuki is the Discord bot of the official server of the [Nuclear](https://github.com/nukeop/nuclear) music player. She has the personality of the Nuclear mascot. The bot reads each message in the server, and the Jev decision model on OpenRouter puts it in a category. Then the bot runs one of four workflows: Help, Bug intake, Feature intake, or Chat. An LLM writes the replies. The LLM has one tool, `run_script`, which runs JavaScript in a QuickJS sandbox. Scripts can read the server, add reactions and custom emoji, read local clones of the Nuclear repositories, and query the GitHub API. Administrators select the LLM with the `/change-model` slash command.

## Run the bot on your computer

Use a second bot application and a test server. Do not connect a development build to the Nuclear server.

You need:

- The stable Rust toolchain.
- `git` on your `PATH`. The bot clones the repositories with the `git` CLI.
- A Discord bot application with the `MESSAGE_CONTENT` and `GUILD_MEMBERS` privileged intents.
- An OpenRouter API key.
- A fine-grained GitHub token with read access to public repositories only.

Do these steps:

1. Copy `.env.template` to `.env`.
2. Set the six mandatory values in `.env`. The section [Environment variables](#environment-variables) tells what each value is.
3. Set `DATA_DIR=data`. The default, `/data`, is the volume path on the server, and Git ignores `/data` in the repository.
4. Invite the bot to your test server.
5. Run `cargo run`.

At start, the bot clones `nukeop/nuclear` into `DATA_DIR/repos`. The first start takes some time because of this clone. The bot also makes the SQLite file `DATA_DIR/nuki.sqlite`.

## Environment variables

The bot reads `.env` from the working directory, if the file exists. Variables in the process environment override the values in `.env`.

| Variable | Default | Purpose |
|---|---|---|
| `DISCORD_TOKEN` | mandatory | The bot token. |
| `DISCORD_GUILD_ID` | mandatory | The server. The bot registers `/change-model` for this server only. |
| `OPENROUTER_API_KEY` | mandatory | The OpenRouter key, for the LLM and for Jev. |
| `GITHUB_TOKEN` | mandatory | A fine-grained token with read access to public repositories only. |
| `REPORTS_CHANNEL_ID` | mandatory | The channel for bug reports and feature reports. |
| `ADMIN_LOG_CHANNEL_ID` | mandatory | The channel for workflow errors. It can be the same channel as the reports channel. |
| `LLM_MODEL` | `google/gemini-3.8-flash` | The LLM when no administrator selected a model with `/change-model`. |
| `CONTEXT_MESSAGES` | `15` | The number of channel messages in the history of a turn. An intake uses the messages of its thread instead. |
| `CHAT_COOLDOWN_MINUTES` | `15` | The time after an unsolicited chat reply in a channel during which Nuki starts no other unsolicited chat in that channel. |
| `THRESHOLD_HELP` | `0.7` | The minimum probability for unsolicited Help. |
| `THRESHOLD_BUG` | `0.7` | The minimum probability for an unsolicited Bug intake. |
| `THRESHOLD_FEATURE` | `0.7` | The minimum probability for an unsolicited Feature intake. |
| `THRESHOLD_CHAT` | `0.7` | The minimum probability for unsolicited Chat. |
| `THRESHOLD_HELP_QUALITY` | `0.7` | The minimum probability that a Help draft is good. Below it, Nuki does not send the draft. |
| `INTAKE_TIMEOUT_HOURS` | `24` | The time without a message from the reporter before the bot completes an intake with the available information. |
| `REPO_FETCH_INTERVAL_MINUTES` | `15` | The interval of `git fetch` on the clones. |
| `DEBOUNCE_SECONDS` | `3` | The time without a message from the author before the bot handles the messages of the author as one message. |
| `DATA_DIR` | `/data` | The directory for the SQLite file and the clones. |
| `RUST_LOG` | `info` | The log filter, in the `tracing-subscriber` `EnvFilter` syntax. |

The thresholds apply only to messages that do not mention Nuki or reply to her. Nuki always replies to a message that mentions her or replies to her.

## Checks

CI runs these three commands for each push and each pull request. Run them before you push:

```sh
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
```

The tests use no network and no live API.

## Deployment

The bot runs on exactly one Fly.io machine in the `fra` region. Do not start a second machine: two bots connect to Discord with the same token and reply to each message two times. `fly.toml` contains the configuration of the app.

The machine has the 1 GB volume `nuki_data` at `/data`. The volume holds the SQLite file and the clones. `fly.toml` sets `DATA_DIR=/data` and `RUST_LOG=info`.

The `Dockerfile` compiles the release build in a `rust:1-trixie` stage, and copies the `nuki` binary into a `debian:trixie-slim` stage with `git` and the CA certificates. The prompts are in the binary, thus the image does not contain `prompts/`.

The six mandatory variables are Fly secrets. Set them with `fly secrets set`:

```sh
fly secrets set DISCORD_TOKEN=... DISCORD_GUILD_ID=... OPENROUTER_API_KEY=... \
  GITHUB_TOKEN=... REPORTS_CHANNEL_ID=... ADMIN_LOG_CHANNEL_ID=...
```

A push to `master` deploys the bot after CI passes. To deploy by hand, run:

```sh
fly deploy --ha=false
```

Always use `--ha=false`. Without it, Fly makes two machines on the first deploy of an app.

## Prompts

All prompts are Markdown files in `prompts/`:

| Path | Contents |
|---|---|
| `prompts/personality.md`, `prompts/nuclear.md`, `prompts/knowledge.md` | The shared parts of the system prompt: who Nuki is, what Nuclear is, and where Nuki finds information. |
| `prompts/workflows/` | The instructions for each workflow. |
| `prompts/decisions/` | The questions for Jev: the message category, the quality of a Help draft, and the checklist fields of the intakes. |
| `prompts/tools/run-script.md` | The description of the `run_script` tool. The bot adds the TypeScript declarations of the sandbox functions after it. |

`src/prompts/files.rs` compiles these files into the binary with `include_str!`. Thus, a change to a prompt has an effect only after a new build.
