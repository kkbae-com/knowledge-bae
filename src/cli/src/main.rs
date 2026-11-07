mod chunkers;
mod doctor;
mod embedder;
mod git;
mod hooks;
mod ingest;
mod obsidian;
mod project;
mod project_notes;
mod repos;
mod skill;
mod store;
mod week;

use std::io::IsTerminal;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use clap::{Args, Parser, Subcommand};

use embedder::LocalEmbedder;
use ingest::{Embedder, Report};
use store::{Store, Vault};

/// The built-in developer vault layout, offered during onboarding.
const DEFAULT_STRUCTURE: &str = include_str!("../defaults/vault-structure.md");

#[derive(Parser)]
#[command(name = "knowledge-bae", version, about = "Local semantic search for Obsidian vaults")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Manage the vaults that get indexed
    Vault {
        #[command(subcommand)]
        action: VaultAction,
    },
    /// Install or remove the git hooks that keep a vault's index current
    Hooks {
        #[command(subcommand)]
        action: HooksAction,
    },
    /// Bring the index up to date with a vault
    Ingest(IngestArgs),
    /// Search the index
    Search(SearchArgs),
    /// Show what is in the index
    Status,
    /// Check the index, each vault and its hooks for problems
    Doctor,
    /// Check or fix the Obsidian Git plugin settings in a vault
    Obsidian {
        #[command(subcommand)]
        action: ObsidianAction,
    },
    /// Install the bundled Claude Code skill
    Skill {
        #[command(subcommand)]
        action: SkillAction,
    },
    /// Track projects and the weekly work done on them
    Project {
        #[command(subcommand)]
        action: ProjectAction,
    },
}

#[derive(Subcommand)]
enum VaultAction {
    /// Register a vault
    Add {
        /// Folder containing the vault
        path: PathBuf,
        /// Name for the vault (defaults to the folder name)
        #[arg(long)]
        name: Option<String>,
        /// File holding the instructions for how this vault should be structured
        #[arg(long)]
        structure_file: Option<PathBuf>,
    },
    /// List registered vaults
    List,
    /// Show or change the instructions for how a vault should be organized
    Structure {
        /// Vault name or path (defaults to the vault containing the current directory)
        vault: Option<String>,
        /// Replace the instructions with the contents of this file
        #[arg(long, conflicts_with = "set_default")]
        set_file: Option<PathBuf>,
        /// Replace the instructions with the built-in developer vault layout
        #[arg(long)]
        set_default: bool,
    },
    /// Unregister a vault and delete its index data (the vault itself is untouched)
    Remove { name: String },
}

#[derive(Subcommand)]
enum ObsidianAction {
    /// Show whether the Obsidian Git plugin is installed, enabled and committing automatically
    Check {
        /// Vault name or path (defaults to the vault containing the current directory)
        #[arg(long)]
        vault: Option<String>,
    },
    /// Enable the plugin and turn on automatic commits if they are off (other settings are left alone)
    Setup {
        #[arg(long)]
        vault: Option<String>,
        /// Minutes between automatic commits, used only if no automatic commit is set yet
        #[arg(long, default_value_t = obsidian::DEFAULT_INTERVAL_MINUTES)]
        interval: u64,
    },
}

