---
name: knowledge-bae
description: Search and set up the user's Obsidian vault with the `knowledge-bae` command line tool (local semantic search over their notes). Use this whenever the user asks to find, recall or look up something in their notes, vault, knowledge base, past stories, bugs or investigations ("what did we decide about X", "did I write anything on Y", "find my notes on Z"), even if they never mention knowledge-bae. Also use it to onboard or set up a vault, install the git hooks, create or reorganize a vault, re-index after notes change, or check why search results are stale or missing.
---

# knowledge-bae

`knowledge-bae` indexes Obsidian vaults on this machine and finds notes by meaning, not just keywords. It only stores and retrieves. You do the reasoning over what it returns. Everything is a shell command; run `knowledge-bae --help` or `<command> --help` if you are unsure of a flag.

## Prerequisites

Search needs only the tool and an indexed vault. Keeping the index current automatically needs the rest. Check them with `knowledge-bae doctor` before relying on any of it, and tell the user plainly which are missing:

- **The `knowledge-bae` command**, installed from the repo with `cargo install --path src/cli` (needs Rust 1.88 or newer to build). Install the binary rather than running a build from `target/`, because hooks call it by absolute path.
- **git**, with the vault as a git repository. Re-indexing runs from git hooks.
- **Obsidian**, with the notes as a vault (the folder has been opened in Obsidian at least once, so it has a `.obsidian` folder).
- **The Obsidian Git community plugin**, installed, enabled, and set to commit automatically. Without automatic commits, edits only become searchable when someone commits or runs `ingest`.

Start with `knowledge-bae doctor` when you do not know the state of things. It reports what is installed, which vaults exist, whether hooks are wired up and whether the index matches the notes on disk. If the command is not found, the tool is not installed: from the knowledge-bae repo run `cargo install --path src/cli`.

## Searching

```bash
knowledge-bae search "how do we deploy the api" -n 5            # all vaults
knowledge-bae search "duplicate ledger bug" --vault work -n 5   # one vault
```

Each result shows a score, the note title and heading, `vault:path`, and an excerpt.

- **Phrase the query as a question or a description of the idea**, the way the note might explain it. Matching is by meaning, so "why did duplicated ledgers lose their bills" finds the bug write-up even if the note never uses those words.
- **Try two or three rephrasings** when the first results look off, and combine what you learn. One query is a single view of the notes.
- **Read the scores honestly.** Roughly 0.7 and up is usually the right note, 0.6 to 0.7 is worth a look, and below that the vault probably does not cover it. If nothing scores well, say the notes do not seem to cover it. Do not invent an answer.
- **Open the note itself for the full context.** The excerpt is one chunk. Run `knowledge-bae vault list` to get the vault's folder, then read `<vault folder>/<path>`.
- **For exact strings** such as a ticket id, an error message or a command, search by meaning will blur them. Use grep on the vault folder instead.
- **Cite the note** (vault and path) when you use what you found, so the user can open it.

## Keeping the index current

Once hooks are installed, every git commit in the vault re-indexes what changed, and the Obsidian Git plugin commits the user's edits on an interval. So an edit made a minute ago may not be searchable yet. If you just wrote or changed notes and want them searchable right now, run:

```bash
knowledge-bae ingest --vault <name> --all
```

This is safe and cheap to repeat: unchanged notes are skipped, and only changed text is embedded. It indexes the files on disk, committed or not.

## Onboarding a vault

Do these in order, and ask instead of guessing where the answer is the user's choice (vault location, vault name, which structure).

