//! Reads what changed in a vault's git repo, for the hooks (`ingest --changed`).

use std::path::Path;
use std::process::Command;

use anyhow::{bail, Context, Result};
use chrono::NaiveDate;

use crate::ingest::Change;

/// What the latest commit changed (a root commit counts as adding everything).
pub fn changes_in_head(dir: &Path) -> Result<Vec<Change>> {
    let out = run_git(
        dir,
        &["diff-tree", "--root", "-r", "-M", "--relative", "--name-status", "-z", "--no-commit-id", "HEAD"],
    )?;
    Ok(parse_name_status(&out))
}

/// What changed between two revisions, e.g. `ORIG_HEAD` and `HEAD` after a merge.
pub fn changes_between(dir: &Path, from: &str, to: &str) -> Result<Vec<Change>> {
    let out = run_git(
        dir,
        &["diff", "--relative", "-M", "--name-status", "-z", from, to],
    )?;
    Ok(parse_name_status(&out))
}

/// One commit, as shown in a weekly activity report.
#[derive(Debug, Clone, PartialEq)]
pub struct Commit {
    /// Abbreviated hash.
    pub hash: String,
    /// Author date, `YYYY-MM-DD` in local time.
    pub date: String,
    pub subject: String,
}

/// Whether `dir` is inside a git work tree.
pub fn is_repo(dir: &Path) -> bool {
    run_git(dir, &["rev-parse", "--git-dir"]).is_ok()
}

/// The URL of the `origin` remote, if the repo has one.
pub fn origin_url(dir: &Path) -> Option<String> {
    trimmed_output(dir, &["remote", "get-url", "origin"])
}

/// The repo's configured `user.email` (the account its commits are made with), if any.
pub fn user_email(dir: &Path) -> Option<String> {
    trimmed_output(dir, &["config", "user.email"])
}

fn trimmed_output(dir: &Path, args: &[&str]) -> Option<String> {
    let out = run_git(dir, args).ok()?;
    let text = String::from_utf8_lossy(&out).trim().to_string();
    (!text.is_empty()).then_some(text)
}

/// Commits by `email` on any local or remote branch, no merges, with a commit date on
/// `first..=last` (local days), limited to files under `dir`. Oldest first.
pub fn commits_by(dir: &Path, email: &str, first: NaiveDate, last: NaiveDate) -> Result<Vec<Commit>> {
    // `--author` matches "Name <email>", so anchoring on the angle brackets matches the whole address.
    let author = format!("--author=<{}>", escape_basic_regex(email));
    let (since, until) = day_range(first, last);
    let out = run_git(
        dir,
        &[
            "log", "--branches", "--remotes", "--no-merges", "--reverse", &author, &since, &until,
            "--date=format-local:%Y-%m-%d", "--format=%h%x00%ad%x00%s", "--", ".",
        ],
    )?;
    Ok(String::from_utf8_lossy(&out)
        .lines()
        .filter_map(|line| {
            let mut parts = line.splitn(3, '\0');
            Some(Commit {
                hash: parts.next()?.to_string(),
                date: parts.next()?.to_string(),
                subject: parts.next()?.to_string(),
            })
        })
        .collect())
}

/// `--since`/`--until` arguments covering whole local days; git reads times without an offset as local.
fn day_range(first: NaiveDate, last: NaiveDate) -> (String, String) {
    (format!("--since={first}T00:00:00"), format!("--until={last}T23:59:59"))
}

