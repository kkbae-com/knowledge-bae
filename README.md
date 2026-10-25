# knowledge-bae

Local semantic search for an [Obsidian](https://obsidian.md) vault, for AI agents such as Claude Code, driven by a skill that runs the command line tool.

A personal tool: one binary, no server, no cloud account, nothing leaves your machine.

> **Status:** early. Indexing, search and the git hooks work from the command line. The Claude Code skill is a first draft; progress is tracked in [issue #2](https://github.com/kkbae-com/knowledge-bae/issues/2).

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
- **The [Obsidian Git](https://github.com/Vinzent03/obsidian-git) community plugin** (Settings > Community plugins > Browse > "Git"), enabled and set to commit automatically. knowledge-bae re-indexes on each commit, so uncommitted edits aren't searchable until the plugin commits them. After you install the plugin, `knowledge-bae obsidian setup` can enable it and turn on automatic commits for you.
- **Rust 1.88+** to build and install knowledge-bae from this repo.

The index is kept current by git hooks in the vault, installed with `knowledge-bae hooks install` (see [Commands](#commands)).

## Try it

The first run downloads the embedding model (about 65 MB).

```bash
cargo install --path src/cli          # installs `knowledge-bae`
knowledge-bae vault add ~/path/to/vault
knowledge-bae ingest --vault <name> --all
knowledge-bae hooks install --vault <name>   # keep the index current from now on
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
| `vault structure [name] [--set-file F \| --set-default]` | Shows the instructions for how the vault should be organized, or replaces them from a file or with the built-in developer layout (projects, stories, bugs, investigations; see `src/cli/defaults/vault-structure.md`). An agent reads these when creating or reorganizing a vault. |
| `vault remove <name>` | Unregisters the vault and deletes its index data. The vault's files are never touched. |

### `hooks`

Installs the git hooks that keep the index current, so you never run `ingest` by hand after the first time. The vault must be a git repository (run `git init` there first if not). Run `install` from the installed binary (`cargo install --path src/cli`), not a `cargo build` copy, because the hooks call the binary by absolute path.

| Command | What it does |
|---|---|
| `hooks install [--vault N]` | Adds a marked block to `post-commit`, `post-merge`, `post-checkout` and `post-rewrite`. Safe to re-run: it refreshes the block (for example after the binary moves). Existing hook content is kept, and a hook that isn't a shell script is refused. |
| `hooks uninstall [--vault N]` | Removes only the knowledge-bae block, and deletes a hook file that has nothing else in it. |
| `hooks status [--vault N]` | Shows which of the four hooks are installed. |

`post-commit` runs `ingest --changed`, which reads exactly what the commit changed. The other three run `ingest --all`, which is cheap when nothing changed because unchanged notes are skipped by hash and the model never loads. Together they cover commits, pulls, branch switches and rebases. Hooks run in the background so a commit never waits on them, and their output goes to `hooks.log` next to the index file.

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

### `doctor`

Checks that everything is wired up and says what to fix: git is installed; the Obsidian Git plugin is installed, enabled and committing; the index opens; each vault's folder exists; its hooks are installed and still point at a binary that exists (a moved or rebuilt binary is the usual cause of silent staleness); the notes on disk match what's indexed; and whether structure instructions are set. Exits non-zero if anything is broken, so an agent can run it to verify setup.

### `status`

Shows where the index file is, which embedding model it uses, and how many vaults, notes and chunks it holds. Chunks "without embeddings" means a run was interrupted or the model changed; run `ingest --all` to fill them in.

### `obsidian`

Checks and fixes the one part of Obsidian's configuration knowledge-bae depends on: the Obsidian Git plugin. Both take `--vault` (default: the vault containing the current directory).

| Command | What it does |
|---|---|
| `obsidian check` | Reports whether the vault has been opened in Obsidian, the plugin is installed and enabled, and automatic commits are on. |
| `obsidian setup [--interval N]` | Adds the plugin to `.obsidian/community-plugins.json` and, only if no automatic commit is configured, sets a commit interval (default 5 minutes). Other settings and plugins, and any interval you already chose, are left untouched. It refuses (and changes nothing) if the plugin isn't installed or a config file isn't valid JSON. |

It can't install the plugin itself; do that in Obsidian (Settings > Community plugins > Browse). Restart Obsidian after `setup` changes anything, because Obsidian keeps its settings in memory and may overwrite a file edited behind its back. `doctor` reports the same checks.

### `skill install`

Copies the bundled Claude Code skill into `~/.claude/skills/knowledge-bae/` (or `--dir` for another skills folder). The skill is compiled into the binary, so it always matches the commands in the build you installed.

## Use it with an agent

The repo ships a Claude Code skill (`skills/knowledge-bae/SKILL.md`). It teaches the agent how to search your vault and read the results, and walks it through onboarding a vault, creating one from your stored structure instructions, and reorganizing one. Install it with the tool:

```bash
knowledge-bae skill install                      # ~/.claude/skills, every project
knowledge-bae skill install --dir .claude/skills # just the current project
```

Then start a new Claude Code session and ask it to set up your vault, or just ask a question your notes might answer. Re-run `skill install` after upgrading to refresh it.

Any other agent that can run shell commands can use the CLI directly with `knowledge-bae search "<query>"`. An MCP server for clients without a shell (Copilot chat, the Claude desktop app) may follow if it is needed.

## Development

```bash
cd src/cli
cargo test
cargo test embedder -- --ignored --nocapture   # downloads the model; prints memory use
```

## License

This project is open source.
