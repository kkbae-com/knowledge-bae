//! Finding the git repos beneath a project folder that is not a repo itself.

use std::path::{Path, PathBuf};

use crate::git;
use crate::store::ProjectDir;

/// How many folder levels beneath a project folder are searched for repos.
pub const MAX_DEPTH: usize = 3;

/// Folders never searched.
const SKIPPED: &[&str] = &[".git", "node_modules", "target"];

/// A repo found beneath a project folder.
#[derive(Debug, Clone, PartialEq)]
pub struct Repo {
    /// Path relative to the project folder, the name used in reports and `--exclude`.
    pub name: String,
    pub path: PathBuf,
}

/// What a project folder stands for right now.
#[derive(Debug, Clone, PartialEq)]
pub enum Folder {
    Missing,
    /// The folder is a repo (or inside one) and is used as it is.
    Repo,
    /// The folder holds these repos, minus the excluded ones. Empty when none are found.
    Parent { repos: Vec<Repo>, excluded: Vec<String> },
}

/// Repos beneath `root` (not `root` itself), as paths relative to it, sorted. Searches
/// `MAX_DEPTH` levels, skips `.git`, `node_modules` and `target`, and does not look inside a repo.
pub fn find_repos(root: &Path) -> Vec<String> {
    let mut found = Vec::new();
    let mut walk = walkdir::WalkDir::new(root).max_depth(MAX_DEPTH).sort_by_file_name().into_iter();
    while let Some(entry) = walk.next() {
        let Ok(entry) = entry else { continue };
        if entry.depth() == 0 || !entry.file_type().is_dir() {
            continue;
        }
        if SKIPPED.contains(&entry.file_name().to_string_lossy().as_ref()) {
            walk.skip_current_dir();
        } else if entry.path().join(".git").exists() {
            // A directory or, for worktrees and submodules, a file.
            if let Ok(rel) = entry.path().strip_prefix(root) {
                found.push(rel.to_string_lossy().replace('\\', "/"));
            }
            walk.skip_current_dir();
        }
    }
    found.sort();
    found
}

/// Decides what `dir` is today. Repos are looked up on every call, so one cloned later is
/// picked up without registering anything again.
pub fn classify(dir: &ProjectDir) -> Folder {
    let root = Path::new(&dir.path);
    if !root.is_dir() {
        return Folder::Missing;
    }
    if git::is_repo(root) {
        return Folder::Repo;
    }
    let (excluded, kept): (Vec<String>, Vec<String>) =
        find_repos(root).into_iter().partition(|name| dir.excludes.contains(name));
    let repos = kept.into_iter().map(|name| Repo { path: root.join(&name), name }).collect();
    Folder::Parent { repos, excluded }
}

/// The name in `names` that `wanted` refers to: an exact relative path, or the last path segment
/// when only one repo has it.
pub fn resolve_name<'a>(names: &'a [String], wanted: &str) -> Option<&'a str> {
    if let Some(exact) = names.iter().find(|n| n.as_str() == wanted) {
        return Some(exact);
    }
    let mut by_last_segment = names.iter().filter(|n| n.rsplit('/').next() == Some(wanted));
    match (by_last_segment.next(), by_last_segment.next()) {
        (Some(only), None) => Some(only),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::process::Command;

    fn fake_repo(root: &Path, rel: &str) {
        fs::create_dir_all(root.join(rel).join(".git")).unwrap();
    }

    fn dir_entry(path: &Path, excludes: &[&str]) -> ProjectDir {
        ProjectDir {
            path: path.to_string_lossy().into_owned(),
            excludes: excludes.iter().map(|e| e.to_string()).collect(),
            ..ProjectDir::default()
        }
    }

    #[test]
    fn finds_repos_up_to_three_levels_sorted() {
        let root = tempfile::tempdir().unwrap();
        for rel in ["b", "a", "group/c", "x/y/z", "p/q/r/too-deep"] {
            fake_repo(root.path(), rel);
        }
        assert_eq!(find_repos(root.path()), ["a", "b", "group/c", "x/y/z"]);
    }

    #[test]
    fn skips_build_folders_and_never_looks_inside_a_repo() {
        let root = tempfile::tempdir().unwrap();
        for rel in ["real", "node_modules/pkg", "target/debug/x", "real/vendor/inner"] {
            fake_repo(root.path(), rel);
        }
        assert_eq!(find_repos(root.path()), ["real"]);
    }

    #[test]
    fn a_git_file_marks_a_worktree_or_submodule() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir_all(root.path().join("wt")).unwrap();
        fs::write(root.path().join("wt/.git"), "gitdir: /elsewhere").unwrap();
        assert_eq!(find_repos(root.path()), ["wt"]);
    }

    #[test]
    fn a_plain_folder_with_no_repos_finds_none() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir_all(root.path().join("docs/notes")).unwrap();
        assert!(find_repos(root.path()).is_empty());
    }

    #[test]
    fn classify_tells_missing_repo_and_parent_apart() {
        assert_eq!(classify(&dir_entry(Path::new("/definitely/not/here"), &[])), Folder::Missing);

        let repo = tempfile::tempdir().unwrap();
        assert!(Command::new("git").arg("-C").arg(repo.path()).args(["init", "-q"]).status().unwrap().success());
        assert_eq!(classify(&dir_entry(repo.path(), &[])), Folder::Repo);

        let parent = tempfile::tempdir().unwrap();
        fake_repo(parent.path(), "one");
        fake_repo(parent.path(), "two");
        match classify(&dir_entry(parent.path(), &["two"])) {
            Folder::Parent { repos, excluded } => {
                assert_eq!(repos.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(), ["one"]);
                assert_eq!(repos[0].path, parent.path().join("one"));
                assert_eq!(excluded, ["two"]);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_repo_cloned_later_is_picked_up_without_registering_again() {
        let parent = tempfile::tempdir().unwrap();
        fake_repo(parent.path(), "one");
        let entry = dir_entry(parent.path(), &[]);
        fake_repo(parent.path(), "cloned-later");
        match classify(&entry) {
            Folder::Parent { repos, .. } => assert_eq!(repos.len(), 2),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_folder_with_no_repos_is_an_empty_parent() {
        let plain = tempfile::tempdir().unwrap();
        assert_eq!(classify(&dir_entry(plain.path(), &[])), Folder::Parent { repos: vec![], excluded: vec![] });
    }

    #[test]
    fn names_resolve_by_path_or_unique_last_segment() {
        let names: Vec<String> = ["app", "group/api", "other/api"].map(String::from).to_vec();
        assert_eq!(resolve_name(&names, "app"), Some("app"));
        assert_eq!(resolve_name(&names, "group/api"), Some("group/api"));
        assert_eq!(resolve_name(&names, "api"), None, "two repos are called api");
        assert_eq!(resolve_name(&names, "nope"), None);
        let one: Vec<String> = ["group/api".to_string()].to_vec();
        assert_eq!(resolve_name(&one, "api"), Some("group/api"));
    }
}
