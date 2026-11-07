# Projects and Weekly Summaries Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Track projects in the SQLite index and gather each week's work per project, so an agent can write approved timesheet summaries into the vault.

**Architecture:** The index is the only source of truth for project records (no automatic sync with the vault). A new `project` command group manages records and gathers a week's activity (commits per repo plus changed vault notes). `project export` / `project import` write and restore generated project notes in the vault by hand. The CLI never writes prose; the skill has the agent draft summaries, get user approval, then write the weekly note and re-ingest.

**Tech Stack:** Rust 2021, clap, rusqlite, chrono (new, local time only), tempfile for tests, real `git` in tests.

**Spec:** GitHub issue #7 (https://github.com/kkbae-com/knowledge-bae/issues/7) and the vault note `kkbae:knowledge-bae/issues/Issue 7 - Projects and Weekly Summaries.md`.

## Global Constraints

- All code under `src/cli/src/`; schema changes are a new entry appended to `MIGRATIONS` in `store.rs` (never edit migration 1).
- The commit email is per repo and **not stored**: it is read from each repo with `git config user.email` at query time.
- A project has one primary vault (nullable), optional locations inside it, and a project number that is optional free text.
- Week is **Sun-Sat by default**, start day configurable, boundaries in the machine's local time.
- Default summary style: a timesheet entry, one short paragraph of **2-3 sentences** per project per week. A project's own instructions override it; empty means the default.
- Summaries and project notes in the vault are never read back automatically. `project import` is the only reader of project notes and is run by the user.
- After every project change the CLI prints a hint to run `project export`, and the skill asks the user to export.
- No new top-level commands except `project`. Follow existing style: `anyhow::Result`, doc comments on pub fns, `#[cfg(test)] mod tests` in the same file, `tempfile` for repos and vaults.
- Run work on a branch (`feat/projects-and-summaries`), not `main`. One PR for the whole issue.

## Review Focus

- A project directory that is not a git repo or has no `origin` (add succeeds with empty origin; `activity` reports it per directory instead of failing).
- A repo with no `user.email` configured (`activity` says so for that directory and continues; `--author` overrides).
- Week edges: commit at Saturday 23:59 and Sunday 00:00 local; today being the start day; start day set to Monday.
- Project with no vault: `activity` skips notes, `export` skips it with a message, `import` of a note naming an unknown vault keeps the project with no vault and warns.
- `import` when the name already exists (skipped unless `--overwrite`) and a description containing a line like `## Other` (must round-trip).
- Two projects claiming the same directory (clear error).

---

### Task 1: Store - projects and week-start setting

**Files:**
- Modify: `src/cli/src/store.rs` (append migration 2, structs, methods, tests)

**Interfaces:**
- Produces:
  - `pub struct ProjectDir { pub path: String, pub origin: String }`
  - `pub struct Project { pub id: i64, pub name: String, pub description: String, pub vault_id: Option<i64>, pub vault_locations: Vec<String>, pub project_number: String, pub misc: String, pub summary_instructions: String, pub dirs: Vec<ProjectDir> }` (derive `Debug, Clone, PartialEq, Default`; `id` is 0 for a project not yet saved)
  - `Store::add_project(&mut self, p: &Project) -> Result<Project>` (returns it with its id)
  - `Store::update_project(&mut self, p: &Project) -> Result<()>` (replaces all fields and dirs of `p.id`)
  - `Store::find_project(&self, name: &str) -> Result<Option<Project>>`
  - `Store::list_projects(&self) -> Result<Vec<Project>>` (ordered by name)
  - `Store::remove_project(&mut self, id: i64) -> Result<bool>`
  - `Store::week_start(&self) -> Result<String>` (preference `week_start`, default `"sunday"`) and `Store::set_week_start(&self, day: &str) -> Result<()>`

