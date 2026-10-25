//! Brings the index in line with a vault on disk.
//!
//! `ingest_all` walks the vault, re-chunks every note that changed, embeds only
//! the chunks it has not seen before, and drops notes that no longer exist.

use std::collections::HashSet;
use std::path::Path;

use anyhow::{bail, Context, Result};
use regex::Regex;
use sha2::{Digest, Sha256};
use walkdir::WalkDir;

use crate::chunkers::parse_note;
use crate::store::{Store, Vault};

const EMBED_BATCH: usize = 32;

pub trait Embedder {
    /// Identifies the model; a different value invalidates every stored vector.
    fn model(&self) -> &str;
    fn dims(&self) -> usize;
    fn embed(&mut self, texts: &[String]) -> Result<Vec<Vec<f32>>>;
}

#[derive(Debug, Default, PartialEq)]
pub struct Report {
    pub added: usize,
    pub updated: usize,
    pub unchanged: usize,
    pub deleted: usize,
    pub chunks_embedded: usize,
    pub chunks_reused: usize,
    /// Files that could not be indexed, with the reason. One bad file never stops the run.
    pub failed: Vec<(String, String)>,
}

#[derive(Debug, PartialEq)]
enum Outcome {
    Added,
    Updated,
    Unchanged,
}

pub fn ingest_all(store: &mut Store, vault: &Vault, embedder: &mut dyn Embedder) -> Result<Report> {
    let root = Path::new(&vault.path);
    if !root.is_dir() {
        bail!("vault '{}' is not a directory: {}", vault.name, vault.path);
    }
    store.ensure_embedding_model(embedder.model(), embedder.dims())?;

    let files = vault_files(root)?;
    let on_disk: HashSet<&str> = files.iter().map(String::as_str).collect();
    let mut report = Report::default();

    for rel in &files {
        match ingest_file(store, vault, rel, embedder, &mut report) {
            Ok(Outcome::Added) => report.added += 1,
            Ok(Outcome::Updated) => report.updated += 1,
            Ok(Outcome::Unchanged) => report.unchanged += 1,
            Err(e) if e.is::<EmbedderError>() => return Err(e),
            Err(e) => report.failed.push((rel.clone(), format!("{e:#}"))),
        }
    }

    for stored in store.note_paths(vault.id)? {
        if !on_disk.contains(stored.as_str()) && store.delete_note(vault.id, &stored)? {
            report.deleted += 1;
        }
    }
    Ok(report)
}

/// Marks failures of the embedder itself, which should stop a run instead of being logged per file.
#[derive(Debug)]
struct EmbedderError(String);

impl std::fmt::Display for EmbedderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "embedding failed: {}", self.0)
    }
}

impl std::error::Error for EmbedderError {}

fn ingest_file(
    store: &mut Store,
    vault: &Vault,
    rel: &str,
    embedder: &mut dyn Embedder,
    report: &mut Report,
) -> Result<Outcome> {
    let bytes = std::fs::read(Path::new(&vault.path).join(rel)).context("reading file")?;
    let source_hash = hex(&Sha256::digest(&bytes));
    let state = store.note_state(vault.id, rel)?;
    if let Some((stored_hash, unembedded)) = &state {
        if *stored_hash == source_hash && *unembedded == 0 {
            return Ok(Outcome::Unchanged);
        }
    }

    let source = String::from_utf8(bytes).context("not valid UTF-8")?;
    let title = Path::new(rel)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let note = parse_note(&title, &source);

    let hashes: Vec<String> = note.chunks.iter().map(|c| c.hash.clone()).collect();
    let mut embeddings = store.reusable_embeddings(&hashes)?;
    report.chunks_reused += embeddings.len();

    let mut missing: Vec<(&str, &str)> = Vec::new();
    let mut seen = HashSet::new();
    for chunk in &note.chunks {
        if !embeddings.contains_key(&chunk.hash) && seen.insert(chunk.hash.as_str()) {
            missing.push((&chunk.hash, &chunk.embed_text));
        }
    }
    for batch in missing.chunks(EMBED_BATCH) {
        let texts: Vec<String> = batch.iter().map(|(_, t)| t.to_string()).collect();
        let vectors = embedder
            .embed(&texts)
            .map_err(|e| EmbedderError(format!("{e:#}")))?;
        if vectors.len() != texts.len() || vectors.iter().any(|v| v.len() != embedder.dims()) {
            return Err(EmbedderError("embedder returned the wrong number or size of vectors".into()).into());
        }
        for ((hash, _), mut vector) in batch.iter().zip(vectors) {
            normalize(&mut vector);
            embeddings.insert(hash.to_string(), vector);
        }
        report.chunks_embedded += batch.len();
    }

    store.replace_note(vault.id, rel, &source_hash, &note, &embeddings)?;
    Ok(if state.is_some() { Outcome::Updated } else { Outcome::Added })
}

