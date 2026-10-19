# Nuki

The Discord bot of the [Nuclear](https://github.com/nukeop/nuclear) music player server. Nuki helps users, collects bug reports and feature requests, and chats. She can read the Nuclear repositories and the GitHub API through a JavaScript sandbox. Admins pick the LLM with `/change-model`.

## Run

Needs Rust, `git`, a Discord bot with the `MESSAGE_CONTENT` and `GUILD_MEMBERS` intents, an OpenRouter key, and a read-only GitHub token.

```sh
cp .env.template .env   # fill in the four values
cargo run
```

The first start clones `nukeop/nuclear`, so it takes a while.

## Configuration

Required: `DISCORD_TOKEN`, `OPENROUTER_API_KEY`, `GITHUB_TOKEN`, `ADMIN_LOG_CHANNEL_ID` (where reports and errors go).

Optional:

| Variable | Default | |
|---|---|---|
| `CONTEXT_MESSAGES` | `15` | Messages of history per turn |
| `CHAT_COOLDOWN_MINUTES` | `15` | Pause between unsolicited chats in a channel |
| `THRESHOLD_HELP`, `_BUG`, `_FEATURE`, `_CHAT` | `0.7` | Confidence needed to answer without a mention |
| `THRESHOLD_HELP_QUALITY` | `0.7` | Confidence needed to send a help answer |
| `INTAKE_TIMEOUT_HOURS` | `24` | Silence before an intake is reported as is |
| `REPO_FETCH_INTERVAL_MINUTES` | `15` | `git fetch` interval |
| `DEBOUNCE_SECONDS` | `3` | Wait for more messages from the same author |
| `DATA_DIR` | `/data` | SQLite file and clones |
| `RUST_LOG` | `info` | Log filter |

## Deploy

A push to `master` deploys to Fly.io after CI passes. To deploy by hand: `fly deploy --ha=false`. Without `--ha=false`, Fly starts two machines, and Nuki replies twice.

## Prompts

The prompts are Markdown files in `prompts/`. They are compiled into the binary, so a change needs a new build.