- [ ] **Step 1: Write failing tests** in `store.rs` tests:
  - `projects_round_trip_with_dirs`: add a project with two dirs and `vault_locations = ["a", "b c"]`; `find_project` returns it equal (id filled); `list_projects` is ordered by name.
  - `update_replaces_dirs`: update with one dir; `find_project(..).dirs.len() == 1`.
  - `duplicate_name_and_shared_directory_are_errors`: second project with the same name errors; second project with the same dir path errors and its error text contains the path.
  - `removing_a_vault_unlinks_its_projects`: after `remove_vault`, `find_project(..).vault_id == None` and the project still exists.
  - `remove_project_deletes_its_dirs`.
  - `week_start_defaults_to_sunday_and_persists`.
- [ ] **Step 2: Run** `cd src/cli && cargo test store::` - Expected: compile errors / FAIL.
- [ ] **Step 3: Implement** migration 2: `projects` (id, name UNIQUE, description, vault_id `REFERENCES vaults(id) ON DELETE SET NULL`, vault_locations JSON text, project_number, misc, summary_instructions) and `project_dirs` (project_id `ON DELETE CASCADE`, path UNIQUE, origin). Use the existing `json()` helper for locations; wrap add/update in a transaction. Week start reuses the existing `preferences` table.
- [ ] **Step 4: Run** `cargo test store::` - Expected: PASS, including `reopening_a_file_keeps_data_and_schema_version` (update its expected `user_version` to 2).
- [ ] **Step 5: Commit** `feat: store projects and week start setting`

### Task 2: Week bounds

**Files:**
- Create: `src/cli/src/week.rs`
- Modify: `src/cli/Cargo.toml` (add `chrono = { version = "0.4", default-features = false, features = ["clock"] }`; confirm the version resolves with `cargo add`), `src/cli/src/main.rs` (`mod week;`)

**Interfaces:**
- Produces:
  - `pub enum WeekSel { This, Last, Containing(NaiveDate) }`
  - `pub fn parse_weekday(s: &str) -> Result<chrono::Weekday>` (case-insensitive full names `sunday`..`saturday`; clear error listing valid names)
  - `pub fn week_bounds(today: NaiveDate, start: Weekday, sel: WeekSel) -> (NaiveDate, NaiveDate)` (first and last day, inclusive, 7 days)
  - `pub fn today() -> NaiveDate` (machine local date)

- [ ] **Step 1: Write failing tests**: with `start = Sunday`, `today = 2026-11-07` (Saturday): `This -> (2026-11-01, 2026-11-07)`, `Last -> (2026-10-25, 2026-10-31)`; `today = 2026-11-08` (Sunday): `This -> (2026-11-08, 2026-11-14)`; `start = Monday`, `today = 2026-11-08`: `This -> (2026-11-02, 2026-11-08)`; `Containing(2026-11-03)` with Sunday start `-> (2026-11-01, 2026-11-07)`; `parse_weekday("Monday")` ok, `parse_weekday("funday")` errors mentioning `sunday`.
- [ ] **Step 2: Run** `cargo test week::` - Expected: FAIL (module missing).
- [ ] **Step 3: Implement** the three functions in `week.rs`: step back `(today.weekday() - start) mod 7` days to find the week start; `Last` is the previous week.
- [ ] **Step 4: Run** `cargo test week::` - Expected: PASS.
- [ ] **Step 5: Commit** `feat: week bounds with configurable start day`

### Task 3: Git activity helpers

**Files:**
- Modify: `src/cli/src/git.rs` (functions and tests)

**Interfaces:**
- Consumes: `run_git` (existing, private to `git.rs`)
- Produces:
  - `pub fn is_repo(dir: &Path) -> bool`
  - `pub fn origin_url(dir: &Path) -> Option<String>` (`git remote get-url origin`, `None` if absent or not a repo)
  - `pub fn user_email(dir: &Path) -> Option<String>` (`git config user.email`, `None` if unset)
  - `pub struct Commit { pub hash: String, pub date: String, pub subject: String }` (`hash` is 7 chars, `date` is `YYYY-MM-DD` local)
  - `pub fn commits_by(dir: &Path, email: &str, first: NaiveDate, last: NaiveDate) -> Result<Vec<Commit>>` (all branches, no merges, author exactly `email`, inclusive local-day range, oldest first)
  - `pub fn paths_changed(dir: &Path, first: NaiveDate, last: NaiveDate, locations: &[String]) -> Result<Vec<String>>` (distinct files touched by any commit in the range, limited to `locations` when non-empty, sorted)

