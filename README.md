# knowledge-bae

Local semantic search for an [Obsidian](https://obsidian.md) vault, exposed to AI agents (Claude, Copilot) through an MCP server.

A personal tool: one binary, no server, no cloud account, nothing leaves your machine.

> **Status:** early. The Rust MCP server is a skeleton; the design below is the plan, tracked in [issue #2](https://github.com/kkbae-com/knowledge-bae/issues/2).

## How it works

```
Obsidian vault (git) ──post-commit hook──▶ knowledge-bae ingest ──▶ local index (SQLite)
                                                                          ▲
Claude / Copilot ──MCP (stdio)──▶ knowledge-bae mcp ──── search ─────────┘
```

One Rust binary, three modes:

```
knowledge-bae mcp                 # MCP server (what the agent launches)
knowledge-bae ingest --all        # full (re)index of the vault
knowledge-bae ingest --changed    # index files changed in the last commit (git hooks)
```

- **Index:** a single SQLite file stored outside the vault. It is derived data and can always be rebuilt from the vault.
- **Embeddings:** a small local model run in-process. It is loaded only when needed and unloaded when idle, so the idle footprint stays small. No API keys.
- **Staying current:** the Obsidian Git plugin auto-commits the vault; git hooks re-index only the changed markdown files.
- **No chat completion:** the agent does the reasoning. knowledge-bae only stores and retrieves.

## Onboarding

The MCP server provides prompts that walk an agent through setup:

- choosing and locating your vault
- installing prerequisites (git, the Obsidian Git plugin)
- installing the git hooks that send vault updates to the indexer
- running the initial ingestion

## Layout

- `src/mcp` - Rust CLI and MCP server

`src/api` holds the earlier .NET Web API prototype (Semantic Kernel + pgvector). It is being retired in favor of the single-binary design above.

## Development

Prerequisites: Rust 1.70+.

```bash
cd src/mcp
cargo build
cargo run
```

## License

This project is open source.