#[derive(Subcommand)]
enum ProjectAction {
    /// Register a project
    Add {
        name: String,
        /// A folder (usually a git repo) that belongs to the project; repeat for several
        #[arg(long = "dir", required = true)]
        dirs: Vec<PathBuf>,
        #[arg(long)]
        description: Option<String>,
        /// The vault this project relates to
        #[arg(long)]
        vault: Option<String>,
        /// A folder or note inside the vault that relates to the project; repeat for several
        #[arg(long = "location")]
        locations: Vec<String>,
        /// Project number in a time tracker application
        #[arg(long)]
        number: Option<String>,
        /// Anything else worth remembering
        #[arg(long)]
        misc: Option<String>,
        /// A repo beneath a folder to leave out (its path in the folder, or its folder name); repeat for several
        #[arg(long = "exclude")]
        excludes: Vec<String>,
    },
    /// List projects
    List,
    /// Show everything about a project
    Show { name: String },
    /// Change a project (only the fields you pass are changed)
    Edit {
        name: String,
        /// Add a folder to the project
        #[arg(long = "dir")]
        dirs: Vec<PathBuf>,
        /// Remove a folder from the project
        #[arg(long = "remove-dir")]
        remove_dirs: Vec<PathBuf>,
        #[arg(long)]
        description: Option<String>,
        /// A vault name, or `none` to unlink the project from its vault
        #[arg(long)]
        vault: Option<String>,
        /// Add a vault location
        #[arg(long = "location")]
        locations: Vec<String>,
        #[arg(long)]
        number: Option<String>,
        #[arg(long)]
        misc: Option<String>,
        /// Leave a repo beneath a folder out of the project
        #[arg(long = "exclude")]
        excludes: Vec<String>,
        /// Take a repo back out of the exclude list
        #[arg(long = "include")]
        includes: Vec<String>,
    },
    /// Forget a project (its folders and vault notes are untouched)
    Remove { name: String },
    /// Show or change how weekly summaries for a project are written
    SummaryStyle {
        name: String,
        /// Use the contents of this file as the project's instructions
        #[arg(long, conflicts_with = "reset")]
        set_file: Option<PathBuf>,
        /// Go back to the built-in default (a 2-3 sentence timesheet paragraph)
        #[arg(long)]
        reset: bool,
    },
    /// Show or set the first day of the week (default sunday)
    WeekStart { day: Option<String> },
    /// Write a note per project into its vault, as a backup you can restore with `project import`
    Export {
        /// Only this vault (default: every vault)
        #[arg(long)]
        vault: Option<String>,
    },
    /// Restore projects from the notes `project export` wrote
    Import {
        /// Only this vault (default: every vault)
        #[arg(long)]
        vault: Option<String>,
        /// Replace projects that already exist (default: leave them alone)
        #[arg(long)]
        overwrite: bool,
    },
    /// List a project's repos and mark which had commits in a week
    Repos {
        name: String,
        /// `this`, `last`, or a date (the week containing it)
        #[arg(long, default_value = "last")]
        week: String,
        /// Look for commits by this email instead of each repo's own user.email
        #[arg(long)]
        author: Option<String>,
    },
    /// Gather a week's commits and changed vault notes per project, for writing summaries
    Activity {
        /// A project name (default: every project)
        name: Option<String>,
        /// `this`, `last`, or a date (the week containing it)
        #[arg(long, default_value = "last")]
        week: String,
        /// Look for commits by this email instead of each repo's own user.email
        #[arg(long)]
        author: Option<String>,
    },
}

#[derive(Subcommand)]
enum SkillAction {
    /// Copy the skill into Claude Code's skills folder (re-run after upgrading to refresh it)
    Install {
        /// Skills folder to install into (default: ~/.claude/skills; use .claude/skills for one project)
        #[arg(long)]
        dir: Option<PathBuf>,
    },
}

#[derive(Subcommand)]
enum HooksAction {
    /// Add the hooks to the vault's git repository (safe to re-run)
    Install {
        /// Vault name or path (defaults to the vault containing the current directory)
        #[arg(long)]
        vault: Option<String>,
    },
    /// Remove only the knowledge-bae parts of the hooks
    Uninstall {
        #[arg(long)]
        vault: Option<String>,
    },
    /// Show which hooks are installed
    Status {
        #[arg(long)]
        vault: Option<String>,
    },
}

