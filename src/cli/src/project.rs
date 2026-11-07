//! Project records: adding and editing them, and the summary style that applies to each.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use chrono::NaiveDate;

use crate::git;
use crate::repos::{self, Folder, Repo};
use crate::store::{Project, ProjectDir, Store};
use crate::week;

/// How weekly summaries are written when a project has no instructions of its own.
pub const DEFAULT_SUMMARY_STYLE: &str = include_str!("../defaults/summary-style.md");

/// Printed after every change so the user remembers to back the records up to the vault.
pub const EXPORT_TIP: &str = "Tip: run `knowledge-bae project export` to save this in the vault.";

pub struct AddArgs {
    pub name: String,
    pub dirs: Vec<PathBuf>,
    pub description: Option<String>,
    pub vault_id: Option<i64>,
    pub locations: Vec<String>,
    pub number: Option<String>,
    pub misc: Option<String>,
    /// Repos beneath the folders to leave out (relative path or unique folder name).
    pub excludes: Vec<String>,
}

/// What `project edit --vault` asks for.
pub enum VaultChange {
    Keep,
    Unlink,
    Set(i64),
}

pub struct EditArgs {
    pub name: String,
    pub add_dirs: Vec<PathBuf>,
    pub remove_dirs: Vec<PathBuf>,
    pub description: Option<String>,
    pub vault: VaultChange,
    pub add_locations: Vec<String>,
    pub number: Option<String>,
    pub misc: Option<String>,
    pub excludes: Vec<String>,
    /// Repos to take back out of the exclude list.
    pub includes: Vec<String>,
}

/// The project's own summary instructions, or the built-in default when it has none.
pub fn effective_style(p: &Project) -> &str {
    if p.summary_instructions.trim().is_empty() {
        DEFAULT_SUMMARY_STYLE
    } else {
        &p.summary_instructions
    }
}

pub fn add(store: &mut Store, a: AddArgs) -> Result<Project> {
    let mut dirs = a.dirs.iter().map(|d| dir_entry(d)).collect::<Result<Vec<_>>>()?;
    apply_excludes(&mut dirs, &a.excludes)?;
    store.add_project(&Project {
        name: a.name,
        description: a.description.unwrap_or_default(),
        vault_id: a.vault_id,
        vault_locations: a.locations,
        project_number: a.number.unwrap_or_default(),
        misc: a.misc.unwrap_or_default(),
        dirs,
        ..Project::default()
    })
}

pub fn edit(store: &mut Store, a: EditArgs) -> Result<Project> {
    let mut p = require_project(store, &a.name)?;
    for dir in &a.remove_dirs {
        let path = absolute(dir)?;
        let before = p.dirs.len();
        p.dirs.retain(|d| d.path != path);
        if p.dirs.len() == before {
            bail!("{path} is not one of the directories of '{}'", p.name);
        }
    }
    for dir in &a.add_dirs {
        p.dirs.push(dir_entry(dir)?);
    }
    apply_includes(&mut p.dirs, &a.includes)?;
    apply_excludes(&mut p.dirs, &a.excludes)?;
    if let Some(description) = a.description {
        p.description = description;
    }
    match a.vault {
        VaultChange::Keep => {}
        VaultChange::Unlink => p.vault_id = None,
        VaultChange::Set(id) => p.vault_id = Some(id),
    }
    for location in a.add_locations {
        if !p.vault_locations.contains(&location) {
            p.vault_locations.push(location);
        }
    }
    if let Some(number) = a.number {
        p.project_number = number;
    }
    if let Some(misc) = a.misc {
        p.misc = misc;
    }
    store.update_project(&p)?;
    Ok(p)
}

/// Sets the project's own summary instructions, or with `None` goes back to the default.
pub fn set_summary_style(store: &mut Store, name: &str, text: Option<String>) -> Result<Project> {
    let mut p = require_project(store, name)?;
    p.summary_instructions = text.unwrap_or_default();
    store.update_project(&p)?;
    Ok(p)
}

/// Validates and stores the first day of the week; returns the stored (lowercase) name.
pub fn set_week_start(store: &Store, day: &str) -> Result<String> {
    week::parse_weekday(day)?;
    let day = day.trim().to_lowercase();
    store.set_week_start(&day)?;
    Ok(day)
}

