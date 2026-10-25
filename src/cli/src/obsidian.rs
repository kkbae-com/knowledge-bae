//! Reads and fixes the Obsidian Git plugin's configuration inside a vault.
//!
//! knowledge-bae indexes on git commits, so the vault needs the Obsidian Git
//! plugin installed, enabled, and set to commit automatically. Those settings,
//! plus turning off push and pull for a vault with no remote, are all that is
//! touched; everything else in the user's config is preserved.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde_json::{json, Map, Value};

pub const PLUGIN_ID: &str = "obsidian-git";
pub const DEFAULT_INTERVAL_MINUTES: u64 = 5;

#[derive(Debug, Clone, PartialEq)]
pub struct GitPluginStatus {
    /// The vault has an `.obsidian` folder (it has been opened in Obsidian).
    pub is_obsidian_vault: bool,
    /// The plugin's files are in the vault.
    pub installed: bool,
    /// The plugin is listed in `community-plugins.json`.
    pub enabled: bool,
    /// Minutes between automatic commits; 0 means off.
    pub auto_commit_minutes: u64,
    /// Commit a while after edits stop.
    pub auto_commit_after_edits: bool,
    /// Each automatic commit also pushes, or pulls first (the plugin's defaults).
    pub syncs_on_commit: bool,
}

impl GitPluginStatus {
    pub fn auto_commit_on(&self) -> bool {
        self.auto_commit_minutes > 0 || self.auto_commit_after_edits
    }

    /// Syncing was on but there is nothing to sync with.
    pub fn sync_needs_disabling(&self, has_remote: bool) -> bool {
        self.syncs_on_commit && !has_remote
    }

    pub fn is_ready(&self) -> bool {
        self.installed && self.enabled && self.auto_commit_on()
    }
}

fn obsidian_dir(vault: &Path) -> PathBuf {
    vault.join(".obsidian")
}

fn plugin_dir(vault: &Path) -> PathBuf {
    obsidian_dir(vault).join("plugins").join(PLUGIN_ID)
}

fn enabled_list_path(vault: &Path) -> PathBuf {
    obsidian_dir(vault).join("community-plugins.json")
}

fn settings_path(vault: &Path) -> PathBuf {
    plugin_dir(vault).join("data.json")
}

pub fn inspect(vault: &Path) -> Result<GitPluginStatus> {
    let dir = plugin_dir(vault);
    let settings = read_json_object(&settings_path(vault))?.unwrap_or_default();
    Ok(GitPluginStatus {
        is_obsidian_vault: obsidian_dir(vault).is_dir(),
        installed: dir.join("main.js").is_file() && dir.join("manifest.json").is_file(),
        enabled: read_enabled_list(vault)?.iter().any(|p| p == PLUGIN_ID),
        auto_commit_minutes: settings.get("autoSaveInterval").and_then(Value::as_u64).unwrap_or(0),
        auto_commit_after_edits: settings
            .get("autoBackupAfterFileChange")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        syncs_on_commit: !bool_setting(&settings, "disablePush", false) || bool_setting(&settings, "pullBeforePush", true),
    })
}

fn bool_setting(settings: &Map<String, Value>, key: &str, default: bool) -> bool {
    settings.get(key).and_then(Value::as_bool).unwrap_or(default)
}

/// Whether the vault's git repository has any remote configured.
pub fn has_remote(vault: &Path) -> bool {
    std::process::Command::new("git")
        .arg("-C")
        .arg(vault)
        .arg("remote")
        .output()
        .map(|o| o.status.success() && !o.stdout.iter().all(u8::is_ascii_whitespace))
        .unwrap_or(false)
}