fn escape_basic_regex(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        if "\\.*[]^$".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// Files touched by any commit on `first..=last` (local days), limited to `locations`
/// when given. Obsidian's own `.obsidian/` files are left out. Sorted, distinct.
pub fn paths_changed(dir: &Path, first: NaiveDate, last: NaiveDate, locations: &[String]) -> Result<Vec<String>> {
    let (since, until) = day_range(first, last);
    let mut args = vec![
        "log", "--branches", "--remotes", "--no-merges", "--name-only", "-z", "--format=", &since, &until, "--",
    ];
    args.extend(locations.iter().map(String::as_str));
    let out = run_git(dir, &args)?;
    let mut paths: Vec<String> = String::from_utf8_lossy(&out)
        .split('\0')
        .map(|p| p.trim_matches('\n'))
        .filter(|p| !p.is_empty() && !p.starts_with(".obsidian/"))
        .map(str::to_string)
        .collect();
    paths.sort();
    paths.dedup();
    Ok(paths)
}

fn run_git(dir: &Path, args: &[&str]) -> Result<Vec<u8>> {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .context("running git (is it installed and on PATH?)")?;
    if !output.status.success() {
        bail!("git {} failed: {}", args[0], String::from_utf8_lossy(&output.stderr).trim());
    }
    Ok(output.stdout)
}

/// Parses `git diff --name-status -z` output: `M\0path\0`, `R100\0old\0new\0`.
pub fn parse_name_status(bytes: &[u8]) -> Vec<Change> {
    let mut fields = bytes
        .split(|b| *b == 0)
        .map(|f| String::from_utf8_lossy(f).into_owned())
        .filter(|f| !f.is_empty());
    let mut out = Vec::new();
    while let Some(status) = fields.next() {
        match status.chars().next() {
            Some('A' | 'M' | 'T') => {
                if let Some(path) = fields.next() {
                    out.push(Change::Upsert(path));
                }
            }
            Some('D') => {
                if let Some(path) = fields.next() {
                    out.push(Change::Delete(path));
                }
            }
            Some('R') => {
                if let (Some(from), Some(to)) = (fields.next(), fields.next()) {
                    out.push(Change::Rename { from, to });
                }
            }
            Some('C') => {
                let _source = fields.next();
                if let Some(path) = fields.next() {
                    out.push(Change::Upsert(path));
                }
            }
            _ => {
                fields.next();
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn parses_every_status() {
        let raw = b"A\0new.md\0M\0edit.md\0D\0gone.md\0R100\0old/a.md\0new/a.md\0C75\0src.md\0copy.md\0T\0typed.md\0";
        assert_eq!(
            parse_name_status(raw),
            vec![
                Change::Upsert("new.md".into()),
                Change::Upsert("edit.md".into()),
                Change::Delete("gone.md".into()),
                Change::Rename { from: "old/a.md".into(), to: "new/a.md".into() },
                Change::Upsert("copy.md".into()),
                Change::Upsert("typed.md".into()),
            ]
        );
    }

    #[test]
    fn keeps_unusual_paths_intact() {
        let raw = "M\0dir with spaces/Ünï.md\0".as_bytes();
        assert_eq!(parse_name_status(raw), vec![Change::Upsert("dir with spaces/Ünï.md".into())]);
    }

    fn git(dir: &Path, args: &[&str]) {
        let status = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["-c", "user.name=t", "-c", "user.email=t@t", "-c", "commit.gpgsign=false"])
            .args(args)
            .output()
            .unwrap();
        assert!(status.status.success(), "{}", String::from_utf8_lossy(&status.stderr));
    }

    #[test]
    fn reads_real_commits_including_the_root_commit() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        git(p, &["init", "-q"]);
        fs::write(p.join("A.md"), "one\n").unwrap();
        fs::write(p.join("B.md"), "two\n").unwrap();
        git(p, &["add", "."]);
        git(p, &["commit", "-q", "-m", "first"]);

        let mut root = changes_in_head(p).unwrap();
        root.sort_by_key(|c| format!("{c:?}"));
        assert_eq!(root, vec![Change::Upsert("A.md".into()), Change::Upsert("B.md".into())]);

        fs::write(p.join("A.md"), "one edited\n").unwrap();
        fs::remove_file(p.join("B.md")).unwrap();
        fs::create_dir(p.join("sub")).unwrap();
        fs::write(p.join("sub").join("C.md"), "three\n").unwrap();
        git(p, &["add", "-A"]);
        git(p, &["commit", "-q", "-m", "second"]);

        let mut second = changes_in_head(p).unwrap();
        second.sort_by_key(|c| format!("{c:?}"));
        assert_eq!(
            second,
            vec![
                Change::Delete("B.md".into()),
                Change::Upsert("A.md".into()),
                Change::Upsert("sub/C.md".into()),
            ]
        );

        let between = changes_between(p, "HEAD~1", "HEAD").unwrap();
        assert_eq!(between.len(), 3);
    }

    #[test]
    fn detects_a_pure_move_as_a_rename() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        git(p, &["init", "-q"]);
        fs::create_dir(p.join("investigations")).unwrap();
        let body = "some reasonably long content so git can see it is the same file\n".repeat(5);
        fs::write(p.join("investigations").join("Slug.md"), &body).unwrap();
        git(p, &["add", "."]);
        git(p, &["commit", "-q", "-m", "first"]);

        fs::create_dir_all(p.join("stories/1-x/investigations")).unwrap();
        fs::rename(
            p.join("investigations/Slug.md"),
            p.join("stories/1-x/investigations/Slug.md"),
        )
        .unwrap();
        git(p, &["add", "-A"]);
        git(p, &["commit", "-q", "-m", "move"]);

        assert_eq!(
            changes_in_head(p).unwrap(),
            vec![Change::Rename {
                from: "investigations/Slug.md".into(),
                to: "stories/1-x/investigations/Slug.md".into()
            }]
        );
    }

    fn commit_as(dir: &Path, email: &str, date: &str, files: &[(&str, &str)], msg: &str) {
        for (path, body) in files {
            let full = dir.join(path);
            fs::create_dir_all(full.parent().unwrap()).unwrap();
            fs::write(full, body).unwrap();
        }
        git(dir, &["add", "-A"]);
        let out = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["-c", "user.name=t", "-c", &format!("user.email={email}"), "-c", "commit.gpgsign=false"])
            .args(["commit", "-q", "--allow-empty", "-m", msg])
            .env("GIT_AUTHOR_DATE", date)
            .env("GIT_COMMITTER_DATE", date)
            .output()
            .unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    }

    fn day(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    #[test]
    fn commits_by_filters_by_author_and_inclusive_day_range() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        git(p, &["init", "-q"]);
        commit_as(p, "me@x.com", "2026-10-31T23:59:00", &[("a", "1")], "too early");
        commit_as(p, "me@x.com", "2026-11-01T00:00:00", &[("a", "2")], "first minute");
        commit_as(p, "other@x.com", "2026-11-03T12:00:00", &[("a", "3")], "not mine");
        commit_as(p, "me@x.com", "2026-11-07T23:59:00", &[("a", "4")], "last minute");
        commit_as(p, "me@x.com", "2026-11-08T00:00:00", &[("a", "5")], "too late");

        let got = commits_by(p, "me@x.com", day(2026, 11, 1), day(2026, 11, 7)).unwrap();
        let subjects: Vec<_> = got.iter().map(|c| c.subject.as_str()).collect();
        assert_eq!(subjects, ["first minute", "last minute"]);
        assert_eq!(got[0].date, "2026-11-01");
        assert_eq!(got[0].hash.len(), 7);
    }

    #[test]
    fn commits_by_matches_the_whole_email_and_only_files_under_dir() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        git(p, &["init", "-q"]);
        commit_as(p, "me@x.com", "2026-11-02T10:00:00", &[("app/f", "1")], "in app");
        commit_as(p, "me@x.com", "2026-11-02T11:00:00", &[("lib/f", "1")], "in lib");
        commit_as(p, "someme@x.com", "2026-11-02T12:00:00", &[("app/g", "1")], "lookalike");
        commit_as(p, "me+tag@x.com", "2026-11-02T13:00:00", &[("app/h", "1")], "plus address");

        let got = commits_by(&p.join("app"), "me@x.com", day(2026, 11, 1), day(2026, 11, 7)).unwrap();
        let subjects: Vec<_> = got.iter().map(|c| c.subject.as_str()).collect();
        assert_eq!(subjects, ["in app"]);
    }

    #[test]
    fn an_empty_repo_has_no_commits() {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "-q"]);
        assert!(commits_by(dir.path(), "me@x.com", day(2026, 11, 1), day(2026, 11, 7)).unwrap().is_empty());
        assert!(paths_changed(dir.path(), day(2026, 11, 1), day(2026, 11, 7), &[]).unwrap().is_empty());
    }

    #[test]
    fn paths_changed_respects_locations_and_skips_obsidian_files() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        git(p, &["init", "-q"]);
        commit_as(p, "me@x.com", "2026-10-20T10:00:00", &[("a/old.md", "x")], "before the week");
        commit_as(
            p,
            "me@x.com",
            "2026-11-02T10:00:00",
            &[("a/x.md", "1"), ("b/y.md", "1"), (".obsidian/workspace.json", "{}")],
            "this week",
        );
        let all = paths_changed(p, day(2026, 11, 1), day(2026, 11, 7), &[]).unwrap();
        assert_eq!(all, ["a/x.md", "b/y.md"]);
        let only_a = paths_changed(p, day(2026, 11, 1), day(2026, 11, 7), &["a".to_string()]).unwrap();
        assert_eq!(only_a, ["a/x.md"]);
    }

    #[test]
    fn origin_and_repo_detection() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        assert!(!is_repo(p));
        assert_eq!(origin_url(p), None);
        git(p, &["init", "-q"]);
        assert!(is_repo(p));
        assert_eq!(origin_url(p), None);
        git(p, &["remote", "add", "origin", "git@example.com:me/repo.git"]);
        assert_eq!(origin_url(p), Some("git@example.com:me/repo.git".to_string()));
        git(p, &["config", "user.email", "me@x.com"]);
        assert_eq!(user_email(p), Some("me@x.com".to_string()));
    }

    #[test]
    fn not_a_repo_is_a_clear_error() {
        let dir = tempfile::tempdir().unwrap();
        let err = changes_in_head(dir.path()).unwrap_err().to_string();
        assert!(err.contains("git diff-tree failed"), "{err}");
    }
}