/// Everything about a project, for `project show`.
pub fn describe(p: &Project, vault_name: Option<&str>) -> String {
    let or_none = |s: &str| if s.trim().is_empty() { "(none)".to_string() } else { s.to_string() };
    let mut out = format!("Project: {}\n", p.name);
    out.push_str(&format!("Description: {}\n", or_none(&p.description)));
    out.push_str(&format!("Project number: {}\n", or_none(&p.project_number)));
    out.push_str(&format!("Vault: {}\n", vault_name.unwrap_or("(none)")));
    out.push_str(&format!("Vault locations: {}\n", or_none(&p.vault_locations.join(", "))));
    out.push_str("Directories:\n");
    for d in &p.dirs {
        let origin = if d.origin.is_empty() { "no origin".to_string() } else { d.origin.clone() };
        out.push_str(&format!("  {} ({origin})\n", d.path));
    }
    out.push_str(&format!("Misc: {}\n", or_none(&p.misc)));
    out.push_str("Summary instructions:\n");
    for line in effective_style(p).lines() {
        out.push_str(&format!("  {line}\n"));
    }
    out
}

/// What was found beneath each folder that is not a repo itself, one line per folder, for `add` and `edit`.
pub fn scan_summary(p: &Project) -> Vec<String> {
    let plural = |n: usize| if n == 1 { "1 repo".to_string() } else { format!("{n} repos") };
    p.dirs
        .iter()
        .filter_map(|dir| match repos::classify(dir) {
            Folder::Parent { repos, excluded } if repos.is_empty() && excluded.is_empty() => {
                Some(format!("No git repositories found under {}; keeping the folder anyway.", dir.path))
            }
            Folder::Parent { repos, excluded } => {
                let names: Vec<_> = repos.iter().map(|r| r.name.as_str()).collect();
                let left_out = if excluded.is_empty() {
                    "all included".to_string()
                } else {
                    format!("excluded: {}", excluded.join(", "))
                };
                Some(format!("Found {} under {}: {} ({left_out})", plural(names.len()), dir.path, names.join(", ")))
            }
            _ => None,
        })
        .collect()
}

/// Which of the project's repos had commits in the week, one line each.
pub fn render_repos(p: &Project, first: NaiveDate, last: NaiveDate, author: Option<&str>) -> String {
    let mut out = format!("## {}: repos, {first} to {last} (all branches)\n", p.name);
    for dir in &p.dirs {
        out.push_str(&format!("{}\n", dir.path));
        let (repos, excluded) = match repos::classify(dir) {
            Folder::Missing => {
                out.push_str("  folder not found\n");
                continue;
            }
            Folder::Repo => (vec![Repo { name: "(the folder itself)".into(), path: PathBuf::from(&dir.path) }], vec![]),
            Folder::Parent { repos, excluded } => (repos, excluded),
        };
        if repos.is_empty() && excluded.is_empty() {
            out.push_str("  no git repositories found beneath it\n");
        }
        for w in repo_weeks(&repos, author, first, last) {
            let line = match (&w.email, &w.commits) {
                (None, _) => format!("no-email {} (set user.email or pass --author)", w.name),
                (Some(_), Err(e)) => format!("error    {} ({e})", w.name),
                (Some(_), Ok(c)) if c.is_empty() => format!("idle     {}", w.name),
                (Some(_), Ok(c)) => format!("active   {} ({} commit{})", w.name, c.len(), if c.len() == 1 { "" } else { "s" }),
            };
            out.push_str(&format!("  {line}\n"));
        }
        for name in excluded {
            out.push_str(&format!("  excluded {name}\n"));
        }
    }
    out
}

/// One repo's commits for the week.
struct RepoWeek {
    name: String,
    email: Option<String>,
    commits: std::result::Result<Vec<git::Commit>, String>,
}

/// The week's commits for each repo, a few repos at a time so a folder with many repos stays quick.
/// Every local and remote branch counts, not only the checked-out one.
fn repo_weeks(repos: &[Repo], author: Option<&str>, first: NaiveDate, last: NaiveDate) -> Vec<RepoWeek> {
    let mut out = Vec::new();
    for chunk in repos.chunks(8) {
        std::thread::scope(|scope| {
            let workers: Vec<_> = chunk
                .iter()
                .map(|repo| {
                    scope.spawn(move || {
                        let email = author.map(str::to_string).or_else(|| git::user_email(&repo.path));
                        let commits = match &email {
                            Some(e) => git::commits_by(&repo.path, e, first, last).map_err(|e| format!("{e:#}")),
                            None => Ok(Vec::new()),
                        };
                        RepoWeek { name: repo.name.clone(), email, commits }
                    })
                })
                .collect();
            for worker in workers {
                out.push(worker.join().expect("a git worker panicked"));
            }
        });
    }
    out
}