/// Vault-relative paths (always `/`-separated) of the notes to index, sorted.
pub fn vault_files(root: &Path) -> Result<Vec<String>> {
    let filters = ignore_filters(root);
    let mut files = Vec::new();
    let walker = WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| e.depth() == 0 || !e.file_name().to_string_lossy().starts_with('.'));
    for entry in walker {
        let entry = entry.context("walking vault")?;
        if !entry.file_type().is_file() {
            continue;
        }
        let is_markdown = entry
            .path()
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("md"));
        if !is_markdown {
            continue;
        }
        let rel = entry
            .path()
            .strip_prefix(root)
            .context("path outside vault")?
            .components()
            .map(|c| c.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/");
        if !filters.iter().any(|f| f.matches(&rel)) {
            files.push(rel);
        }
    }
    files.sort();
    Ok(files)
}

enum IgnoreFilter {
    Prefix(String),
    Pattern(Regex),
}

impl IgnoreFilter {
    fn matches(&self, rel: &str) -> bool {
        match self {
            Self::Prefix(p) => rel.starts_with(p.as_str()),
            Self::Pattern(re) => re.is_match(rel),
        }
    }
}

/// Obsidian's "Excluded files" setting: path prefixes, or `/regex/` entries.
fn ignore_filters(root: &Path) -> Vec<IgnoreFilter> {
    let Ok(text) = std::fs::read_to_string(root.join(".obsidian").join("app.json")) else {
        return Vec::new();
    };
    let Ok(serde_json::Value::Object(settings)) = serde_json::from_str::<serde_json::Value>(&text) else {
        return Vec::new();
    };
    let Some(serde_json::Value::Array(entries)) = settings.get("userIgnoreFilters") else {
        return Vec::new();
    };
    entries
        .iter()
        .filter_map(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .filter_map(|s| {
            if s.len() > 2 && s.starts_with('/') && s.ends_with('/') {
                Regex::new(&s[1..s.len() - 1]).ok().map(IgnoreFilter::Pattern)
            } else {
                Some(IgnoreFilter::Prefix(s.to_string()))
            }
        })
        .collect()
}

fn normalize(v: &mut [f32]) {
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        v.iter_mut().for_each(|x| *x /= norm);
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// Hashes words into a small vector, so related text lands near each other.
    struct FakeEmbedder {
        model: String,
        embedded_texts: usize,
    }

    impl FakeEmbedder {
        fn new() -> Self {
            Self { model: "fake".into(), embedded_texts: 0 }
        }
    }

    impl Embedder for FakeEmbedder {
        fn model(&self) -> &str {
            &self.model
        }
        fn dims(&self) -> usize {
            32
        }
        fn embed(&mut self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
            self.embedded_texts += texts.len();
            Ok(texts
                .iter()
                .map(|t| {
                    let mut v = vec![0.0f32; 32];
                    for word in t.split_whitespace() {
                        let h = word.bytes().fold(7usize, |a, b| a.wrapping_mul(31).wrapping_add(b as usize));
                        v[h % 32] += 1.0;
                    }
                    v
                })
                .collect())
        }
    }

    struct BrokenEmbedder;
    impl Embedder for BrokenEmbedder {
        fn model(&self) -> &str {
            "broken"
        }
        fn dims(&self) -> usize {
            4
        }
        fn embed(&mut self, _: &[String]) -> Result<Vec<Vec<f32>>> {
            bail!("model not available")
        }
    }

    fn setup(files: &[(&str, &str)]) -> (tempfile::TempDir, Store, Vault) {
        let dir = tempfile::tempdir().unwrap();
        for (path, body) in files {
            let full = dir.path().join(path);
            fs::create_dir_all(full.parent().unwrap()).unwrap();
            fs::write(full, body).unwrap();
        }
        let store = Store::open_in_memory().unwrap();
        let vault = store.add_vault("v", dir.path().to_str().unwrap(), "").unwrap();
        (dir, store, vault)
    }

    #[test]
    fn indexes_notes_and_ignores_hidden_and_non_markdown() {
        let (_dir, mut store, vault) = setup(&[
            ("Alpha.md", "## A\n\nalpha text\n"),
            ("proj/Beta.md", "## B\n\nbeta text\n"),
            (".obsidian/plugin.md", "## no\n\nhidden\n"),
            (".trash/Gone.md", "## no\n\ntrashed\n"),
            ("image.png", "not markdown"),
            ("notes.txt", "not markdown"),
        ]);
        let report = ingest_all(&mut store, &vault, &mut FakeEmbedder::new()).unwrap();
        assert_eq!((report.added, report.updated, report.unchanged), (2, 0, 0));
        assert_eq!(store.note_paths(vault.id).unwrap(), vec!["Alpha.md", "proj/Beta.md"]);
    }

    #[test]
    fn second_run_does_no_work() {
        let (_dir, mut store, vault) = setup(&[("A.md", "## S\n\ntext\n"), ("B.md", "## S\n\nmore\n")]);
        let mut embedder = FakeEmbedder::new();
        ingest_all(&mut store, &vault, &mut embedder).unwrap();
        let before = embedder.embedded_texts;

        let report = ingest_all(&mut store, &vault, &mut embedder).unwrap();
        assert_eq!((report.added, report.updated, report.unchanged), (0, 0, 2));
        assert_eq!(embedder.embedded_texts, before);
    }

    #[test]
    fn edited_note_reembeds_only_changed_chunks() {
        let section = |name: &str, word: &str| format!("## {name}\n\n{}\n\n", vec![word; 80].join(" "));
        let original = format!("{}{}", section("One", "red"), section("Two", "blue"));
        let (dir, mut store, vault) = setup(&[("A.md", &original)]);
        let mut embedder = FakeEmbedder::new();
        ingest_all(&mut store, &vault, &mut embedder).unwrap();
        let before = embedder.embedded_texts;

        let edited = format!("{}{}", section("One", "red"), section("Two", "green"));
        fs::write(dir.path().join("A.md"), edited).unwrap();
        let report = ingest_all(&mut store, &vault, &mut embedder).unwrap();

        assert_eq!((report.added, report.updated), (0, 1));
        assert_eq!(embedder.embedded_texts - before, 1);
        assert_eq!(report.chunks_reused, 1);
        assert_eq!(store.stats().unwrap().chunks, 2);
    }

    #[test]
    fn deleted_files_are_removed_from_the_index() {
        let (dir, mut store, vault) = setup(&[("A.md", "## S\n\ntext\n"), ("B.md", "## S\n\nmore\n")]);
        let mut embedder = FakeEmbedder::new();
        ingest_all(&mut store, &vault, &mut embedder).unwrap();

        fs::remove_file(dir.path().join("B.md")).unwrap();
        let report = ingest_all(&mut store, &vault, &mut embedder).unwrap();
        assert_eq!(report.deleted, 1);
        assert_eq!(store.note_paths(vault.id).unwrap(), vec!["A.md"]);
        assert_eq!(store.stats().unwrap().notes, 1);
    }

    #[test]
    fn moved_note_is_not_reembedded() {
        let (dir, mut store, vault) = setup(&[("investigations/Slug.md", "## S\n\nsome findings\n")]);
        let mut embedder = FakeEmbedder::new();
        ingest_all(&mut store, &vault, &mut embedder).unwrap();
        let before = embedder.embedded_texts;

        fs::create_dir_all(dir.path().join("stories/1-x/investigations")).unwrap();
        fs::rename(
            dir.path().join("investigations/Slug.md"),
            dir.path().join("stories/1-x/investigations/Slug.md"),
        )
        .unwrap();
        let report = ingest_all(&mut store, &vault, &mut embedder).unwrap();

        assert_eq!((report.added, report.deleted), (1, 1));
        assert_eq!(embedder.embedded_texts, before);
        assert_eq!(store.note_paths(vault.id).unwrap(), vec!["stories/1-x/investigations/Slug.md"]);
    }

    #[test]
    fn obsidian_ignore_filters_are_honored() {
        let (_dir, mut store, vault) = setup(&[
            (".obsidian/app.json", r#"{"userIgnoreFilters": ["Archive/", "/^drafts\\/.*wip/"]}"#),
            ("Keep.md", "## S\n\nkeep\n"),
            ("Archive/Old.md", "## S\n\nold\n"),
            ("drafts/a-wip.md", "## S\n\nwip\n"),
            ("drafts/final.md", "## S\n\nfinal\n"),
        ]);
        ingest_all(&mut store, &vault, &mut FakeEmbedder::new()).unwrap();
        assert_eq!(store.note_paths(vault.id).unwrap(), vec!["Keep.md", "drafts/final.md"]);
    }

    #[test]
    fn changing_the_model_reembeds_everything() {
        let (_dir, mut store, vault) = setup(&[("A.md", "## S\n\ntext\n")]);
        let mut embedder = FakeEmbedder::new();
        ingest_all(&mut store, &vault, &mut embedder).unwrap();
        let before = embedder.embedded_texts;

        embedder.model = "other".into();
        let report = ingest_all(&mut store, &vault, &mut embedder).unwrap();
        assert_eq!(report.updated, 1);
        assert_eq!(embedder.embedded_texts - before, 1);
        assert_eq!(store.stats().unwrap().unembedded_chunks, 0);
    }

    #[test]
    fn a_bad_file_is_reported_and_the_rest_still_index() {
        let (dir, mut store, vault) = setup(&[("Good.md", "## S\n\nfine\n")]);
        fs::write(dir.path().join("Bad.md"), [0xff, 0xfe, 0xfd]).unwrap();
        let report = ingest_all(&mut store, &vault, &mut FakeEmbedder::new()).unwrap();
        assert_eq!(report.added, 1);
        assert_eq!(report.failed.len(), 1);
        assert_eq!(report.failed[0].0, "Bad.md");
        assert_eq!(store.note_paths(vault.id).unwrap(), vec!["Good.md"]);
    }

    #[test]
    fn embedder_failure_stops_the_run() {
        let (_dir, mut store, vault) = setup(&[("A.md", "## S\n\ntext\n")]);
        let err = ingest_all(&mut store, &vault, &mut BrokenEmbedder).unwrap_err();
        assert!(err.to_string().contains("embedding failed"), "{err}");
        assert_eq!(store.stats().unwrap().notes, 0);
    }

    #[test]
    fn missing_vault_directory_is_an_error() {
        let store_vault = {
            let store = Store::open_in_memory().unwrap();
            let vault = store.add_vault("v", "/definitely/not/here", "").unwrap();
            (store, vault)
        };
        let (mut store, vault) = store_vault;
        assert!(ingest_all(&mut store, &vault, &mut FakeEmbedder::new()).is_err());
    }

    #[test]
    fn indexed_notes_are_searchable() {
        let (_dir, mut store, vault) = setup(&[
            ("Cats.md", "## Pets\n\nthe cat sat on the mat\n"),
            ("Rust.md", "## Lang\n\nborrow checker lifetimes traits\n"),
        ]);
        let mut embedder = FakeEmbedder::new();
        ingest_all(&mut store, &vault, &mut embedder).unwrap();

        let mut query = embedder.embed(&["borrow checker lifetimes".to_string()]).unwrap().remove(0);
        normalize(&mut query);
        let hits = store.search(&query, None, 2).unwrap();
        assert_eq!(hits[0].title, "Rust");
        assert_eq!(hits[0].note_path, "Rust.md");
    }
}
