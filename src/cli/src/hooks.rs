//! Installs the git hooks that keep a vault's index current.
//!
//! Each hook gets a marked block of shell. The block goes right after the
//! shebang, so an existing hook's own logic (even one that ends in `exit`) cannot
//! stop it, and it runs in the background so a commit never waits on the
//! embedding model. Uninstalling removes only that block.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};

const BEGIN: &str = "# >>> knowledge-bae >>>";
const END: &str = "# <<< knowledge-bae <<<";

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Hook {
    PostCommit,
    PostMerge,
    PostCheckout,
    PostRewrite,
}

impl Hook {
    pub const ALL: [Hook; 4] = [Hook::PostCommit, Hook::PostMerge, Hook::PostCheckout, Hook::PostRewrite];

    pub fn file_name(self) -> &'static str {
        match self {
            Hook::PostCommit => "post-commit",
            Hook::PostMerge => "post-merge",
            Hook::PostCheckout => "post-checkout",
            Hook::PostRewrite => "post-rewrite",
        }
    }

    /// A commit has a precise diff. For merges, checkouts and rebases the range
    /// is awkward, and a full pass is cheap because unchanged notes are skipped
    /// by hash (the model only loads if something actually changed).
    fn ingest_args(self) -> &'static str {
        match self {
            Hook::PostCommit => "--changed",
            _ => "--all",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum HookState {
    Installed,
    /// A hook file exists but has no knowledge-bae block.
    Missing,
}

/// The shell block for one hook. Paths are single-quoted so spaces are safe.
pub fn render_block(hook: Hook, exe: &Path, vault: &Path, log: &Path) -> String {
    format!(
        "{BEGIN}\n\
         # Managed by `knowledge-bae hooks install`; remove with `knowledge-bae hooks uninstall`.\n\
         {exe} ingest {args} --vault {vault} >> {log} 2>&1 &\n\
         {END}\n",
        exe = shell_quote(exe),
        args = hook.ingest_args(),
        vault = shell_quote(vault),
        log = shell_quote(log),
    )
}

fn shell_quote(path: &Path) -> String {
    // Git for Windows runs hooks in its own sh, which accepts forward slashes.
    let text = path.to_string_lossy().replace('\\', "/");
    format!("'{}'", text.replace('\'', r"'\''"))
}

/// Returns the hook file's new contents with the block added (or refreshed).
pub fn apply_block(existing: Option<&str>, block: &str) -> Result<String> {
    let Some(existing) = existing else {
        return Ok(format!("#!/bin/sh\n{block}"));
    };
    if let Some(start) = existing.find(BEGIN) {
        let end = existing[start..]
            .find(END)
            .map(|i| start + i + END.len())
            .context("found the start of a knowledge-bae block but not its end; fix the hook by hand")?;
        let end = existing[end..].strip_prefix('\n').map_or(end, |rest| existing.len() - rest.len());
        return Ok(format!("{}{}{}", &existing[..start], block, &existing[end..]));
    }
    match existing.lines().next() {
        Some(first) if first.starts_with("#!") => {
            if !is_shell_shebang(first) {
                bail!("this hook is not a shell script ({first}), so the block can't be added safely; add it by hand");
            }
            let split = existing.find('\n').map_or(existing.len(), |i| i + 1);
            let (shebang, rest) = existing.split_at(split);
            let shebang = if shebang.ends_with('\n') { shebang.to_string() } else { format!("{shebang}\n") };
            Ok(format!("{shebang}{block}{rest}"))
        }
        _ => Ok(format!("#!/bin/sh\n{block}{existing}")),
    }
}

fn is_shell_shebang(line: &str) -> bool {
    let program = line
        .trim_start_matches("#!")
        .split_whitespace()
        .flat_map(|part| part.rsplit('/').next())
        .collect::<Vec<_>>();
    program.iter().any(|p| matches!(*p, "sh" | "bash" | "zsh" | "dash" | "ash" | "ksh"))
}

/// Contents with the block removed, or `None` if nothing meaningful is left.
pub fn remove_block(existing: &str) -> Option<String> {
    let start = existing.find(BEGIN)?;
    let end = existing[start..].find(END).map(|i| start + i + END.len())?;
    let end = existing[end..].strip_prefix('\n').map_or(end, |rest| existing.len() - rest.len());
    let remaining = format!("{}{}", &existing[..start], &existing[end..]);
    let meaningful = remaining
        .lines()
        .any(|l| !l.trim().is_empty() && !l.starts_with("#!"));
    meaningful.then_some(remaining)
}

pub fn hooks_dir(vault: &Path) -> Result<PathBuf> {
    let output = Command::new("git")
        .arg("-C")
        .arg(vault)
        .args(["rev-parse", "--git-path", "hooks"])
        .output()
        .context("running git (is it installed and on PATH?)")?;
    if !output.status.success() {
        bail!(
            "{} is not a git repository. Run `git init` there first (the Obsidian Git plugin needs it too).",
            vault.display()
        );
    }
    let path = PathBuf::from(String::from_utf8_lossy(&output.stdout).trim());
    Ok(if path.is_absolute() { path } else { vault.join(path) })
}

pub fn install(vault: &Path, exe: &Path, log: &Path) -> Result<Vec<Hook>> {
    let dir = hooks_dir(vault)?;
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let mut done = Vec::new();
    for hook in Hook::ALL {
        let path = dir.join(hook.file_name());
        let existing = read_if_exists(&path)?;
        let block = render_block(hook, exe, vault, log);
        let updated = apply_block(existing.as_deref(), &block)
            .with_context(|| format!("{}", path.display()))?;
        std::fs::write(&path, updated).with_context(|| format!("writing {}", path.display()))?;
        make_executable(&path)?;
        done.push(hook);
    }
    Ok(done)
}

pub fn uninstall(vault: &Path) -> Result<Vec<Hook>> {
    let dir = hooks_dir(vault)?;
    let mut removed = Vec::new();
    for hook in Hook::ALL {
        let path = dir.join(hook.file_name());
        let Some(existing) = read_if_exists(&path)? else { continue };
        if !existing.contains(BEGIN) {
            continue;
        }
        match remove_block(&existing) {
            Some(rest) => std::fs::write(&path, rest)?,
            None => std::fs::remove_file(&path)?,
        }
        removed.push(hook);
    }
    Ok(removed)
}

pub fn status(vault: &Path) -> Result<Vec<(Hook, HookState)>> {
    let dir = hooks_dir(vault)?;
    let mut out = Vec::new();
    for hook in Hook::ALL {
        let installed = read_if_exists(&dir.join(hook.file_name()))?.is_some_and(|c| c.contains(BEGIN));
        out.push((hook, if installed { HookState::Installed } else { HookState::Missing }));
    }
    Ok(out)
}

/// The binary an installed hook will run, read back from its block.
pub fn installed_exe(vault: &Path, hook: Hook) -> Result<Option<PathBuf>> {
    let path = hooks_dir(vault)?.join(hook.file_name());
    let Some(text) = read_if_exists(&path)? else { return Ok(None) };
    Ok(parse_exe(&text))
}

fn parse_exe(hook_text: &str) -> Option<PathBuf> {
    let start = hook_text.find(BEGIN)?;
    let line = hook_text[start..]
        .lines()
        .find(|l| l.starts_with('\'') && l.contains(" ingest "))?;
    // The command starts with a single-quoted path; `'\''` encodes a literal quote.
    let mut out = String::new();
    let mut chars = line[1..].chars();
    while let Some(c) = chars.next() {
        if c != '\'' {
            out.push(c);
            continue;
        }
        let rest: String = chars.clone().take(3).collect();
        if rest == "\\''" {
            out.push('\'');
            chars.nth(2);
        } else {
            return Some(PathBuf::from(out));
        }
    }
    None
}

fn read_if_exists(path: &Path) -> Result<Option<String>> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
    }
}