#[derive(Args)]
struct IngestArgs {
    /// Vault name or path (defaults to the vault containing the current directory)
    #[arg(long)]
    vault: Option<String>,
    /// Index the whole vault
    #[arg(long, conflicts_with = "changed", required_unless_present = "changed")]
    all: bool,
    /// Index only what the latest git commit changed (for git hooks)
    #[arg(long)]
    changed: bool,
    /// With --changed: compare from this revision (e.g. ORIG_HEAD) instead of the latest commit
    #[arg(long, requires = "changed", requires = "to")]
    from: Option<String>,
    /// With --changed: compare up to this revision
    #[arg(long, requires = "from")]
    to: Option<String>,
}

#[derive(Args)]
struct SearchArgs {
    query: String,
    /// Search only this vault (default: all vaults)
    #[arg(long)]
    vault: Option<String>,
    #[arg(long, short = 'n', default_value_t = 5)]
    limit: usize,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Vault { action } => vault_command(action),
        Command::Hooks { action } => hooks_command(action),
        Command::Ingest(args) => ingest_command(args),
        Command::Search(args) => search_command(args),
        Command::Status => status_command(),
        Command::Doctor => doctor_command(),
        Command::Obsidian { action } => obsidian_command(action),
        Command::Skill { action } => skill_command(action),
        Command::Project { action } => project_command(action),
    }
}

fn open_store() -> Result<Store> {
    Store::open(&store::default_db_path()?)
}

fn new_embedder() -> Result<LocalEmbedder> {
    Ok(LocalEmbedder::new(
        embedder::default_cache_dir()?,
        std::io::stderr().is_terminal(),
    ))
}

fn vault_command(action: VaultAction) -> Result<()> {
    let store = open_store()?;
    match action {
        VaultAction::Add { path, name, structure_file } => {
            if !path.is_dir() {
                bail!("{} is not a directory", path.display());
            }
            let abs = std::path::absolute(&path)?;
            let name = match name {
                Some(n) => n,
                None => abs
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .context("could not derive a vault name; pass --name")?,
            };
            let structure = match structure_file {
                Some(f) => std::fs::read_to_string(&f)
                    .with_context(|| format!("reading {}", f.display()))?,
                None => String::new(),
            };
            let vault = store.add_vault(&name, &abs.to_string_lossy(), &structure)?;
            println!("Added vault '{}' at {}", vault.name, vault.path);
            println!("Next: knowledge-bae ingest --vault {} --all", vault.name);
        }
        VaultAction::List => {
            let vaults = store.list_vaults()?;
            if vaults.is_empty() {
                println!("No vaults yet. Add one with: knowledge-bae vault add <path>");
            }
            for v in vaults {
                let structure = if v.structure.is_empty() { "no structure set" } else { "structure set" };
                println!("{}  {}  ({structure})", v.name, v.path);
            }
        }
        VaultAction::Structure { vault, set_file, set_default } => {
            let vault = resolve_vault(&store, vault.as_deref())?;
            let new = match (set_file, set_default) {
                (Some(file), _) => Some(
                    std::fs::read_to_string(&file)
                        .with_context(|| format!("reading {}", file.display()))?,
                ),
                (None, true) => Some(DEFAULT_STRUCTURE.to_string()),
                (None, false) => None,
            };
            match new {
                Some(text) => {
                    store.set_vault_structure(vault.id, &text)?;
                    println!("Updated the structure instructions for '{}'", vault.name);
                }
                None if vault.structure.trim().is_empty() => {
                    println!("No structure instructions are set for '{}'.", vault.name);
                    println!("Use the built-in layout with: knowledge-bae vault structure {} --set-default", vault.name);
                }
                None => print!("{}", vault.structure),
            }
        }
        VaultAction::Remove { name } => {
            let vault = find_vault(&store, &name)?;
            store.remove_vault(vault.id)?;
            println!("Removed vault '{}' from the index", vault.name);
        }
    }
    Ok(())
}