/// The commits section for a folder that holds repos: only repos with commits get their own list.
fn render_parent(
    dir: &ProjectDir,
    repos: &[Repo],
    excluded: &[String],
    author: Option<&str>,
    first: NaiveDate,
    last: NaiveDate,
) -> String {
    if repos.is_empty() && excluded.is_empty() {
        return format!("### Commits: {}\n- not a git repository, and no repos were found beneath it\n", dir.path);
    }
    let mut out = format!("### Commits: {} (repos beneath it, all branches)\n", dir.path);
    let (mut idle, mut no_email) = (Vec::new(), Vec::new());
    for w in repo_weeks(repos, author, first, last) {
        match (&w.email, &w.commits) {
            (None, _) => no_email.push(w.name),
            (Some(_), Ok(c)) if c.is_empty() => idle.push(w.name),
            (Some(email), Ok(commits)) => {
                out.push_str(&format!("#### {} ({email})\n", w.name));
                for c in commits {
                    out.push_str(&format!("- {} {} {}\n", c.date, c.hash, c.subject));
                }
            }
            (Some(email), Err(e)) => out.push_str(&format!("#### {} ({email})\n- could not read git history: {e}\n", w.name)),
        }
    }
    if !idle.is_empty() {
        out.push_str(&format!("- no commits: {}\n", idle.join(", ")));
    }
    if !no_email.is_empty() {
        out.push_str(&format!("- no user.email set (pass --author <email>): {}\n", no_email.join(", ")));
    }
    if !excluded.is_empty() {
        out.push_str(&format!("- excluded: {}\n", excluded.join(", ")));
    }
    out
}

/// A markdown report of one project's week: facts only, for the agent to turn into a summary.
/// `author` overrides the commit email that is otherwise read from each repo.
pub fn render_activity(
    store: &Store,
    p: &Project,
    first: NaiveDate,
    last: NaiveDate,
    author: Option<&str>,
) -> Result<String> {
    let mut out = format!("## {}", p.name);
    if !p.project_number.is_empty() {
        out.push_str(&format!(" (project number {})", p.project_number));
    }
    out.push_str(&format!("\n\nWeek: {first} to {last}\n\nSummary instructions:\n"));
    for line in effective_style(p).lines() {
        out.push_str(&format!("  {line}\n"));
    }
    for dir in &p.dirs {
        out.push('\n');
        match repos::classify(dir) {
            Folder::Parent { repos, excluded } => out.push_str(&render_parent(dir, &repos, &excluded, author, first, last)),
            Folder::Repo | Folder::Missing => {
                let email = author.map(str::to_string).or_else(|| git::user_email(Path::new(&dir.path)));
                out.push_str(&render_commits(dir, email.as_deref(), first, last));
            }
        }
    }
    if let Some(vault) = store.list_vaults()?.into_iter().find(|v| Some(v.id) == p.vault_id) {
        out.push_str(&format!("\n### Vault notes changed: {}\n", vault.name));
        match git::paths_changed(Path::new(&vault.path), first, last, &p.vault_locations) {
            Ok(paths) if paths.is_empty() => out.push_str("- none\n"),
            Ok(paths) => paths.iter().for_each(|path| out.push_str(&format!("- {path}\n"))),
            Err(e) => out.push_str(&format!("- could not read the vault's git history: {e:#}\n")),
        }
    }
    Ok(out)
}

/// The commits section for one folder. `email` is the author to look for, if known.
fn render_commits(dir: &ProjectDir, email: Option<&str>, first: NaiveDate, last: NaiveDate) -> String {
    let path = Path::new(&dir.path);
    let mut out = match email {
        Some(email) if path.exists() && git::is_repo(path) => format!("### Commits: {} ({email}, all branches)\n", dir.path),
        _ => format!("### Commits: {}\n", dir.path),
    };
    let line = if !path.exists() {
        "- folder not found".to_string()
    } else if !git::is_repo(path) {
        "- not a git repository".to_string()
    } else if let Some(email) = email {
        match git::commits_by(path, email, first, last) {
            Ok(commits) if commits.is_empty() => "- no commits".to_string(),
            Ok(commits) => commits
                .iter()
                .map(|c| format!("- {} {} {}", c.date, c.hash, c.subject))
                .collect::<Vec<_>>()
                .join("\n"),
            Err(e) => format!("- could not read git history: {e:#}"),
        }
    } else {
        "- no user.email set in this repo; pass --author <email>".to_string()
    };
    out.push_str(&line);
    out.push('\n');
    out
}