- [ ] **Step 1: Write failing tests** using temp repos and `GIT_AUTHOR_DATE`/`GIT_COMMITTER_DATE` env on `git commit`:
  - `commits_by_filters_by_author_and_inclusive_day_range`: commits at `2026-10-31T23:59:00`, `2026-11-01T00:00:00`, `2026-11-07T23:59:00`, `2026-11-08T00:00:00` by the user, plus one by `other@x` inside the range; range `2026-11-01..=2026-11-07` returns exactly the two user commits inside it, oldest first.
  - `paths_changed_respects_locations`: commits touching `a/x.md` and `b/y.md`; `locations = ["a"]` returns only `a/x.md`; empty locations returns both.
  - `origin_and_email_are_none_when_missing`: fresh repo returns `None, None` for origin (email may come from global config, so only assert origin); a plain temp dir has `is_repo == false`.
- [ ] **Step 2: Run** `cargo test git::` - Expected: FAIL.
- [ ] **Step 3: Implement** with `git log --all --no-merges --author=^<regex-escaped email>$ --since=<first>T00:00:00 --until=<last>T23:59:59 --format=%h%x00%ad%x00%s --date=format-local:%Y-%m-%d` (no timezone suffix, so git reads the bounds in local time); `paths_changed` uses `git log --name-only --format=` with `-- <locations>` pathspecs.
- [ ] **Step 4: Run** `cargo test git::` - Expected: PASS.
- [ ] **Step 5: Commit** `feat: git helpers for per-repo activity`

### Task 4: `project` commands (add, list, show, edit, remove, summary-style, week-start)

**Files:**
- Create: `src/cli/src/project.rs` (command logic), `src/cli/defaults/summary-style.md`
- Modify: `src/cli/src/main.rs` (`mod project;`, `Command::Project { action: ProjectAction }`, clap enum, dispatch)