fn find_vault(store: &Store, name_or_path: &str) -> Result<Vault> {
    store
        .find_vault(name_or_path)?
        .with_context(|| format!("no vault named or located at '{name_or_path}' (see: knowledge-bae vault list)"))
}

/// The vault to act on: the one named, else the one containing the current
/// directory (how git hooks run), else the only one registered.
fn resolve_vault(store: &Store, name: Option<&str>) -> Result<Vault> {
    if let Some(name) = name {
        return find_vault(store, name);
    }
    let vaults = store.list_vaults()?;
    if let Ok(cwd) = std::env::current_dir().and_then(|d| d.canonicalize()) {
        let containing = vaults
            .iter()
            .filter(|v| Path::new(&v.path).canonicalize().is_ok_and(|p| cwd.starts_with(p)))
            .max_by_key(|v| v.path.len());
        if let Some(v) = containing {
            return Ok(v.clone());
        }
    }
    match vaults.as_slice() {
        [only] => Ok(only.clone()),
        [] => bail!("no vaults registered (knowledge-bae vault add <path>)"),
        _ => bail!("several vaults are registered; pass --vault <name>"),
    }
}

fn hooks_command(action: HooksAction) -> Result<()> {
    let store = open_store()?;
    match action {
        HooksAction::Install { vault } => {
            let vault = resolve_vault(&store, vault.as_deref())?;
            let exe = std::env::current_exe().context("finding this program's path")?;
            let log = store::default_db_path()?.with_file_name("hooks.log");
            let installed = hooks::install(Path::new(&vault.path), &exe, &log)?;
            let names: Vec<_> = installed.iter().map(|h| h.file_name()).collect();
            println!("Installed {} in {}", names.join(", "), vault.path);
            println!("Hooks run {} and log to {}", exe.display(), log.display());
            if exe.components().any(|c| c.as_os_str() == "target") {
                eprintln!("Warning: that looks like a build directory. Run `cargo install --path src/cli` and install the hooks from the installed binary so they keep working.");
            }
        }
        HooksAction::Uninstall { vault } => {
            let vault = resolve_vault(&store, vault.as_deref())?;
            let removed = hooks::uninstall(Path::new(&vault.path))?;
            if removed.is_empty() {
                println!("No knowledge-bae hooks were installed in {}", vault.path);
            } else {
                let names: Vec<_> = removed.iter().map(|h| h.file_name()).collect();
                println!("Removed {} from {}", names.join(", "), vault.path);
            }
        }
        HooksAction::Status { vault } => {
            let vault = resolve_vault(&store, vault.as_deref())?;
            for (hook, state) in hooks::status(Path::new(&vault.path))? {
                let state = if state == hooks::HookState::Installed { "installed" } else { "not installed" };
                println!("{:<14} {state}", hook.file_name());
            }
        }
    }
    Ok(())
}

fn ingest_command(args: IngestArgs) -> Result<()> {
    let mut store = open_store()?;
    let vault = resolve_vault(&store, args.vault.as_deref())?;
    let mut embedder = new_embedder()?;

    let report = if args.changed {
        let root = Path::new(&vault.path);
        let changes = match (&args.from, &args.to) {
            (Some(from), Some(to)) => git::changes_between(root, from, to)?,
            _ => git::changes_in_head(root)?,
        };
        ingest::ingest_changes(&mut store, &vault, &mut embedder, &changes)?
    } else {
        ingest::ingest_all(&mut store, &vault, &mut embedder)?
    };
    print_report(&vault, &report);
    Ok(())
}

fn print_report(vault: &Vault, r: &Report) {
    println!(
        "{}: {} added, {} updated, {} unchanged, {} moved, {} deleted ({} chunks embedded, {} reused)",
        vault.name, r.added, r.updated, r.unchanged, r.moved, r.deleted, r.chunks_embedded, r.chunks_reused
    );
    for (path, reason) in &r.failed {
        eprintln!("  skipped {path}: {reason}");
    }
}