/// Enables the plugin and turns on automatic commits if neither is already set.
/// With no git remote it also turns off push and pull, which would only fail.
/// Returns a description of each change made; empty means nothing needed doing.
pub fn ensure(vault: &Path, interval_minutes: u64) -> Result<Vec<String>> {
    let status = inspect(vault)?;
    if !status.is_obsidian_vault {
        bail!("{} has no .obsidian folder. Open the folder as a vault in Obsidian once first.", vault.display());
    }
    if !status.installed {
        bail!(
            "the Obsidian Git plugin is not installed in this vault. In Obsidian: Settings > Community plugins > Browse > \"Git\" > Install, then run this again."
        );
    }

    let mut changes = Vec::new();

    if !status.enabled {
        let mut list = read_enabled_list(vault)?;
        list.push(PLUGIN_ID.to_string());
        write_json(&enabled_list_path(vault), &json!(list))?;
        changes.push("enabled the Obsidian Git plugin".to_string());
    }

    let no_sync = status.sync_needs_disabling(has_remote(vault));
    if !status.auto_commit_on() || no_sync {
        let path = settings_path(vault);
        let mut settings = read_json_object(&path)?.unwrap_or_default();
        if !status.auto_commit_on() {
            settings.insert("autoSaveInterval".into(), json!(interval_minutes));
            changes.push(format!("set the plugin to commit automatically every {interval_minutes} minutes"));
        }
        if no_sync {
            settings.insert("disablePush".into(), json!(true));
            settings.insert("pullBeforePush".into(), json!(false));
            changes.push("turned off push and pull on commit, since this vault has no git remote".to_string());
        }
        write_json(&path, &Value::Object(settings))?;
    }
    Ok(changes)
}

fn read_enabled_list(vault: &Path) -> Result<Vec<String>> {
    let path = enabled_list_path(vault);
    let Some(text) = read_if_exists(&path)? else { return Ok(Vec::new()) };
    let value: Value = serde_json::from_str(&text)
        .with_context(|| format!("{} is not valid JSON; fix or remove it", path.display()))?;
    let Value::Array(items) = value else {
        bail!("{} should be a JSON list of plugin ids", path.display());
    };
    Ok(items.into_iter().filter_map(|v| v.as_str().map(String::from)).collect())
}

fn read_json_object(path: &Path) -> Result<Option<Map<String, Value>>> {
    let Some(text) = read_if_exists(path)? else { return Ok(None) };
    match serde_json::from_str::<Value>(&text)
        .with_context(|| format!("{} is not valid JSON; fix or remove it", path.display()))?
    {
        Value::Object(map) => Ok(Some(map)),
        _ => bail!("{} should be a JSON object", path.display()),
    }
}