**Interfaces:**
- Consumes: Task 1 store API, `git::{is_repo, origin_url}`, `week::parse_weekday`, existing `resolve_vault`/`find_vault` in `main.rs` (make them `pub(crate)` or pass the resolved `Vault` in)
- Produces:
  - `pub const DEFAULT_SUMMARY_STYLE: &str = include_str!("../defaults/summary-style.md")` - instructions that state the 2-3 sentence timesheet paragraph rule
  - `pub fn effective_style(p: &Project) -> &str` (project's own instructions, else the default)
  - `project add <name> --dir <path>... [--description T] [--vault V] [--location L]... [--number N] [--misc T]`: stores origin per dir via `origin_url` (empty if none), stores absolute paths, errors if a dir does not exist
  - `project list` (one line per project: name, number if any, vault, dir count), `project show <name>` (all fields incl. effective style)
  - `project edit <name>` with the same optional flags (`--dir` / `--location` add; `--remove-dir P` removes; `--vault none` unlinks)
  - `project remove <name>`
  - `project summary-style <name> [--set-file F | --reset]`: prints the effective instructions, sets them from a file, or clears them back to the default
  - `project week-start [DAY]`: prints or sets the start day via `parse_weekday`
  - every mutating command ends with: `Tip: run \`knowledge-bae project export\` to save this in the vault.`

- [ ] **Step 1: Write failing tests** in `project.rs` against `Store::open_in_memory` and temp dirs, calling the pure functions behind the commands (`add_project(store, args) -> Result<Project>`, etc.): add records absolute dir + empty origin for a non-repo dir; add with a real repo with `origin` records its URL; add with a missing dir errors; `effective_style` returns the default when empty and the override when set; `--reset` restores the default; `week-start` rejects `funday`; the default style text contains `2-3 sentences` (pin the rule).
- [ ] **Step 2: Run** `cargo test project::` - Expected: FAIL.
- [ ] **Step 3: Implement** the functions and wire clap; `vault` flag resolves through `find_vault` and stores `vault_id`.
- [ ] **Step 4: Run** `cargo test` - Expected: PASS. Then smoke test: `cargo run -- project add demo --dir . --vault kkbae` against a scratch `KNOWLEDGE_BAE_DB`, then `project show demo`.
- [ ] **Step 5: Commit** `feat: project commands`

### Task 5: `project activity`

**Files:**
- Modify: `src/cli/src/project.rs`, `src/cli/src/main.rs` (clap variant)

**Interfaces:**
- Consumes: Tasks 1-4, `git::{commits_by, paths_changed, user_email, is_repo}`, `week::{week_bounds, today, parse_weekday, WeekSel}`
- Produces:
  - `project activity [NAME] [--week this|last|YYYY-MM-DD] [--author EMAIL]` (default week: `last`; no name means every project)
  - `pub fn render_activity(store: &Store, p: &Project, first: NaiveDate, last: NaiveDate, author: Option<&str>) -> Result<String>` returning markdown: a `## <name>` heading with project number, the week range, the effective summary instructions, then per directory `### Commits: <dir> (<email>)` with `- DATE HASH subject` lines (or `- not a git repository` / `- no user.email set; pass --author`), then `### Vault notes changed: <vault>` with one path per line (omitted when the project has no vault)
  - Output is facts only; no prose summary

- [ ] **Step 1: Write failing tests** with a temp repo and temp vault repo: `render_activity` lists the in-range commit by the repo's email and omits the out-of-range one; a non-repo dir produces the `not a git repository` line and still renders other dirs; a repo without email (set `user.email` to empty via `git config`, or pass an env-isolated config) and no `--author` produces the `no user.email set` line; vault notes section lists a changed note and is absent for a project without a vault; heading includes the project number when set.
- [ ] **Step 2: Run** `cargo test project::` - Expected: FAIL.
- [ ] **Step 3: Implement** `render_activity` and the command (resolve `week_start` from the store, `--week` parsed as `this`/`last`/date).
- [ ] **Step 4: Run** `cargo test` - Expected: PASS. Smoke test against this repo with a project pointing at `.`.
- [ ] **Step 5: Commit** `feat: project activity report for a week`

### Task 6: `project export` / `project import`

**Files:**
- Create: `src/cli/src/project_notes.rs`
- Modify: `src/cli/src/main.rs` (`mod project_notes;`, clap variants)

**Interfaces:**
- Consumes: Task 1 store API, `Vault`
- Produces:
  - `pub fn render_note(p: &Project, vault_name: Option<&str>) -> String`: markdown starting `# Project: <name>`, a line marking it generated by `knowledge-bae project export` (restore with `project import`), then sections `## Description`, `## Details` (bullets: vault, locations, project number), `## Directories` (bullets: `path` and origin), `## Misc`, `## Summary instructions`
  - `pub struct ParsedProject { pub project: Project, pub vault_name: Option<String> }` and `pub fn parse_note(text: &str) -> Result<ParsedProject>`: only the exact section headings above split the text, so a `## Other` line inside a description is preserved
  - `project export [--vault V]`: writes `<vault>/Projects/<name>.md` for each project linked to the vault (all vaults if omitted), creating the folder; projects with no vault are listed as skipped; then runs `ingest --all` for those vaults so the notes are searchable
  - `project import [--vault V] [--overwrite]`: reads `<vault>/Projects/*.md`, upserts by name, skips existing names unless `--overwrite` and says which; an unknown vault name imports the project unlinked with a warning

- [ ] **Step 1: Write failing tests**: `note_round_trips` (render then parse equals the original for a project with multi-line description, a description containing `## Other`, two dirs, locations, number, misc, style); `export_then_import_restores_after_losing_the_index` (export to a temp vault, build a fresh in-memory store with the vault registered, import, `list_projects` equals the originals); `import_skips_existing_unless_overwrite`; `import_with_unknown_vault_keeps_project_unlinked`; `export_skips_projects_without_a_vault`.
- [ ] **Step 2: Run** `cargo test project_notes::` - Expected: FAIL.
- [ ] **Step 3: Implement** render/parse/export/import; ordering in `render_note` is fixed so repeated exports produce identical files (no churn in the vault's git history).
- [ ] **Step 4: Run** `cargo test` - Expected: PASS. Smoke test: export, delete the scratch DB, re-add the vault, import, `project list`.
- [ ] **Step 5: Commit** `feat: project export and import`

### Task 7: Doctor checks for projects

**Files:**
- Modify: `src/cli/src/doctor.rs`, `src/cli/src/main.rs` (`doctor_command` prints a "Projects" section)

**Interfaces:**
- Consumes: Task 1 store API, `git::{is_repo, origin_url}`
- Produces: `pub fn check_projects(store: &Store) -> Vec<Check>`: `Info` when none registered; per project `Warn` for a directory that no longer exists, a directory that is not a git repo while an origin is stored, a stored origin that differs from the repo's current origin, or a vault link that is gone (`vault_id == None` while `project export` would skip it); `Ok` line when all fine

- [ ] **Step 1: Write failing tests**: no projects gives one `Info`; a project whose dir is deleted gives a `Warn` naming the path; a project with no vault gives a `Warn` naming the project; a healthy project gives `Ok`.
- [ ] **Step 2: Run** `cargo test doctor::` - Expected: FAIL.
- [ ] **Step 3: Implement** `check_projects` and call it from `doctor_command`.
- [ ] **Step 4: Run** `cargo test` - Expected: PASS.
- [ ] **Step 5: Commit** `feat: doctor checks projects`

### Task 8: Skill and docs

**Files:**
- Modify: `skills/knowledge-bae/SKILL.md`, `README.md`

**Interfaces:**
- Consumes: the final command names and flags from Tasks 4-7 (check each with `--help` before writing them down)

- [ ] **Step 1: Update the skill** (keep the existing frontmatter test passing; add "projects, weekly summaries, timesheet entries" to the `description` so Claude picks the skill for timesheet requests):
  - **Onboarding projects**: ask for the directories (a project can span several repos), infer the remote, ask for description, which vault it belongs to and optional locations, optional project number and misc; run `project add`; then **ask the user whether to run `project export`** now.
  - **Onboarding summaries**: show the default (timesheet paragraph, 2-3 sentences), ask if it fits or the user wants their own format; store with `project summary-style <name> --set-file`; ask about the week start day only if the user does not use Sun-Sat (`project week-start`); ask to export.
  - **Producing timesheet entries**: run `project activity [name] --week ...`; draft one entry per project following the effective instructions and the format the user asks for; **show the drafts and wait for approval**; only then write the vault's weekly note (`Weekly Summaries/<week start date>.md`, one section per project linked to that vault) and run `ingest --vault <name> --all`. If a commit directory has no email, ask the user for the address.
  - **After every project change** (add, edit, remove, summary-style): ask the user whether to run `project export`.
  - Add `project`-related items to the prerequisites and "When something is wrong" (doctor project warnings, `project import` to recover from a lost index).
- [ ] **Step 2: Update the README** with a short "Projects and weekly summaries" section covering the commands, the default summary style, and the export/import backup model.
- [ ] **Step 3: Verify** `cargo test` (skill frontmatter test), `cargo run -- project --help`, and that every command and flag named in `SKILL.md` appears in `--help` output.
- [ ] **Step 4: Run an end-to-end pass** in a scratch DB: add a project for this repo, run `project activity --week last`, export, delete the DB, import, `doctor` shows no project warnings.
- [ ] **Step 5: Commit** `docs: skill and README for projects and weekly summaries`

---

## Self-Review

- **Spec coverage:** record fields (Tasks 1, 4), per-repo email not stored (Tasks 3, 5), Sun-Sat configurable local-time week (Tasks 2, 4), default 2-3 sentence style with overrides (Task 4), activity gathering (Task 5), index as sole source of truth with manual export/import (Task 6), summaries written to the vault after approval and one note per vault per week (Task 8, skill), export prompt after every change (Task 4 hint, Task 8 skill), doctor (Task 7), skill onboarding (Task 8).
- **Assumption to confirm:** "check the git history" is implemented as reading each repo's `git config user.email` and filtering `git log` by it, with `--author` as an override. If you meant detecting the email from past commits instead, only `git::user_email` changes.
- **Open layout:** the note layouts are fixed in Task 6 and Task 8 only as far as the information goes, per your note that exact layout does not matter.