fn search_command(args: SearchArgs) -> Result<()> {
    let store = open_store()?;
    let vault_id = match &args.vault {
        Some(name) => Some(find_vault(&store, name)?.id),
        None => None,
    };
    let stats = store.stats()?;
    if stats.chunks == stats.unembedded_chunks {
        println!("Nothing is indexed yet. Add a vault and index it:");
        println!("  knowledge-bae vault add <path>");
        println!("  knowledge-bae ingest --vault <name> --all");
        return Ok(());
    }
    let query = new_embedder()?.embed_query(&args.query)?;
    let hits = store.search(&query, vault_id, args.limit)?;
    if hits.is_empty() {
        println!("No results. Has the vault been indexed? (knowledge-bae status)");
    }
    for (i, hit) in hits.iter().enumerate() {
        let mut place = hit.title.clone();
        for heading in &hit.heading_path {
            place.push_str(" > ");
            place.push_str(heading);
        }
        println!("{}. [{:.2}] {place}", i + 1, hit.score);
        println!("   {}:{}", hit.vault, hit.note_path);
        let snippet: String = hit.text.split_whitespace().collect::<Vec<_>>().join(" ");
        let snippet: String = snippet.chars().take(200).collect();
        println!("   {snippet}");
    }
    Ok(())
}

fn status_command() -> Result<()> {
    let path = store::default_db_path()?;
    let store = Store::open(&path)?;
    let stats = store.stats()?;
    println!("Index:   {}", path.display());
    println!("Model:   {}", embedder::MODEL_NAME);
    println!(
        "Vaults:  {}   Notes: {}   Chunks: {} ({} without embeddings)",
        stats.vaults, stats.notes, stats.chunks, stats.unembedded_chunks
    );
    Ok(())
}

fn doctor_command() -> Result<()> {
    use doctor::Level;
    let path = store::default_db_path()?;
    let store = Store::open(&path)?;
    let mut failed = false;
    let mut show = |checks: Vec<doctor::Check>| {
        for c in checks {
            let mark = match c.level {
                Level::Ok => "ok  ",
                Level::Info => "info",
                Level::Warn => "warn",
                Level::Fail => "FAIL",
            };
            failed |= c.level == Level::Fail;
            println!("  [{mark}] {}", c.text);
        }
    };

    println!("Environment");
    show(doctor::check_environment());
    println!("Index ({})", path.display());
    show(doctor::check_index(&store));
    let vaults = store.list_vaults()?;
    if vaults.is_empty() {
        println!("Vaults\n  [warn] none registered (knowledge-bae vault add <path>)");
    }
    for vault in vaults {
        println!("Vault '{}' ({})", vault.name, vault.path);
        show(doctor::check_vault(&store, &vault));
    }
    println!("Projects");
    show(doctor::check_projects(&store));
    if failed {
        std::process::exit(1);
    }
    Ok(())
}

/// The named vault, or every registered vault.
fn selected_vaults(store: &Store, name: Option<&str>) -> Result<Vec<Vault>> {
    match name {
        Some(name) => Ok(vec![find_vault(store, name)?]),
        None => store.list_vaults(),
    }
}