pub fn require_project(store: &Store, name: &str) -> Result<Project> {
    store
        .find_project(name)?
        .with_context(|| format!("no project named '{name}' (see: knowledge-bae project list)"))
}

/// The repos beneath a folder that is not a repo itself (none for a repo or a missing folder).
fn repo_names_beneath(dir: &ProjectDir) -> Vec<String> {
    let root = Path::new(&dir.path);
    if !root.is_dir() || git::is_repo(root) {
        return Vec::new();
    }
    repos::find_repos(root)
}

/// Leaves the named repos out of the folders that hold them; a name found nowhere is an error.
fn apply_excludes(dirs: &mut [ProjectDir], wanted: &[String]) -> Result<()> {
    for name in wanted {
        let mut found_any = false;
        for dir in dirs.iter_mut() {
            let names = repo_names_beneath(dir);
            if let Some(rel) = repos::resolve_name(&names, name) {
                if !dir.excludes.iter().any(|e| e == rel) {
                    dir.excludes.push(rel.to_string());
                }
                found_any = true;
            }
        }
        if !found_any {
            let all: Vec<String> = dirs.iter().flat_map(repo_names_beneath).collect();
            let found = if all.is_empty() { "none".to_string() } else { all.join(", ") };
            bail!("no repo called '{name}' found beneath the project's folders (found: {found})");
        }
    }
    Ok(())
}

/// Takes the named repos back out of the exclude lists; a name that is not excluded is an error.
fn apply_includes(dirs: &mut [ProjectDir], wanted: &[String]) -> Result<()> {
    for name in wanted {
        let mut found_any = false;
        for dir in dirs.iter_mut() {
            if let Some(rel) = repos::resolve_name(&dir.excludes, name).map(str::to_string) {
                dir.excludes.retain(|e| *e != rel);
                found_any = true;
            }
        }
        if !found_any {
            bail!("'{name}' is not excluded from this project");
        }
    }
    Ok(())
}

/// One spelling per folder, so the same folder cannot be registered twice: symlinks, `..` and a
/// trailing slash are resolved when the folder exists, and the slash is dropped when it does not.
fn absolute(dir: &Path) -> Result<String> {
    let path = match dir.canonicalize() {
        Ok(path) => path,
        Err(_) => std::path::absolute(dir)?.components().collect(),
    };
    Ok(path.to_string_lossy().into_owned())
}

