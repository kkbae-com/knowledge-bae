//! Health checks for the index and each registered vault.

use std::collections::HashSet;
use std::path::Path;

use crate::hooks::{self, HookState};
use crate::ingest::vault_files;
use crate::obsidian;
use crate::store::{Store, Vault};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Level {
    Ok,
    Info,
    Warn,
    Fail,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Check {
    pub level: Level,
    pub text: String,
}

fn check(level: Level, text: impl Into<String>) -> Check {
    Check { level, text: text.into() }
}

pub fn check_environment() -> Vec<Check> {
    match std::process::Command::new("git").arg("--version").output() {
        Ok(out) if out.status.success() => {
            vec![check(Level::Ok, String::from_utf8_lossy(&out.stdout).trim().to_string())]
        }
        _ => vec![check(Level::Fail, "git was not found on PATH (needed for hooks and incremental indexing)")],
    }
}

pub fn check_index(store: &Store) -> Vec<Check> {
    match store.stats() {
        Ok(stats) => {
            let mut out = vec![check(
                Level::Ok,
                format!("index has {} vaults, {} notes, {} chunks", stats.vaults, stats.notes, stats.chunks),
            )];
            if stats.unembedded_chunks > 0 {
                out.push(check(
                    Level::Warn,
                    format!("{} chunks have no embedding (run `knowledge-bae ingest --all` for each vault)", stats.unembedded_chunks),
                ));
            }
            out
        }
        Err(e) => vec![check(Level::Fail, format!("could not read the index: {e:#}"))],
    }
}

pub fn check_vault(store: &Store, vault: &Vault) -> Vec<Check> {
    let root = Path::new(&vault.path);
    if !root.is_dir() {
        return vec![check(Level::Fail, format!("folder not found: {}", vault.path))];
    }
    let mut out = vec![check(Level::Ok, "folder exists")];

    match hooks::status(root) {
        Err(_) => out.push(check(
            Level::Warn,
            "not a git repository, so no hooks and no Obsidian Git (run `git init` in the vault)",
        )),
        Ok(statuses) => {
            let missing: Vec<_> = statuses
                .iter()
                .filter(|(_, s)| *s == HookState::Missing)
                .map(|(h, _)| h.file_name())
                .collect();
            if missing.len() == statuses.len() {
                out.push(check(Level::Warn, "git hooks not installed (run `knowledge-bae hooks install`)"));
            } else if !missing.is_empty() {
                out.push(check(
                    Level::Warn,
                    format!("hooks missing: {} (run `knowledge-bae hooks install`)", missing.join(", ")),
                ));
            } else {
                out.push(check(Level::Ok, "git hooks installed"));
            }
            for (hook, state) in &statuses {
                if *state != HookState::Installed {
                    continue;
                }
                if let Ok(Some(exe)) = hooks::installed_exe(root, *hook) {
                    if !exe.exists() {
                        out.push(check(
                            Level::Fail,
                            format!(
                                "{} hook runs {}, which no longer exists (re-run `knowledge-bae hooks install`)",
                                hook.file_name(),
                                exe.display()
                            ),
                        ));
                    }
                }
            }
        }
    }

    out.push(check_obsidian(root, &vault.name));

    match (vault_files(root), store.note_paths(vault.id)) {
        (Ok(on_disk), Ok(indexed)) => {
            let disk: HashSet<&str> = on_disk.iter().map(String::as_str).collect();
            let stored: HashSet<&str> = indexed.iter().map(String::as_str).collect();
            let unindexed = disk.difference(&stored).count();
            let stale = stored.difference(&disk).count();
            if unindexed == 0 && stale == 0 {
                out.push(check(Level::Ok, format!("{} notes, all indexed", indexed.len())));
            } else {
                out.push(check(
                    Level::Warn,
                    format!(
                        "{unindexed} notes on disk are not indexed and {stale} indexed notes no longer exist (run `knowledge-bae ingest --vault {} --all`)",
                        vault.name
                    ),
                ));
            }
        }
        (Err(e), _) | (_, Err(e)) => out.push(check(Level::Fail, format!("could not compare with the index: {e:#}"))),
    }

    if vault.structure.trim().is_empty() {
        out.push(check(
            Level::Info,
            format!("no structure instructions (optional: `knowledge-bae vault structure {} --set-default`)", vault.name),
        ));
    } else {
        out.push(check(Level::Ok, "structure instructions set"));
    }
    out
}

fn check_obsidian(root: &Path, vault_name: &str) -> Check {
    let fix = format!("run `knowledge-bae obsidian setup --vault {vault_name}`");
    match obsidian::inspect(root) {
        Err(e) => check(Level::Warn, format!("could not read the Obsidian config: {e:#}")),
        Ok(s) if !s.is_obsidian_vault => check(Level::Info, "no .obsidian folder yet (open this folder as a vault in Obsidian)"),
        Ok(s) if !s.installed => check(
            Level::Warn,
            "Obsidian Git plugin is not installed (Obsidian > Settings > Community plugins > Browse > \"Git\"); without it edits are only indexed when you commit by hand",
        ),
        Ok(s) if !s.enabled => check(Level::Warn, format!("Obsidian Git plugin is installed but not enabled ({fix})")),
        Ok(s) if !s.auto_commit_on() => check(Level::Warn, format!("Obsidian Git automatic commits are off ({fix})")),
        Ok(s) if s.sync_needs_disabling(obsidian::has_remote(root)) => check(
            Level::Warn,
            format!("Obsidian Git will try to push and pull but this vault has no git remote ({fix} to turn that off, or add a remote)"),
        ),
        Ok(_) => check(Level::Ok, "Obsidian Git plugin enabled and committing automatically"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::process::Command;

    fn levels(checks: &[Check]) -> Vec<Level> {
        checks.iter().map(|c| c.level).collect()
    }

    fn git_init(dir: &Path) {
        assert!(Command::new("git").arg("-C").arg(dir).args(["init", "-q"]).status().unwrap().success());
    }

    #[test]
    fn missing_folder_fails() {
        let store = Store::open_in_memory().unwrap();
        let vault = store.add_vault("v", "/definitely/not/here", "").unwrap();
        assert_eq!(levels(&check_vault(&store, &vault)), vec![Level::Fail]);
    }

    #[test]
    fn plain_folder_warns_about_git_and_unindexed_notes() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("A.md"), "## S\n\ntext\n").unwrap();
        let store = Store::open_in_memory().unwrap();
        let vault = store.add_vault("v", dir.path().to_str().unwrap(), "").unwrap();

        let checks = check_vault(&store, &vault);
        let text: Vec<_> = checks.iter().map(|c| c.text.as_str()).collect();
        assert!(text.iter().any(|t| t.contains("not a git repository")), "{text:?}");
        assert!(text.iter().any(|t| t.contains("1 notes on disk are not indexed")), "{text:?}");
        assert!(text.iter().any(|t| t.contains("no structure instructions")));
        assert!(!levels(&checks).contains(&Level::Fail));
    }

    fn configure_obsidian_git(dir: &Path) {
        let plugin = dir.join(".obsidian/plugins/obsidian-git");
        fs::create_dir_all(&plugin).unwrap();
        fs::write(plugin.join("main.js"), "//").unwrap();
        fs::write(plugin.join("manifest.json"), "{}").unwrap();
        obsidian::ensure(dir, 5).unwrap();
    }

    #[test]
    fn obsidian_git_problems_are_named() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open_in_memory().unwrap();
        let vault = store.add_vault("v", dir.path().to_str().unwrap(), "x").unwrap();
        let texts = |checks: Vec<Check>| checks.into_iter().map(|c| c.text).collect::<Vec<_>>().join("\n");

        assert!(texts(check_vault(&store, &vault)).contains("no .obsidian folder"));

        fs::create_dir_all(dir.path().join(".obsidian")).unwrap();
        assert!(texts(check_vault(&store, &vault)).contains("plugin is not installed"));

        let plugin = dir.path().join(".obsidian/plugins/obsidian-git");
        fs::create_dir_all(&plugin).unwrap();
        fs::write(plugin.join("main.js"), "//").unwrap();
        fs::write(plugin.join("manifest.json"), "{}").unwrap();
        assert!(texts(check_vault(&store, &vault)).contains("installed but not enabled"));

        fs::write(dir.path().join(".obsidian/community-plugins.json"), r#"["obsidian-git"]"#).unwrap();
        assert!(texts(check_vault(&store, &vault)).contains("automatic commits are off"));

        obsidian::ensure(dir.path(), 5).unwrap();
        assert!(texts(check_vault(&store, &vault)).contains("committing automatically"));
    }

    #[test]
    fn healthy_vault_has_no_warnings() {
        let dir = tempfile::tempdir().unwrap();
        git_init(dir.path());
        configure_obsidian_git(dir.path());
        let exe = std::env::current_exe().unwrap();
        hooks::install(dir.path(), &exe, &dir.path().join("log")).unwrap();
        let store = Store::open_in_memory().unwrap();
        let vault = store.add_vault("v", dir.path().to_str().unwrap(), "layout").unwrap();

        let checks = check_vault(&store, &vault);
        assert!(levels(&checks).iter().all(|l| *l == Level::Ok), "{checks:?}");
    }

    #[test]
    fn hook_pointing_at_a_missing_binary_fails() {
        let dir = tempfile::tempdir().unwrap();
        git_init(dir.path());
        hooks::install(dir.path(), Path::new("/nowhere/knowledge-bae"), &dir.path().join("log")).unwrap();
        let store = Store::open_in_memory().unwrap();
        let vault = store.add_vault("v", dir.path().to_str().unwrap(), "layout").unwrap();

        let checks = check_vault(&store, &vault);
        assert!(checks.iter().any(|c| c.level == Level::Fail && c.text.contains("no longer exists")), "{checks:?}");
    }

    #[test]
    fn partially_installed_hooks_are_named() {
        let dir = tempfile::tempdir().unwrap();
        git_init(dir.path());
        let exe = std::env::current_exe().unwrap();
        hooks::install(dir.path(), &exe, &dir.path().join("log")).unwrap();
        fs::remove_file(hooks::hooks_dir(dir.path()).unwrap().join("post-merge")).unwrap();
        let store = Store::open_in_memory().unwrap();
        let vault = store.add_vault("v", dir.path().to_str().unwrap(), "layout").unwrap();

        let checks = check_vault(&store, &vault);
        assert!(checks.iter().any(|c| c.level == Level::Warn && c.text.contains("hooks missing: post-merge")), "{checks:?}");
    }
}
