mod chunkers;
mod embedder;
mod git;
mod ingest;
mod store;

use std::io::IsTerminal;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use clap::{Args, Parser, Subcommand};

use embedder::LocalEmbedder;
use ingest::{Embedder, Report};
use store::{Store, Vault};

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
    /// Bring the index up to date with a vault
    Ingest(IngestArgs),
    /// Search the index
    Search(SearchArgs),
    /// Show what is in the index
    Status,
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
    /// Unregister a vault and delete its index data (the vault itself is untouched)
    Remove { name: String },
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
        Command::Ingest(args) => ingest_command(args),
        Command::Search(args) => search_command(args),
        Command::Status => status_command(),
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