fn project_command(action: ProjectAction) -> Result<()> {
    let mut store = open_store()?;
    match action {
        ProjectAction::Add { name, dirs, description, vault, locations, number, misc, excludes } => {
            let vault_id = match vault {
                Some(v) => Some(find_vault(&store, &v)?.id),
                None => None,
            };
            let p = project::add(
                &mut store,
                project::AddArgs { name, dirs, description, vault_id, locations, number, misc, excludes },
            )?;
            println!("Added project '{}' with {} folder(s)", p.name, p.dirs.len());
            for line in project::scan_summary(&p) {
                println!("{line}");
            }
            if p.vault_id.is_none() {
                println!("It is not linked to a vault yet (project edit {} --vault <name>), so it cannot be exported.", p.name);
            }
            println!("{}", project::EXPORT_TIP);
        }
        ProjectAction::List => {
            let projects = store.list_projects()?;
            if projects.is_empty() {
                println!("No projects yet. Add one with: knowledge-bae project add <name> --dir <folder>");
            }
            let vaults = store.list_vaults()?;
            for p in projects {
                let vault = vaults.iter().find(|v| Some(v.id) == p.vault_id).map_or("no vault", |v| v.name.as_str());
                let number = if p.project_number.is_empty() { String::new() } else { format!("  #{}", p.project_number) };
                println!("{}{number}  ({vault}, {} folder(s))", p.name, p.dirs.len());
            }
        }
        ProjectAction::Show { name } => {
            let p = project::require_project(&store, &name)?;
            let vault = store.list_vaults()?.into_iter().find(|v| Some(v.id) == p.vault_id);
            print!("{}", project::describe(&p, vault.as_ref().map(|v| v.name.as_str())));
        }
        ProjectAction::Edit { name, dirs, remove_dirs, description, vault, locations, number, misc, excludes, includes } => {
            let vault = match vault.as_deref() {
                None => project::VaultChange::Keep,
                Some("none") => project::VaultChange::Unlink,
                Some(v) => project::VaultChange::Set(find_vault(&store, v)?.id),
            };
            let p = project::edit(
                &mut store,
                project::EditArgs {
                    name,
                    add_dirs: dirs,
                    remove_dirs,
                    description,
                    vault,
                    add_locations: locations,
                    number,
                    misc,
                    excludes,
                    includes,
                },
            )?;
            println!("Updated project '{}'", p.name);
            for line in project::scan_summary(&p) {
                println!("{line}");
            }
            println!("{}", project::EXPORT_TIP);
        }
        ProjectAction::Remove { name } => {
            let p = project::require_project(&store, &name)?;
            store.remove_project(p.id)?;
            println!("Removed project '{}'", p.name);
            println!("{}", project::EXPORT_TIP);
        }
        ProjectAction::SummaryStyle { name, set_file, reset } => {
            let text = match (&set_file, reset) {
                (Some(f), _) => Some(std::fs::read_to_string(f).with_context(|| format!("reading {}", f.display()))?),
                (None, true) => None,
                (None, false) => {
                    print!("{}", project::effective_style(&project::require_project(&store, &name)?));
                    return Ok(());
                }
            };
            let p = project::set_summary_style(&mut store, &name, text)?;
            println!("Updated the summary instructions for '{}'", p.name);
            println!("{}", project::EXPORT_TIP);
        }
        ProjectAction::Export { vault } => {
            let vaults = selected_vaults(&store, vault.as_deref())?;
            let report = project_notes::export(&store, &vaults)?;
            for (vault, file) in &report.written {
                println!("Wrote {} ({vault})", file.display());
            }
            for name in &report.skipped {
                println!("Skipped '{name}': not linked to a vault (project edit {name} --vault <name>)");
            }
            for conflict in &report.conflicts {
                eprintln!("warning: {conflict}");
            }
            if report.written.is_empty() {
                println!("Nothing was exported.");
            } else {
                let mut embedder = new_embedder()?;
                for vault in vaults.iter().filter(|v| report.written.iter().any(|(name, _)| *name == v.name)) {
                    print_report(vault, &ingest::ingest_all(&mut store, vault, &mut embedder)?);
                }
            }
        }
        ProjectAction::Import { vault, overwrite } => {
            let vaults = selected_vaults(&store, vault.as_deref())?;
            let report = project_notes::import(&mut store, &vaults, overwrite)?;
            for name in &report.added {
                println!("Added '{name}'");
            }
            for name in &report.updated {
                println!("Replaced '{name}'");
            }
            for name in &report.skipped {
                println!("Kept the existing '{name}' (use --overwrite to replace it with the note)");
            }
            for warning in &report.warnings {
                eprintln!("warning: {warning}");
            }
            if report.added.is_empty() && report.updated.is_empty() && report.skipped.is_empty() {
                println!("No project notes found in {}/ of the selected vault(s).", project_notes::NOTES_DIR);
            }
        }
        ProjectAction::Repos { name, week, author } => {
            let p = project::require_project(&store, &name)?;
            let start = week::parse_weekday(&store.week_start()?)?;
            let (first, last) = week::week_bounds(week::today(), start, week::parse_week_sel(&week)?);
            print!("{}", project::render_repos(&p, first, last, author.as_deref()));
        }
        ProjectAction::Activity { name, week, author } => {
            let projects = match name {
                Some(name) => vec![project::require_project(&store, &name)?],
                None => store.list_projects()?,
            };
            if projects.is_empty() {
                println!("No projects yet. Add one with: knowledge-bae project add <name> --dir <folder>");
            }
            let start = week::parse_weekday(&store.week_start()?)?;
            let (first, last) = week::week_bounds(week::today(), start, week::parse_week_sel(&week)?);
            for p in &projects {
                println!("{}", project::render_activity(&store, p, first, last, author.as_deref())?);
            }
        }
        ProjectAction::WeekStart { day } => match day {
            Some(day) => {
                println!("Weeks now start on {}", project::set_week_start(&store, &day)?);
            }
            None => println!("Weeks start on {}", store.week_start()?),
        },
    }
    Ok(())
}