#[cfg(unix)]
fn make_executable(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = std::fs::metadata(path)?.permissions();
    perms.set_mode(perms.mode() | 0o755);
    std::fs::set_permissions(path, perms)?;
    Ok(())
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn block(hook: Hook) -> String {
        render_block(hook, Path::new("/bin/kb"), Path::new("/v"), Path::new("/log"))
    }

    #[test]
    fn block_quotes_paths_and_picks_the_right_ingest_mode() {
        let b = render_block(
            Hook::PostCommit,
            Path::new("/home/it's me/kb"),
            Path::new("/my vault"),
            Path::new("/log file"),
        );
        assert!(b.contains(r#"'/home/it'\''s me/kb' ingest --changed --vault '/my vault' >> '/log file' 2>&1 &"#), "{b}");
        assert!(block(Hook::PostMerge).contains("ingest --all"));
        assert!(block(Hook::PostRewrite).contains("ingest --all"));
        assert!(block(Hook::PostCheckout).contains("ingest --all"));
    }

    #[test]
    fn windows_paths_become_forward_slashes() {
        let b = render_block(Hook::PostCommit, Path::new(r"C:\bin\kb.exe"), Path::new("/v"), Path::new("/l"));
        assert!(b.contains("'C:/bin/kb.exe'"));
    }

    #[test]
    fn new_hook_gets_a_shebang() {
        let out = apply_block(None, &block(Hook::PostCommit)).unwrap();
        assert!(out.starts_with("#!/bin/sh\n# >>> knowledge-bae >>>"));
    }

    #[test]
    fn existing_hook_is_kept_and_block_goes_after_the_shebang() {
        let existing = "#!/bin/bash\necho hi\nexit 0\n";
        let out = apply_block(Some(existing), &block(Hook::PostCommit)).unwrap();
        assert!(out.starts_with("#!/bin/bash\n# >>> knowledge-bae >>>"));
        assert!(out.ends_with("echo hi\nexit 0\n"));
    }

    #[test]
    fn applying_twice_replaces_instead_of_duplicating() {
        let first = apply_block(Some("#!/bin/sh\necho hi\n"), &block(Hook::PostCommit)).unwrap();
        let newer = render_block(Hook::PostCommit, Path::new("/new/kb"), Path::new("/v"), Path::new("/log"));
        let second = apply_block(Some(&first), &newer).unwrap();
        assert_eq!(second.matches(BEGIN).count(), 1);
        assert!(second.contains("/new/kb") && !second.contains("/bin/kb"));
        assert!(second.contains("echo hi"));
    }

    #[test]
    fn non_shell_hooks_are_refused() {
        let err = apply_block(Some("#!/usr/bin/env python3\nprint(1)\n"), &block(Hook::PostCommit))
            .unwrap_err()
            .to_string();
        assert!(err.contains("not a shell script"), "{err}");
        assert!(apply_block(Some("#!/usr/bin/env bash\n"), &block(Hook::PostCommit)).is_ok());
    }

    #[test]
    fn exe_path_round_trips_through_the_block() {
        for exe in ["/bin/kb", "/home/it's me/kb", "/with space/kb"] {
            let b = render_block(Hook::PostCommit, Path::new(exe), Path::new("/v"), Path::new("/l"));
            assert_eq!(parse_exe(&format!("#!/bin/sh\n{b}")), Some(PathBuf::from(exe)), "{exe}");
        }
        assert_eq!(parse_exe("#!/bin/sh\necho hi\n"), None);
    }

    #[test]
    fn remove_keeps_other_content_and_drops_empty_files() {
        let with_other = apply_block(Some("#!/bin/sh\necho hi\n"), &block(Hook::PostCommit)).unwrap();
        assert_eq!(remove_block(&with_other).unwrap(), "#!/bin/sh\necho hi\n");

        let only_ours = apply_block(None, &block(Hook::PostCommit)).unwrap();
        assert_eq!(remove_block(&only_ours), None);
    }

    fn git(dir: &Path, args: &[&str]) {
        let out = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["-c", "user.name=t", "-c", "user.email=t@t", "-c", "commit.gpgsign=false"])
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    }

    #[test]
    fn install_status_uninstall_on_a_real_repo() {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "-q"]);
        let hooks = hooks_dir(dir.path()).unwrap();
        fs::create_dir_all(&hooks).unwrap();
        fs::write(hooks.join("post-merge"), "#!/bin/sh\necho mine\n").unwrap();

        let installed = install(dir.path(), Path::new("/bin/kb"), Path::new("/log")).unwrap();
        assert_eq!(installed.len(), 4);
        install(dir.path(), Path::new("/bin/kb"), Path::new("/log")).unwrap();
        let merge = fs::read_to_string(hooks.join("post-merge")).unwrap();
        assert_eq!(merge.matches(BEGIN).count(), 1);
        assert!(merge.contains("echo mine"));
        assert!(status(dir.path()).unwrap().iter().all(|(_, s)| *s == HookState::Installed));

        let removed = uninstall(dir.path()).unwrap();
        assert_eq!(removed.len(), 4);
        assert_eq!(fs::read_to_string(hooks.join("post-merge")).unwrap(), "#!/bin/sh\necho mine\n");
        assert!(!hooks.join("post-commit").exists());
        assert!(status(dir.path()).unwrap().iter().all(|(_, s)| *s == HookState::Missing));
    }

    #[test]
    fn not_a_repo_says_to_git_init() {
        let dir = tempfile::tempdir().unwrap();
        let err = install(dir.path(), Path::new("/bin/kb"), Path::new("/log")).unwrap_err().to_string();
        assert!(err.contains("git init"), "{err}");
    }

    /// Makes a real commit and checks that git runs the hook, with the right
    /// arguments, even when the vault path contains a space.
    #[cfg(unix)]
    #[test]
    fn a_real_commit_fires_the_hook_with_the_right_arguments() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let vault = root.path().join("my vault");
        fs::create_dir(&vault).unwrap();
        git(&vault, &["init", "-q"]);

        let record = root.path().join("record.txt");
        let fake = root.path().join("fake kb");
        fs::write(&fake, format!("#!/bin/sh\necho \"$@\" > '{}'\n", record.display())).unwrap();
        fs::set_permissions(&fake, fs::Permissions::from_mode(0o755)).unwrap();

        install(&vault, &fake, &root.path().join("hook.log")).unwrap();
        fs::write(vault.join("Note.md"), "hello\n").unwrap();
        git(&vault, &["add", "."]);
        git(&vault, &["commit", "-q", "-m", "x"]);

        let mut seen = String::new();
        for _ in 0..50 {
            if let Ok(text) = fs::read_to_string(&record) {
                seen = text;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        assert_eq!(seen.trim(), format!("ingest --changed --vault {}", vault.display()));
    }
}