1. Run `knowledge-bae doctor` to see what already exists.
2. Ask where the vault is, or where to create it. A vault sits next to the folder that holds the code repositories, not inside it.
3. Make sure the folder is a git repository (`git init` there if not). Hooks and the Obsidian Git plugin both need it. A fresh vault has nothing committed yet; that is fine, the plugin makes the first commit.
4. Check the Obsidian side with `knowledge-bae obsidian check --vault <name>` (once the vault is registered in step 5; before that, look for `<vault>/.obsidian/plugins/obsidian-git`).
   - **Not installed:** the user installs it, since that is the official path: Obsidian, Settings, Community plugins, Browse, "Git", Install. If community plugins are switched off ("Restricted mode"), they have to turn them on in the same settings page; that is their decision to make in Obsidian. If they would rather you do it and say so, you can download the same three files Obsidian would (`gh release download --repo Vinzent03/obsidian-git --pattern main.js --pattern manifest.json --pattern styles.css --dir "<vault>/.obsidian/plugins/obsidian-git"`), but ask first because it puts third-party code into their vault.
   - **Installed but not enabled, or automatic commits off:** run `knowledge-bae obsidian setup --vault <name>`. It enables the plugin and sets a commit interval only if none is configured, and leaves every other setting and plugin alone. It never overrides an interval or "commit after edits stop" the user already chose.
   - **Restart Obsidian afterwards.** Obsidian keeps its settings in memory and can overwrite a file changed behind its back, so ask the user to restart it or run "Reload app without saving" from the command palette. Do this when the setup command reports it changed something.
5. Register the vault: `knowledge-bae vault add <path> --name <name>`.
6. Ask how they want the vault organized, then store it. The built-in developer layout (projects, stories, bugs, investigations) is `knowledge-bae vault structure <name> --set-default`. Their own rules go in a file: `--set-file <file>`.
7. Install the hooks: `knowledge-bae hooks install --vault <name>`. Check `which knowledge-bae` first; if it points into a `target/` folder, install the real binary (`cargo install --path src/cli`) and run the hooks command from that one, because hooks call the binary by absolute path.
8. Index everything: `knowledge-bae ingest --vault <name> --all`. The first run downloads a roughly 65 MB embedding model, so warn the user it may take a moment.
9. Run `knowledge-bae doctor` again and confirm there are no `FAIL` or `warn` lines you can fix.

## Creating a vault

When the user asks you to create their vault, the rules for its shape live in the vault's stored structure instructions, not in this skill, so they stay editable.

1. `knowledge-bae vault structure <name>` prints the instructions. If none are set, offer `--set-default` or ask for their own, then read it again.
2. Ask for what the structure needs: the project names, and which repositories each project spans.
3. Create the folders and starter notes the instructions describe inside the vault folder. Never overwrite an existing file.
4. Run `knowledge-bae ingest --vault <name> --all` so the new notes are searchable immediately.

Leave committing to the Obsidian Git plugin unless the user asks you to commit.

## Reorganizing a vault

1. Read the stored instructions (`knowledge-bae vault structure <name>`) and list the vault's current tree.
2. Work out what is out of place and show the user the list of moves before touching anything. Moving notes is easy to get subtly wrong, so wait for a yes.
3. Move files with `git mv` so git records a rename. A move to another folder keeps each note's embeddings, so nothing is recomputed. Renaming a file changes its title, so it is re-indexed, and any `[[links]]` to the old name need updating.
4. Obsidian resolves `[[Note]]` links by filename, so links survive a folder move as long as filenames stay unique. Grep for the old name after any rename.
5. Run `knowledge-bae ingest --vault <name> --all` to sync the index straight away. It also removes entries for notes that no longer exist.

## When something is wrong

- **Results are missing or stale:** run `knowledge-bae doctor`. If it reports unindexed notes, run `ingest --all`.
- **A hook points at a binary that no longer exists** (the tool was rebuilt or moved): reinstall with `knowledge-bae hooks install`.
- **Hooks seem not to run:** `knowledge-bae hooks status`, and read `hooks.log`, which sits in the same folder as the index file (`knowledge-bae status` prints the index path).
- **"Nothing is indexed yet":** no vault has been added or ingested. Follow the onboarding steps.
- **Search is slow the first time in a session:** the embedding model loads on first use, about a second. That is expected.