fn skill_command(action: SkillAction) -> Result<()> {
    let SkillAction::Install { dir } = action;
    let dir = match dir {
        Some(d) => d,
        None => skill::personal_skills_dir()?,
    };
    let (file, replaced) = skill::install(&dir)?;
    println!("{} {}", if replaced { "Updated" } else { "Installed" }, file.display());
    println!("Start a new Claude Code session to pick it up.");
    Ok(())
}

fn obsidian_command(action: ObsidianAction) -> Result<()> {
    let store = open_store()?;
    match action {
        ObsidianAction::Check { vault } => {
            let vault = resolve_vault(&store, vault.as_deref())?;
            let s = obsidian::inspect(Path::new(&vault.path))?;
            let yes = |b: bool| if b { "yes" } else { "no" };
            println!("Obsidian vault (.obsidian folder): {}", yes(s.is_obsidian_vault));
            println!("Obsidian Git plugin installed:     {}", yes(s.installed));
            println!("Obsidian Git plugin enabled:       {}", yes(s.enabled));
            println!(
                "Automatic commits:                 {}",
                match (s.auto_commit_minutes, s.auto_commit_after_edits) {
                    (0, false) => "off".to_string(),
                    (0, true) => "after edits stop".to_string(),
                    (m, false) => format!("every {m} minutes"),
                    (m, true) => format!("every {m} minutes and after edits stop"),
                }
            );
            let remote = obsidian::has_remote(Path::new(&vault.path));
            println!("Git remote:                        {}", yes(remote));
            println!("Pushes and pulls on commit:        {}", yes(s.syncs_on_commit));
            if !s.is_ready() || s.sync_needs_disabling(remote) {
                println!("Fix what can be fixed with: knowledge-bae obsidian setup --vault {}", vault.name);
            }
        }
        ObsidianAction::Setup { vault, interval } => {
            let vault = resolve_vault(&store, vault.as_deref())?;
            let changes = obsidian::ensure(Path::new(&vault.path), interval)?;
            if changes.is_empty() {
                println!("Already set up: the plugin is enabled and committing automatically.");
            } else {
                for change in &changes {
                    println!("- {change}");
                }
                println!("Restart Obsidian (or run \"Reload app without saving\" from the command palette) so it picks this up.");
                println!("Obsidian keeps its settings in memory, so changes made while it is open can be overwritten if you skip this.");
            }
        }
    }
    Ok(())
}
