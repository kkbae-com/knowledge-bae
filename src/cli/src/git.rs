//! Reads what changed in a vault's git repo, for the hooks (`ingest --changed`).

use std::path::Path;
use std::process::Command;

use anyhow::{bail, Context, Result};

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

    #[test]
    fn not_a_repo_is_a_clear_error() {
        let dir = tempfile::tempdir().unwrap();
        let err = changes_in_head(dir.path()).unwrap_err().to_string();
        assert!(err.contains("git diff-tree failed"), "{err}");
    }
}
