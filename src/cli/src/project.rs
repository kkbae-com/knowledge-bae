//! Project records: adding and editing them, and the summary style that applies to each.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

use crate::git;
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
    let dirs = a.dirs.iter().map(|d| dir_entry(d)).collect::<Result<Vec<_>>>()?;
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

pub fn require_project(store: &Store, name: &str) -> Result<Project> {
    store
        .find_project(name)?
        .with_context(|| format!("no project named '{name}' (see: knowledge-bae project list)"))
}

fn absolute(dir: &Path) -> Result<String> {
    Ok(std::path::absolute(dir)?.to_string_lossy().into_owned())
}

/// An existing directory as a project directory, with the origin of the repo it is in (if any).
fn dir_entry(dir: &Path) -> Result<ProjectDir> {
    if !dir.is_dir() {
        bail!("{} is not a directory", dir.display());
    }
    Ok(ProjectDir { path: absolute(dir)?, origin: git::origin_url(dir).unwrap_or_default() })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn add_args(name: &str, dirs: Vec<PathBuf>) -> AddArgs {
        AddArgs { name: name.into(), dirs, description: None, vault_id: None, locations: vec![], number: None, misc: None }
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
            dirs: vec![ProjectDir { path: "/p".into(), origin: "git@x:p.git".into() }],
            ..Project::default()
        };
        let text = describe(&p, Some("kkbae"));
        for needle in ["p", "does things", "P-1", "note", "work/p", "/p", "git@x:p.git", "kkbae", "2-3 sentences"] {
            assert!(text.contains(needle), "missing {needle}: {text}");
        }
    }
}