fn read_if_exists(path: &Path) -> Result<Option<String>> {
    match std::fs::read_to_string(path) {
        Ok(t) => Ok(Some(t)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
    }
}

/// Two-space indentation, like Obsidian writes it, so diffs stay small.
fn write_json(path: &Path, value: &Value) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, serde_json::to_string_pretty(value)?)
        .with_context(|| format!("writing {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn vault(installed: bool) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join(".obsidian")).unwrap();
        if installed {
            let p = plugin_dir(dir.path());
            fs::create_dir_all(&p).unwrap();
            fs::write(p.join("main.js"), "//").unwrap();
            fs::write(p.join("manifest.json"), "{}").unwrap();
        }
        dir
    }

    #[test]
    fn reports_each_missing_piece() {
        let plain = tempfile::tempdir().unwrap();
        assert!(!inspect(plain.path()).unwrap().is_obsidian_vault);

        let none = vault(false);
        let s = inspect(none.path()).unwrap();
        assert!(s.is_obsidian_vault && !s.installed && !s.enabled && !s.auto_commit_on());

        let installed = vault(true);
        let s = inspect(installed.path()).unwrap();
        assert!(s.installed && !s.enabled && !s.is_ready());
    }

    #[test]
    fn ensure_enables_and_sets_an_interval_on_a_fresh_install() {
        let dir = vault(true);
        let changes = ensure(dir.path(), 5).unwrap();
        assert_eq!(changes.len(), 3);

        let s = inspect(dir.path()).unwrap();
        assert!(s.is_ready());
        assert_eq!(s.auto_commit_minutes, 5);
        assert_eq!(ensure(dir.path(), 5).unwrap(), Vec::<String>::new());
    }

    fn git(dir: &Path, args: &[&str]) {
        assert!(std::process::Command::new("git").arg("-C").arg(dir).args(args).status().unwrap().success());
    }

    #[test]
    fn push_and_pull_are_turned_off_only_without_a_remote() {
        let local = vault(true);
        git(local.path(), &["init", "-q"]);
        assert!(!has_remote(local.path()));
        assert!(inspect(local.path()).unwrap().syncs_on_commit);
        ensure(local.path(), 5).unwrap();
        let v: Value = serde_json::from_str(&fs::read_to_string(settings_path(local.path())).unwrap()).unwrap();
        assert_eq!((v["disablePush"].clone(), v["pullBeforePush"].clone()), (json!(true), json!(false)));
        assert!(!inspect(local.path()).unwrap().syncs_on_commit);

        let remote = vault(true);
        git(remote.path(), &["init", "-q"]);
        git(remote.path(), &["remote", "add", "origin", "https://example.com/vault.git"]);
        assert!(has_remote(remote.path()));
        ensure(remote.path(), 5).unwrap();
        let v: Value = serde_json::from_str(&fs::read_to_string(settings_path(remote.path())).unwrap()).unwrap();
        assert!(v.get("disablePush").is_none() && v.get("pullBeforePush").is_none());
        assert!(inspect(remote.path()).unwrap().syncs_on_commit);
    }

    #[test]
    fn ensure_keeps_other_plugins_and_other_settings_in_order() {
        let dir = vault(true);
        fs::write(enabled_list_path(dir.path()), r#"["dataview", "calendar"]"#).unwrap();
        fs::write(
            settings_path(dir.path()),
            r#"{"zeta": 1, "commitMessage": "mine", "autoSaveInterval": 0, "alpha": {"x": true}}"#,
        )
        .unwrap();

        ensure(dir.path(), 10).unwrap();

        let list: Vec<String> = serde_json::from_str(&fs::read_to_string(enabled_list_path(dir.path())).unwrap()).unwrap();
        assert_eq!(list, vec!["dataview", "calendar", "obsidian-git"]);
        let text = fs::read_to_string(settings_path(dir.path())).unwrap();
        assert!(text.find("zeta").unwrap() < text.find("commitMessage").unwrap(), "key order changed:\n{text}");
        assert!(text.find("commitMessage").unwrap() < text.find("alpha").unwrap());
        let v: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["autoSaveInterval"], 10);
        assert_eq!(v["commitMessage"], "mine");
        assert_eq!(v["alpha"]["x"], true);
    }

    #[test]
    fn existing_auto_commit_choices_are_respected() {
        let dir = vault(true);
        fs::write(enabled_list_path(dir.path()), r#"["obsidian-git"]"#).unwrap();
        fs::write(settings_path(dir.path()), r#"{"autoSaveInterval": 0, "autoBackupAfterFileChange": true, "disablePush": true, "pullBeforePush": false}"#).unwrap();
        assert!(ensure(dir.path(), 5).unwrap().is_empty());
        assert_eq!(inspect(dir.path()).unwrap().auto_commit_minutes, 0);

        let other = vault(true);
        fs::write(enabled_list_path(other.path()), r#"["obsidian-git"]"#).unwrap();
        fs::write(settings_path(other.path()), r#"{"autoSaveInterval": 30, "disablePush": true, "pullBeforePush": false}"#).unwrap();
        assert!(ensure(other.path(), 5).unwrap().is_empty());
        assert_eq!(inspect(other.path()).unwrap().auto_commit_minutes, 30);
    }

    #[test]
    fn refuses_when_it_cannot_help_safely() {
        let not_vault = tempfile::tempdir().unwrap();
        assert!(ensure(not_vault.path(), 5).unwrap_err().to_string().contains(".obsidian"));

        let not_installed = vault(false);
        let err = ensure(not_installed.path(), 5).unwrap_err().to_string();
        assert!(err.contains("not installed"), "{err}");
        assert!(!enabled_list_path(not_installed.path()).exists());

        let broken = vault(true);
        fs::write(settings_path(broken.path()), "{ not json").unwrap();
        let err = ensure(broken.path(), 5).unwrap_err().to_string();
        assert!(err.contains("not valid JSON"), "{err}");
        assert_eq!(fs::read_to_string(settings_path(broken.path())).unwrap(), "{ not json");
    }
}
