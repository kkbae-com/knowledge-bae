---
name: knowledge-bae
description: Search and set up the user's Obsidian vault with the `knowledge-bae` command line tool (local semantic search over their notes). Use this whenever the user asks to find, recall or look up something in their notes, vault, knowledge base, past stories, bugs or investigations ("what did we decide about X", "did I write anything on Y", "find my notes on Z"), even if they never mention knowledge-bae. Also use it to onboard or set up a vault, install the git hooks, create or reorganize a vault, re-index after notes change, or check why search results are stale or missing. Also use it to track the user's projects and produce their weekly summaries: timesheet entries, "what did I work on last week", "fill in my timesheet", registering a project, or setting how summaries are written.
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
   - **Installed but not enabled, or automatic commits off:** run `knowledge-bae obsidian setup --vault <name>`. It enables the plugin and sets a commit interval only if none is configured. If the vault has no git remote, it also turns off the plugin's push and pull on commit, which would only fail. It leaves every other setting and plugin alone. It never overrides an interval or "commit after edits stop" the user already chose.
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

## Projects and weekly summaries

knowledge-bae can remember the user's projects and gather each week's work on them, so you can write timesheet entries on request. It stores records and gathers facts. You write the prose. Project records live only in the index. `project export` writes a note per project into the vault as a backup, and `project import` restores them.

### Onboarding a project

1. Run `knowledge-bae project list` to see what is already registered.
2. Ask for the project's name and its folders. A folder can be a git repo or a parent folder that holds several repos (for example `~/Projects/fantasy-empire`); register the parent rather than each repo by hand. A project can have several folders.
3. Ask which vault it belongs to (`knowledge-bae vault list`), whether specific folders or notes in that vault relate to it, a short description, a project number if a time tracker application tracks it, and anything else worth remembering. Only the name and a folder are required.
4. Register it: `knowledge-bae project add <name> --dir <folder> [--dir <folder>] --vault <vault> [--location <path in vault>] [--description "..."] [--number <n>] [--misc "..."]`. The remote is detected from a folder that is a repo. The commit email is deliberately not stored: it is read from each repo when needed.
   - **When a folder is not a repo**, `project add` scans beneath it (three levels deep, skipping `node_modules`, `target` and repos nested inside repos) and prints the repos it found, all included by default. **Show that list to the user and confirm which repos to include.** To drop some, run `knowledge-bae project edit <name> --exclude <repo>` (the repo's path in the folder, or its folder name when that is unique); `--include <repo>` takes one back. The folder is stored, not a snapshot, so a repo cloned there later is picked up automatically. If no repos are found it says so and keeps the folder.
5. Change a project later with `project edit <name>` (add folders with `--dir`, remove one with `--remove-dir`, `--vault none` to unlink) and remove one with `project remove <name>`.
6. **After every change (add, edit, remove, summary style), ask the user whether to run `knowledge-bae project export`** so the vault has a current backup. Run it if they say yes. A project that has no vault cannot be exported.

### Onboarding the summary style

1. Show the default (`knowledge-bae project summary-style <name>`): a timesheet entry, one short paragraph of 2-3 sentences per project per week.
2. Ask whether that fits or they want their own format, for example a different length, a required prefix such as the project number, or a different tone. Store their own instructions from a file with `knowledge-bae project summary-style <name> --set-file <file>`; `--reset` goes back to the default.
3. Weeks start on Sunday and end on Saturday, in this machine's local time. If the user's week differs, set it with `knowledge-bae project week-start <day>`.
4. Ask whether to run `project export` (see above).

### Producing timesheet entries

1. Run `knowledge-bae project activity [<name>] --week last` (or `this`, or a date to get the week containing it). It prints, per project, the summary instructions, the commits by each repo's own git user, and the vault notes changed that week. For a folder of repos it lists only the repos that had commits (all branches count, not just the checked-out one) and collapses the rest into one `no commits: a, b, c` line, so you can see nothing was missed. Rely on that output rather than inspecting git by hand. For a quick check of which repos were active, run `knowledge-bae project repos <name> --week last`. If a folder reports `no user.email set`, ask the user for the address and rerun with `--author <email>`.
2. Write one entry per project following that project's instructions and the output format the user asked for (a table, plain lines, a pasted block). Use only what the activity shows; do not invent work. If a project had no activity, say so instead of writing an entry.
3. **Show the drafts and wait for the user's approval.** Revise until they are happy. Do not write anything to the vault before that.
4. Once approved, add the entries to that vault's note for the week: `Weekly Summaries/<first day of the week, YYYY-MM-DD>.md`, titled `# Week of <first day> to <last day>`, with a `## <project name>` section per project linked to that vault. If the note exists, add or replace only the project sections you were asked to; never overwrite other content.
5. Run `knowledge-bae ingest --vault <name> --all` so the note is searchable.

## When something is wrong

- **Results are missing or stale:** run `knowledge-bae doctor`. If it reports unindexed notes, run `ingest --all`.
- **A hook points at a binary that no longer exists** (the tool was rebuilt or moved): reinstall with `knowledge-bae hooks install`.
- **Hooks seem not to run:** `knowledge-bae hooks status`, and read `hooks.log`, which sits in the same folder as the index file (`knowledge-bae status` prints the index path).
- **`doctor` warns about a project** (folder not found, origin changed, no vault; it also lists the repos found beneath each parent folder, which is how to spot a newly cloned repo): fix it with `project edit <name>` (`--dir`, `--remove-dir`, `--vault`), then ask about `project export`.
- **The index was lost or deleted:** register the vaults again (`vault add`), run `ingest --all`, then `knowledge-bae project import` to restore the projects from the notes `project export` wrote. Existing projects are kept unless `--overwrite` is given.
- **"Nothing is indexed yet":** no vault has been added or ingested. Follow the onboarding steps.
- **Search is slow the first time in a session:** the embedding model loads on first use, about a second. That is expected.
