# knowledge-bae

Local semantic search for an [Obsidian](https://obsidian.md) vault, for AI agents such as Claude Code, driven by a skill that runs the command line tool.

A personal tool: one binary, no server, no cloud account, nothing leaves your machine.

> **Status:** early. Indexing and search work from the command line. The git-hook installer and the agent skill (onboarding, search, vault creation) are still to come; progress is tracked in [issue #2](https://github.com/kkbae-com/knowledge-bae/issues/2).

## How it works

```
Obsidian vault (git) ──post-commit hook──▶ knowledge-bae ingest ──▶ local index (SQLite)
                                                                          ▲
Claude Code ──skill──▶ knowledge-bae search ───────────────────────────┘
```

One Rust binary: the CLI an agent (or you) runs, and the ingest that git hooks run. See [Commands](#commands).

- **Index:** a single SQLite file stored outside the vault. It is derived data and can always be rebuilt from the vault.
- **Embeddings:** a small quantized model (bge-small) run in-process. It loads on first use and takes about 175 MB of RAM; a session that never searches stays around 10 MB. No API keys.
- **Staying current:** the Obsidian Git plugin auto-commits the vault; git hooks re-index only the changed markdown files.
- **No chat completion:** the agent does the reasoning. knowledge-bae only stores and retrieves.

## Onboarding

A skill will walk an agent through setup:

- choosing and locating your vault
- installing prerequisites (git, the Obsidian Git plugin)
- installing the git hooks that send vault updates to the indexer
- running the initial ingestion

## Layout

- `src/cli` - the Rust command line tool

## Prerequisites

- **[Obsidian](https://obsidian.md)** with your notes as a vault.
- **git**, with the vault as a git repository (`git init` in the vault folder if it isn't one yet).
- **The [Obsidian Git](https://github.com/Vinzent03/obsidian-git) community plugin** (Settings > Community plugins > Browse). Turn on its automatic commit interval so your edits are committed for you. knowledge-bae re-indexes on each commit, so uncommitted edits aren't searchable until the plugin commits them.
- **Rust 1.88+** to build and install knowledge-bae from this repo.

The index is kept current by git hooks in the vault (`post-commit`, `post-merge`, `post-checkout`) that run `knowledge-bae ingest --changed`. Installing them is not automated yet; it will be a command and part of the onboarding prompt.

## Try it

The first run downloads the embedding model (about 65 MB).

```bash
cargo install --path src/cli          # installs `knowledge-bae`
knowledge-bae vault add ~/path/to/vault
knowledge-bae ingest --vault <name> --all
knowledge-bae search "how do I deploy the api"
```

The index lives in your platform data directory (override with `KNOWLEDGE_BAE_DB`). It is derived data: delete it and run `ingest --all` to rebuild.

## Commands

Run `knowledge-bae <command> --help` for every flag.

### `vault`

Manages the vaults that get indexed. You can register several.

| Command | What it does |
|---|---|
| `vault add <path> [--name N] [--structure-file F]` | Registers the folder as a vault. The name defaults to the folder name. `--structure-file` stores instructions for how the vault should be organized, which the create/reorganize prompts will use. |
| `vault list` | Shows each registered vault, its path, and whether structure instructions are set. |
| `vault remove <name>` | Unregisters the vault and deletes its index data. The vault's files are never touched. |

### `ingest`

Brings the index up to date with a vault. Pass `--vault <name or path>`; if you leave it out, the vault containing the current directory is used (which is how git hooks run it), or the only registered vault.

| Command | What it does |
|---|---|
| `ingest --all` | Walks the whole vault. New and edited notes are re-chunked and embedded, unchanged notes are skipped, and notes that no longer exist are removed. Safe to run any time; it is also how you rebuild after deleting the index or switching models. |
| `ingest --changed` | Indexes only what the latest git commit changed: additions, edits, deletions and renames. Moving a note to another folder reuses its embeddings. This is what the `post-commit` hook runs. |
| `ingest --changed --from <rev> --to <rev>` | Same, for the changes between two revisions, e.g. `--from ORIG_HEAD --to HEAD` for a `post-merge` hook. |

Only markdown files are indexed. Hidden folders (`.obsidian`, `.trash`, `.git`) and anything matched by Obsidian's "Excluded files" setting are skipped. A file that can't be read is reported and skipped; it doesn't stop the run.

### `search "<query>"`

Finds the chunks closest in meaning to the query and prints each with its score, note title and headings, vault and path, and a short excerpt. `--vault <name>` limits it to one vault (default: all) and `-n <count>` sets how many results (default 5). The first search in a process loads the embedding model, which takes about a second.

### `status`

Shows where the index file is, which embedding model it uses, and how many vaults, notes and chunks it holds. Chunks "without embeddings" means a run was interrupted or the model changed; run `ingest --all` to fill them in.

## Use it with an agent

Any agent that can run shell commands can use the CLI directly, e.g. by running `knowledge-bae search "<query>"` and reading the output.

A Claude Code skill that teaches the agent when and how to do this, and walks through onboarding, is planned. An MCP server for clients without a shell (Copilot chat, the Claude desktop app) may follow if it is needed.

## Development

```bash
cd src/cli
cargo test
cargo test embedder -- --ignored --nocapture   # downloads the model; prints memory use
```

## License

This project is open source.