/// An existing directory as a project directory, with the origin of the repo it is in (if any).
fn dir_entry(dir: &Path) -> Result<ProjectDir> {
    if !dir.is_dir() {
        bail!("{} is not a directory", dir.display());
    }
    Ok(ProjectDir { path: absolute(dir)?, origin: git::origin_url(dir).unwrap_or_default(), ..ProjectDir::default() })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn add_args(name: &str, dirs: Vec<PathBuf>) -> AddArgs {
        AddArgs {
            name: name.into(),
            dirs,
            description: None,
            vault_id: None,
            locations: vec![],
            number: None,
            misc: None,
            excludes: vec![],
        }
    }

    fn edit_args(name: &str) -> EditArgs {
        EditArgs {
            name: name.into(),
            add_dirs: vec![],
            remove_dirs: vec![],
            description: None,
            vault: VaultChange::Keep,
            add_locations: vec![],
            number: None,
            misc: None,
            excludes: vec![],
            includes: vec![],
        }
    }

    #[test]
    fn add_records_the_absolute_directory_and_no_origin_for_a_plain_folder() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open_in_memory().unwrap();
        let p = add(&mut store, add_args("p", vec![dir.path().to_path_buf()])).unwrap();
        assert_eq!(p.dirs.len(), 1);
        assert!(Path::new(&p.dirs[0].path).is_absolute());
        assert_eq!(p.dirs[0].origin, "");
        assert_eq!(store.find_project("p").unwrap(), Some(p));
    }

    #[test]
    fn add_records_the_origin_of_a_repo() {
        let dir = tempfile::tempdir().unwrap();
        for args in [&["init", "-q"][..], &["remote", "add", "origin", "git@example.com:me/r.git"]] {
            assert!(Command::new("git").arg("-C").arg(dir.path()).args(args).status().unwrap().success());
        }
        let mut store = Store::open_in_memory().unwrap();
        let p = add(&mut store, add_args("p", vec![dir.path().to_path_buf()])).unwrap();
        assert_eq!(p.dirs[0].origin, "git@example.com:me/r.git");
    }

    #[test]
    fn add_with_a_missing_directory_is_an_error_and_stores_nothing() {
        let mut store = Store::open_in_memory().unwrap();
        let err = add(&mut store, add_args("p", vec![PathBuf::from("/definitely/not/here")])).unwrap_err();
        assert!(format!("{err:#}").contains("/definitely/not/here"), "{err:#}");
        assert!(store.find_project("p").unwrap().is_none());
    }

    #[test]
    fn the_same_folder_spelled_differently_is_one_folder() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("sub");
        std::fs::create_dir(&sub).unwrap();
        let with_slash = PathBuf::from(format!("{}/", dir.path().display()));
        let roundabout = sub.join("..");
        let mut store = Store::open_in_memory().unwrap();
        let a = add(&mut store, add_args("a", vec![with_slash])).unwrap();
        assert!(!a.dirs[0].path.ends_with('/'), "{}", a.dirs[0].path);
        assert!(!a.dirs[0].path.contains(".."));

        assert!(add(&mut store, add_args("b", vec![dir.path().to_path_buf()])).is_err());
        assert!(add(&mut store, add_args("c", vec![roundabout])).is_err());

        let mut e = edit_args("a");
        e.remove_dirs = vec![dir.path().to_path_buf()];
        e.add_dirs = vec![sub];
        assert_eq!(edit(&mut store, e).unwrap().dirs.len(), 1);
    }

    #[test]
    fn edit_adds_and_removes_directories_and_changes_fields() {
        let (a, b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let vault_dir = tempfile::tempdir().unwrap();
        let mut store = Store::open_in_memory().unwrap();
        let vault = store.add_vault("v", &vault_dir.path().to_string_lossy(), "").unwrap();
        add(&mut store, add_args("p", vec![a.path().to_path_buf()])).unwrap();

        let mut e = edit_args("p");
        e.add_dirs = vec![b.path().to_path_buf()];
        e.remove_dirs = vec![a.path().to_path_buf()];
        e.vault = VaultChange::Set(vault.id);
        e.add_locations = vec!["work/p".into()];
        e.number = Some("P-9".into());
        let p = edit(&mut store, e).unwrap();
        assert_eq!(p.dirs.len(), 1);
        assert!(p.dirs[0].path.ends_with(b.path().file_name().unwrap().to_str().unwrap()));
        assert_eq!((p.vault_id, p.vault_locations, p.project_number.as_str()), (Some(vault.id), vec!["work/p".to_string()], "P-9"));

        let mut unlink = edit_args("p");
        unlink.vault = VaultChange::Unlink;
        assert_eq!(edit(&mut store, unlink).unwrap().vault_id, None);
    }

    #[test]
    fn the_default_style_is_a_two_to_three_sentence_timesheet_paragraph() {
        assert!(DEFAULT_SUMMARY_STYLE.contains("2-3 sentences"));
        let mut p = Project::default();
        assert_eq!(effective_style(&p), DEFAULT_SUMMARY_STYLE);
        p.summary_instructions = "one line only".into();
        assert_eq!(effective_style(&p), "one line only");
    }

    #[test]
    fn summary_style_can_be_set_then_reset() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open_in_memory().unwrap();
        add(&mut store, add_args("p", vec![dir.path().to_path_buf()])).unwrap();
        let p = set_summary_style(&mut store, "p", Some("terse".into())).unwrap();
        assert_eq!(effective_style(&p), "terse");
        let p = set_summary_style(&mut store, "p", None).unwrap();
        assert_eq!(effective_style(&p), DEFAULT_SUMMARY_STYLE);
        assert!(set_summary_style(&mut store, "missing", None).is_err());
    }

    #[test]
    fn week_start_is_validated_and_stored_lowercase() {
        let store = Store::open_in_memory().unwrap();
        assert!(set_week_start(&store, "funday").is_err());
        assert_eq!(store.week_start().unwrap(), "sunday");
        assert_eq!(set_week_start(&store, "Monday").unwrap(), "monday");
        assert_eq!(store.week_start().unwrap(), "monday");
    }

    #[test]
    fn describe_shows_every_field_and_the_effective_style() {
        let p = Project {
            name: "p".into(),
            description: "does things".into(),
            project_number: "P-1".into(),
            misc: "note".into(),
            vault_locations: vec!["work/p".into()],
            dirs: vec![ProjectDir { path: "/p".into(), origin: "git@x:p.git".into(), ..ProjectDir::default() }],
            ..Project::default()
        };
        let text = describe(&p, Some("kkbae"));
        for needle in ["p", "does things", "P-1", "note", "work/p", "/p", "git@x:p.git", "kkbae", "2-3 sentences"] {
            assert!(text.contains(needle), "missing {needle}: {text}");
        }
    }

    fn day(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    fn git_in(dir: &Path, args: &[&str]) {
        let out = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["-c", "user.name=t", "-c", "commit.gpgsign=false"])
            .args(args)
            .env("GIT_AUTHOR_DATE", "2026-11-03T10:00:00")
            .env("GIT_COMMITTER_DATE", "2026-11-03T10:00:00")
            .output()
            .unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    }

    /// A repo whose only commit is dated 2026-11-03, plus an earlier one on 2026-10-01.
    fn repo_with_commits(email: &str, file: &str, subject: &str) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        make_repo(dir.path(), email, file, subject);
        dir
    }

    fn make_repo(p: &Path, email: &str, file: &str, subject: &str) {
        std::fs::create_dir_all(p).unwrap();
        git_in(p, &["init", "-q"]);
        git_in(p, &["config", "user.email", email]);
        std::fs::create_dir_all(p.join(file).parent().unwrap()).unwrap();
        std::fs::write(p.join(file), "x").unwrap();
        git_in(p, &["add", "-A"]);
        git_in(p, &["commit", "-q", "-m", subject]);
    }

    /// A repo with no commits at all.
    fn empty_repo(p: &Path, email: &str) {
        std::fs::create_dir_all(p).unwrap();
        git_in(p, &["init", "-q"]);
        git_in(p, &["config", "user.email", email]);
    }

    fn saved(store: &mut Store, name: &str, dirs: &[&Path], vault_id: Option<i64>) -> Project {
        let mut a = add_args(name, dirs.iter().map(|d| d.to_path_buf()).collect());
        a.vault_id = vault_id;
        add(store, a).unwrap()
    }

    #[test]
    fn activity_lists_the_weeks_commits_by_the_repos_own_email() {
        let repo = repo_with_commits("me@x.com", "a.txt", "did the thing");
        git_in(repo.path(), &["config", "user.email", "other@x.com"]);
        std::fs::write(repo.path().join("b.txt"), "y").unwrap();
        git_in(repo.path(), &["add", "-A"]);
        git_in(repo.path(), &["commit", "-q", "-m", "someone else"]);
        git_in(repo.path(), &["config", "user.email", "me@x.com"]);

        let mut store = Store::open_in_memory().unwrap();
        let mut p = saved(&mut store, "demo", &[repo.path()], None);
        p.project_number = "P-7".into();
        let text = render_activity(&store, &p, day(2026, 11, 1), day(2026, 11, 7), None).unwrap();
        assert!(text.contains("## demo") && text.contains("P-7"), "{text}");
        assert!(text.contains("2026-11-01") && text.contains("2026-11-07"), "{text}");
        assert!(text.contains("2-3 sentences"), "the effective style is included: {text}");
        assert!(text.contains("did the thing") && text.contains("me@x.com"), "{text}");
        assert!(!text.contains("someone else"), "{text}");

        let other_week = render_activity(&store, &p, day(2026, 11, 8), day(2026, 11, 14), None).unwrap();
        assert!(!other_week.contains("did the thing"), "{other_week}");
    }

    #[test]
    fn a_plain_folder_is_reported_and_other_folders_still_render() {
        let plain = tempfile::tempdir().unwrap();
        let repo = repo_with_commits("me@x.com", "a.txt", "real work");
        let mut store = Store::open_in_memory().unwrap();
        let p = saved(&mut store, "demo", &[plain.path(), repo.path()], None);
        let text = render_activity(&store, &p, day(2026, 11, 1), day(2026, 11, 7), None).unwrap();
        assert!(text.contains("not a git repository"), "{text}");
        assert!(text.contains("real work"), "{text}");
    }

    #[test]
    fn an_unknown_email_is_said_so_and_an_author_override_fixes_it() {
        let repo = repo_with_commits("me@x.com", "a.txt", "real work");
        let dir = ProjectDir { path: repo.path().to_string_lossy().into_owned(), origin: String::new(), ..ProjectDir::default() };
        let text = render_commits(&dir, None, day(2026, 11, 1), day(2026, 11, 7));
        assert!(text.contains("no user.email set") && text.contains("--author"), "{text}");
        let text = render_commits(&dir, Some("me@x.com"), day(2026, 11, 1), day(2026, 11, 7));
        assert!(text.contains("real work"), "{text}");
        let none = render_commits(&dir, Some("me@x.com"), day(2026, 12, 1), day(2026, 12, 7));
        assert!(none.contains("no commits"), "{none}");
    }

    #[test]
    fn vault_notes_changed_are_listed_only_for_projects_with_a_vault() {
        let vault_repo = repo_with_commits("me@x.com", "work/demo/Notes.md", "notes");
        git_in(vault_repo.path(), &["config", "user.email", "me@x.com"]);
        let code = repo_with_commits("me@x.com", "a.txt", "code");
        let mut store = Store::open_in_memory().unwrap();
        let vault = store.add_vault("v", &vault_repo.path().to_string_lossy(), "").unwrap();

        let linked = saved(&mut store, "linked", &[code.path()], Some(vault.id));
        let text = render_activity(&store, &linked, day(2026, 11, 1), day(2026, 11, 7), None).unwrap();
        assert!(text.contains("Vault notes changed: v") && text.contains("work/demo/Notes.md"), "{text}");

        let other_code = repo_with_commits("me@x.com", "b.txt", "more code");
        let unlinked = saved(&mut store, "unlinked", &[other_code.path()], None);
        let text = render_activity(&store, &unlinked, day(2026, 11, 1), day(2026, 11, 7), None).unwrap();
        assert!(!text.contains("Vault notes changed"), "{text}");
    }

    #[test]
    fn a_folder_of_repos_includes_them_all_by_default_and_says_so() {
        let parent = tempfile::tempdir().unwrap();
        empty_repo(&parent.path().join("repo-a"), "me@x.com");
        empty_repo(&parent.path().join("group/repo-b"), "me@x.com");
        let mut store = Store::open_in_memory().unwrap();
        let p = add(&mut store, add_args("p", vec![parent.path().to_path_buf()])).unwrap();
        assert!(p.dirs[0].excludes.is_empty());
        let summary = scan_summary(&p).join("\n");
        assert!(summary.contains("2 repos") && summary.contains("repo-a") && summary.contains("group/repo-b"), "{summary}");
    }

    #[test]
    fn a_folder_without_repos_is_still_accepted_with_a_note() {
        let plain = tempfile::tempdir().unwrap();
        let mut store = Store::open_in_memory().unwrap();
        let p = add(&mut store, add_args("p", vec![plain.path().to_path_buf()])).unwrap();
        let summary = scan_summary(&p).join("\n");
        assert!(summary.contains("No git repositories"), "{summary}");
    }

    #[test]
    fn exclude_on_add_is_stored_and_an_unknown_name_is_an_error() {
        let parent = tempfile::tempdir().unwrap();
        empty_repo(&parent.path().join("keep"), "me@x.com");
        empty_repo(&parent.path().join("old/legacy"), "me@x.com");
        let mut store = Store::open_in_memory().unwrap();

        let mut bad = add_args("bad", vec![parent.path().to_path_buf()]);
        bad.excludes = vec!["nope".into()];
        let err = format!("{:#}", add(&mut store, bad).unwrap_err());
        assert!(err.contains("nope") && err.contains("legacy"), "the error lists what was found: {err}");
        assert!(store.find_project("bad").unwrap().is_none());

        let mut args = add_args("p", vec![parent.path().to_path_buf()]);
        args.excludes = vec!["legacy".into()]; // by unique folder name
        let p = add(&mut store, args).unwrap();
        assert_eq!(p.dirs[0].excludes, ["old/legacy"]);
        assert!(scan_summary(&p).join("\n").contains("excluded"), "{:?}", scan_summary(&p));
    }

    #[test]
    fn edit_can_exclude_and_then_include_a_repo_again() {
        let parent = tempfile::tempdir().unwrap();
        empty_repo(&parent.path().join("a"), "me@x.com");
        empty_repo(&parent.path().join("b"), "me@x.com");
        let mut store = Store::open_in_memory().unwrap();
        add(&mut store, add_args("p", vec![parent.path().to_path_buf()])).unwrap();

        let mut e = edit_args("p");
        e.excludes = vec!["b".into()];
        assert_eq!(edit(&mut store, e).unwrap().dirs[0].excludes, ["b"]);

        let mut e = edit_args("p");
        e.includes = vec!["b".into()];
        assert!(edit(&mut store, e).unwrap().dirs[0].excludes.is_empty());

        let mut e = edit_args("p");
        e.includes = vec!["b".into()];
        assert!(edit(&mut store, e).is_err(), "b is not excluded any more");
    }

    #[test]
    fn activity_for_a_folder_of_repos_lists_active_repos_and_collapses_the_rest() {
        let parent = tempfile::tempdir().unwrap();
        make_repo(&parent.path().join("active-repo"), "me@x.com", "a.txt", "shipped the feature");
        empty_repo(&parent.path().join("quiet-one"), "me@x.com");
        empty_repo(&parent.path().join("quiet-two"), "me@x.com");
        make_repo(&parent.path().join("skipped"), "me@x.com", "s.txt", "should not appear");
        let mut store = Store::open_in_memory().unwrap();
        let mut args = add_args("p", vec![parent.path().to_path_buf()]);
        args.excludes = vec!["skipped".into()];
        let p = add(&mut store, args).unwrap();

        let text = render_activity(&store, &p, day(2026, 11, 1), day(2026, 11, 7), None).unwrap();
        assert!(text.contains("active-repo") && text.contains("shipped the feature"), "{text}");
        assert!(text.contains("no commits: quiet-one, quiet-two"), "{text}");
        assert!(!text.contains("should not appear"), "{text}");
        assert!(text.contains("excluded: skipped"), "{text}");
        assert!(text.contains("all branches"), "{text}");
        assert!(!text.contains("not a git repository"), "{text}");
    }

    #[test]
    fn a_repo_cloned_later_shows_up_in_activity_without_registering_again() {
        let parent = tempfile::tempdir().unwrap();
        make_repo(&parent.path().join("first"), "me@x.com", "a.txt", "first repo work");
        let mut store = Store::open_in_memory().unwrap();
        let p = add(&mut store, add_args("p", vec![parent.path().to_path_buf()])).unwrap();
        make_repo(&parent.path().join("second"), "me@x.com", "b.txt", "cloned later work");
        let text = render_activity(&store, &p, day(2026, 11, 1), day(2026, 11, 7), None).unwrap();
        assert!(text.contains("first repo work") && text.contains("cloned later work"), "{text}");
    }

    #[test]
    fn commits_on_a_feature_branch_count() {
        let parent = tempfile::tempdir().unwrap();
        let repo = parent.path().join("r");
        make_repo(&repo, "me@x.com", "a.txt", "on main");
        git_in(&repo, &["checkout", "-q", "-b", "feature"]);
        std::fs::write(repo.join("f.txt"), "f").unwrap();
        git_in(&repo, &["add", "-A"]);
        git_in(&repo, &["commit", "-q", "-m", "feature branch work"]);
        git_in(&repo, &["checkout", "-q", "-"]);
        let mut store = Store::open_in_memory().unwrap();
        let p = add(&mut store, add_args("p", vec![parent.path().to_path_buf()])).unwrap();
        let text = render_activity(&store, &p, day(2026, 11, 1), day(2026, 11, 7), None).unwrap();
        assert!(text.contains("feature branch work"), "{text}");
    }

    #[test]
    fn repos_marks_which_repos_were_active() {
        let parent = tempfile::tempdir().unwrap();
        make_repo(&parent.path().join("busy"), "me@x.com", "a.txt", "one");
        empty_repo(&parent.path().join("idle"), "me@x.com");
        let mut store = Store::open_in_memory().unwrap();
        let p = add(&mut store, add_args("p", vec![parent.path().to_path_buf()])).unwrap();
        let text = render_repos(&p, day(2026, 11, 1), day(2026, 11, 7), None);
        let line = |name: &str| text.lines().find(|l| l.contains(name)).unwrap_or_else(|| panic!("{name} in {text}")).to_string();
        assert!(line("busy").contains("active") && line("busy").contains("1 commit"), "{text}");
        assert!(line("idle").contains("idle"), "{text}");
    }
}
