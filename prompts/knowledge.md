# Where to find information

You have local clones of the Nuclear repositories and read-only access to GitHub and to this Discord server, all through the `run_script` tool.

## The `nukeop/nuclear` repository

It is a pnpm monorepo managed with Turborepo. The packages in `packages/`:

- `player`: the main app (Tauri: React in `src/`, Rust in `src-tauri/`).
- `ui`: shared UI components.
- `model`: the shared data model.
- `plugin-sdk`: the SDK that plugins use (`@nuclearplayer/plugin-sdk`).
- `hifi`: the audio engine.
- `themes`: the theme engine and built-in themes.
- `i18n`: translations.
- `docs`: the user and developer documentation.
- `website`: the website at nuclearplayer.com.
- `storybook`: demos of the UI components.
- `tools`: release scripts.
- `videos`, `eslint-config`, `tailwind-config`: supporting packages.

## Documentation

The documentation at https://docs.nuclearplayer.com is generated from `packages/docs`. `packages/docs/SUMMARY.md` is its table of contents.

To link a page, convert its path:

- `packages/docs/<path>.md` has the URL `https://docs.nuclearplayer.com/<path>`. Example: `packages/docs/user-manual/installation.md` is `https://docs.nuclearplayer.com/user-manual/installation`.
- `packages/docs/README.md` is `https://docs.nuclearplayer.com`. A `README.md` inside a directory has the URL of that directory.

Link to the documentation when a page answers the question. Only link pages that you have seen exist.

## Other repositories

The `NuclearPlayer` GitHub organization has the plugins (`NuclearPlayer/nuclear-plugin-*`, for example YouTube, Bandcamp, SoundCloud, MusicBrainz, Discogs, Last.fm, LRCLIB), the plugin registry (`NuclearPlayer/plugin-registry`), the theme registry (`NuclearPlayer/theme-registry`), and online services (`NuclearPlayer/nuclear-tritone`). Call `repo.listRepos()` for the full list. A repository is cloned the first time you use it.

## Using `run_script`

- `repo.*` reads code, docs, commits, and tags in the local clones.
- `github.*` reads releases, pull requests, and GitHub Discussions.
- `discord.*` reads messages in this server, adds reactions, and adds custom emoji.

Do as much as you can in one script: search, read, and filter in the same call. Return only the data you need, not whole files or long lists, because the output is truncated at 20 KB.
