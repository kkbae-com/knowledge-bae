//! The local index: vaults, preferences, notes and chunks in one SQLite file.
//!
//! The index is derived data. The vault is the source of truth, so the file can
//! always be rebuilt with `ingest --all`. Embeddings are stored as normalized
//! little-endian f32 blobs and searched by brute force, streaming rows so
//! memory stays flat regardless of index size.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use rusqlite::{params, Connection, OptionalExtension};

use crate::chunkers::ParsedNote;

const MIGRATIONS: &[&str] = &[r#"
    CREATE TABLE preferences (
        key   TEXT PRIMARY KEY,
        value TEXT NOT NULL
    );
    CREATE TABLE meta (
        key   TEXT PRIMARY KEY,
        value TEXT NOT NULL
    );
    CREATE TABLE vaults (
        id         INTEGER PRIMARY KEY,
        name       TEXT NOT NULL UNIQUE,
        path       TEXT NOT NULL UNIQUE,
        structure  TEXT NOT NULL DEFAULT '',
        created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
    );
    CREATE TABLE notes (
        id       INTEGER PRIMARY KEY,
        vault_id INTEGER NOT NULL REFERENCES vaults(id) ON DELETE CASCADE,
        path     TEXT NOT NULL,
        title    TEXT NOT NULL,
        tags     TEXT NOT NULL DEFAULT '[]',
        aliases  TEXT NOT NULL DEFAULT '[]',
        links    TEXT NOT NULL DEFAULT '[]',
        UNIQUE (vault_id, path)
    );
    CREATE TABLE chunks (
        id           INTEGER PRIMARY KEY,
        note_id      INTEGER NOT NULL REFERENCES notes(id) ON DELETE CASCADE,
        ordinal      INTEGER NOT NULL,
        heading_path TEXT NOT NULL DEFAULT '[]',
        text         TEXT NOT NULL,
        links        TEXT NOT NULL DEFAULT '[]',
        hash         TEXT NOT NULL,
        embedding    BLOB
    );
    CREATE INDEX chunks_note ON chunks (note_id);
    CREATE INDEX chunks_hash ON chunks (hash);
"#];

#[derive(Debug, Clone, PartialEq)]
pub struct Vault {
    pub id: i64,
    pub name: String,
    pub path: String,
    pub structure: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Hit {
    pub score: f32,
    pub vault: String,
    pub note_path: String,
    pub title: String,
    pub heading_path: Vec<String>,
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Stats {
    pub vaults: usize,
    pub notes: usize,
    pub chunks: usize,
    pub unembedded_chunks: usize,
}

pub struct Store {
    conn: Connection,
}

/// `KNOWLEDGE_BAE_DB` if set, otherwise the platform data directory.
pub fn default_db_path() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("KNOWLEDGE_BAE_DB") {
        return Ok(PathBuf::from(path));
    }
    let dir = dirs::data_dir().context("could not determine a data directory")?;
    Ok(dir.join("knowledge-bae").join("index.db"))
}

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        let conn = Connection::open(path).with_context(|| format!("opening {}", path.display()))?;
        // Several processes share this file: the MCP server and short-lived hook runs.
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.busy_timeout(std::time::Duration::from_secs(10))?;
        Self::init(conn)
    }

    #[cfg(test)]
    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Self> {
        conn.pragma_update(None, "foreign_keys", "ON")?;
        let mut store = Self { conn };
        store.migrate()?;
        Ok(store)
    }

    fn migrate(&mut self) -> Result<()> {
        let current = self.conn.pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))? as usize;
        if current > MIGRATIONS.len() {
            bail!(
                "index was created by a newer knowledge-bae (schema {current}, this build knows {})",
                MIGRATIONS.len()
            );
        }
        for (i, sql) in MIGRATIONS.iter().enumerate().skip(current) {
            let tx = self.conn.transaction()?;
            tx.execute_batch(sql)?;
            tx.pragma_update(None, "user_version", (i + 1) as i64)?;
            tx.commit()?;
        }
        Ok(())
    }

    // Preferences

    pub fn set_preference(&self, key: &str, value: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO preferences (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    pub fn get_preference(&self, key: &str) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row("SELECT value FROM preferences WHERE key = ?1", [key], |r| r.get(0))
            .optional()?)
    }

    // Vaults

    pub fn add_vault(&self, name: &str, path: &str, structure: &str) -> Result<Vault> {
        self.conn
            .execute(
                "INSERT INTO vaults (name, path, structure) VALUES (?1, ?2, ?3)",
                params![name, path, structure],
            )
            .with_context(|| format!("adding vault '{name}' (name and path must be unique)"))?;
        Ok(Vault {
            id: self.conn.last_insert_rowid(),
            name: name.to_string(),
            path: path.to_string(),
            structure: structure.to_string(),
        })
    }

    pub fn list_vaults(&self) -> Result<Vec<Vault>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, name, path, structure FROM vaults ORDER BY name")?;
        let rows = stmt.query_map([], vault_from_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Looks a vault up by name or by path.
    pub fn find_vault(&self, name_or_path: &str) -> Result<Option<Vault>> {
        Ok(self
            .conn
            .query_row(
                "SELECT id, name, path, structure FROM vaults WHERE name = ?1 OR path = ?1",
                [name_or_path],
                vault_from_row,
            )
            .optional()?)
    }

    pub fn set_vault_structure(&self, vault_id: i64, structure: &str) -> Result<()> {
        let changed = self.conn.execute(
            "UPDATE vaults SET structure = ?1 WHERE id = ?2",
            params![structure, vault_id],
        )?;
        if changed == 0 {
            bail!("no vault with id {vault_id}");
        }
        Ok(())
    }

    /// Removes the vault and, by cascade, its notes and chunks.
    pub fn remove_vault(&self, vault_id: i64) -> Result<bool> {
        Ok(self.conn.execute("DELETE FROM vaults WHERE id = ?1", [vault_id])? > 0)
    }

    // Embedding model

    /// Records which model produced the embeddings. If it differs from the one
    /// on file, every stored embedding is cleared (vectors from different models
    /// are not comparable) and `true` is returned so the caller can re-ingest.
    pub fn ensure_embedding_model(&mut self, model: &str, dims: usize) -> Result<bool> {
        let wanted = format!("{model}:{dims}");
        let on_file: Option<String> = self
            .conn
            .query_row("SELECT value FROM meta WHERE key = 'embedding_model'", [], |r| r.get(0))
            .optional()?;
        if on_file.as_deref() == Some(wanted.as_str()) {
            return Ok(false);
        }
        let tx = self.conn.transaction()?;
        tx.execute("UPDATE chunks SET embedding = NULL", [])?;
        tx.execute(
            "INSERT INTO meta (key, value) VALUES ('embedding_model', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            [&wanted],
        )?;
        tx.commit()?;
        Ok(on_file.is_some())
    }

    // Notes

    /// Embeddings already stored for these chunk hashes, so unchanged chunks are never re-embedded.
    pub fn reusable_embeddings(&self, hashes: &[String]) -> Result<HashMap<String, Vec<f32>>> {
        let mut out = HashMap::new();
        let mut stmt = self
            .conn
            .prepare("SELECT embedding FROM chunks WHERE hash = ?1 AND embedding IS NOT NULL LIMIT 1")?;
        for hash in hashes {
            if out.contains_key(hash) {
                continue;
            }
            let blob: Option<Vec<u8>> = stmt.query_row([hash], |r| r.get(0)).optional()?;
            if let Some(blob) = blob {
                out.insert(hash.clone(), blob_to_vec(&blob));
            }
        }
        Ok(out)
    }

    /// Replaces everything stored for the note at `path`. `embeddings` maps chunk
    /// hash to its (already normalized) vector; chunks without one are stored
    /// unembedded and can be filled in by a later ingest.
    pub fn replace_note(
        &mut self,
        vault_id: i64,
        path: &str,
        note: &ParsedNote,
        embeddings: &HashMap<String, Vec<f32>>,
    ) -> Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute(
            "DELETE FROM notes WHERE vault_id = ?1 AND path = ?2",
            params![vault_id, path],
        )?;
        tx.execute(
            "INSERT INTO notes (vault_id, path, title, tags, aliases, links)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                vault_id,
                path,
                note.title,
                json(&note.tags),
                json(&note.aliases),
                json(&note.links),
            ],
        )?;
        let note_id = tx.last_insert_rowid();
        {
            let mut insert = tx.prepare(
                "INSERT INTO chunks (note_id, ordinal, heading_path, text, links, hash, embedding)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            )?;
            for (ordinal, chunk) in note.chunks.iter().enumerate() {
                let blob = embeddings.get(&chunk.hash).map(|v| vec_to_blob(v));
                insert.execute(params![
                    note_id,
                    ordinal as i64,
                    json(&chunk.heading_path),
                    chunk.text,
                    json(&chunk.links),
                    chunk.hash,
                    blob,
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Moves a note without touching its chunks or embeddings.
    pub fn rename_note(&self, vault_id: i64, from: &str, to: &str) -> Result<bool> {
        self.conn.execute(
            "DELETE FROM notes WHERE vault_id = ?1 AND path = ?2",
            params![vault_id, to],
        )?;
        let changed = self.conn.execute(
            "UPDATE notes SET path = ?3 WHERE vault_id = ?1 AND path = ?2",
            params![vault_id, from, to],
        )?;
        Ok(changed > 0)
    }

    pub fn delete_note(&self, vault_id: i64, path: &str) -> Result<bool> {
        let changed = self.conn.execute(
            "DELETE FROM notes WHERE vault_id = ?1 AND path = ?2",
            params![vault_id, path],
        )?;
        Ok(changed > 0)
    }

    pub fn note_paths(&self, vault_id: i64) -> Result<Vec<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT path FROM notes WHERE vault_id = ?1 ORDER BY path")?;
        let rows = stmt.query_map([vault_id], |r| r.get(0))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn stats(&self) -> Result<Stats> {
        let count = |sql: &str| -> Result<usize> {
            Ok(self.conn.query_row(sql, [], |r| r.get::<_, i64>(0))? as usize)
        };
        Ok(Stats {
            vaults: count("SELECT COUNT(*) FROM vaults")?,
            notes: count("SELECT COUNT(*) FROM notes")?,
            chunks: count("SELECT COUNT(*) FROM chunks")?,
            unembedded_chunks: count("SELECT COUNT(*) FROM chunks WHERE embedding IS NULL")?,
        })
    }

    // Search

    /// Nearest chunks by cosine similarity. `query` must be normalized, like the stored vectors.
    pub fn search(&self, query: &[f32], vault_id: Option<i64>, limit: usize) -> Result<Vec<Hit>> {
        let mut scored: Vec<(f32, i64)> = Vec::new();
        {
            let mut stmt = self.conn.prepare(
                "SELECT c.id, c.embedding FROM chunks c
                 JOIN notes n ON n.id = c.note_id
                 WHERE c.embedding IS NOT NULL AND (?1 IS NULL OR n.vault_id = ?1)",
            )?;
            let mut rows = stmt.query([vault_id])?;
            while let Some(row) = rows.next()? {
                let id: i64 = row.get(0)?;
                let blob: Vec<u8> = row.get(1)?;
                let score = dot_blob(query, &blob);
                scored.push((score, id));
            }
        }
        scored.sort_by(|a, b| b.0.total_cmp(&a.0));
        scored.truncate(limit);

        let mut stmt = self.conn.prepare(
            "SELECT v.name, n.path, n.title, c.heading_path, c.text
             FROM chunks c
             JOIN notes n ON n.id = c.note_id
             JOIN vaults v ON v.id = n.vault_id
             WHERE c.id = ?1",
        )?;
        let mut hits = Vec::with_capacity(scored.len());
        for (score, id) in scored {
            let hit = stmt.query_row([id], |r| {
                let heading_path: String = r.get(3)?;
                Ok(Hit {
                    score,
                    vault: r.get(0)?,
                    note_path: r.get(1)?,
                    title: r.get(2)?,
                    heading_path: serde_json::from_str(&heading_path).unwrap_or_default(),
                    text: r.get(4)?,
                })
            })?;
            hits.push(hit);
        }
        Ok(hits)
    }
}

fn vault_from_row(r: &rusqlite::Row) -> rusqlite::Result<Vault> {
    Ok(Vault {
        id: r.get(0)?,
        name: r.get(1)?,
        path: r.get(2)?,
        structure: r.get(3)?,
    })
}

fn json<T: serde::Serialize>(value: &T) -> String {
    serde_json::to_string(value).expect("serializing strings cannot fail")
}

fn vec_to_blob(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|f| f.to_le_bytes()).collect()
}

fn blob_to_vec(blob: &[u8]) -> Vec<f32> {
    blob.chunks_exact(4)
        .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .collect()
}

fn dot_blob(query: &[f32], blob: &[u8]) -> f32 {
    query
        .iter()
        .zip(blob.chunks_exact(4))
        .map(|(q, b)| q * f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunkers::parse_note;

    fn unit(v: &[f32]) -> Vec<f32> {
        let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        v.iter().map(|x| x / norm).collect()
    }

    fn note(title: &str, body: &str) -> ParsedNote {
        parse_note(title, body)
    }

    fn embed_all(note: &ParsedNote, v: &[f32]) -> HashMap<String, Vec<f32>> {
        note.chunks.iter().map(|c| (c.hash.clone(), unit(v))).collect()
    }

    #[test]
    fn vaults_and_preferences_round_trip() {
        let store = Store::open_in_memory().unwrap();
        let vault = store.add_vault("work", "/vaults/work", "stories/ bugs/").unwrap();
        assert_eq!(store.find_vault("work").unwrap().unwrap(), vault);
        assert_eq!(store.find_vault("/vaults/work").unwrap().unwrap(), vault);
        assert!(store.add_vault("work", "/elsewhere", "").is_err());

        store.set_vault_structure(vault.id, "new layout").unwrap();
        assert_eq!(store.list_vaults().unwrap()[0].structure, "new layout");

        assert_eq!(store.get_preference("k").unwrap(), None);
        store.set_preference("k", "1").unwrap();
        store.set_preference("k", "2").unwrap();
        assert_eq!(store.get_preference("k").unwrap().as_deref(), Some("2"));
    }

    #[test]
    fn replace_note_replaces_instead_of_duplicating() {
        let mut store = Store::open_in_memory().unwrap();
        let v = store.add_vault("v", "/v", "").unwrap();
        let first = note("A", "## One\n\nfirst version\n");
        store.replace_note(v.id, "A.md", &first, &HashMap::new()).unwrap();
        let second = note("A", "## One\n\nsecond version\n");
        store.replace_note(v.id, "A.md", &second, &HashMap::new()).unwrap();

        let stats = store.stats().unwrap();
        assert_eq!((stats.notes, stats.chunks), (1, 1));
        assert_eq!(store.note_paths(v.id).unwrap(), vec!["A.md"]);
    }

    #[test]
    fn unchanged_chunks_reuse_embeddings_and_moves_keep_them() {
        let mut store = Store::open_in_memory().unwrap();
        let v = store.add_vault("v", "/v", "").unwrap();
        let n = note("A", "## One\n\nsome text\n");
        store.replace_note(v.id, "A.md", &n, &embed_all(&n, &[1.0, 0.0])).unwrap();

        let hashes: Vec<String> = n.chunks.iter().map(|c| c.hash.clone()).collect();
        assert_eq!(store.reusable_embeddings(&hashes).unwrap().len(), 1);

        assert!(store.rename_note(v.id, "A.md", "stories/A.md").unwrap());
        assert_eq!(store.note_paths(v.id).unwrap(), vec!["stories/A.md"]);
        assert_eq!(store.stats().unwrap().unembedded_chunks, 0);
    }

    #[test]
    fn search_ranks_by_cosine_and_filters_by_vault() {
        let mut store = Store::open_in_memory().unwrap();
        let a = store.add_vault("a", "/a", "").unwrap();
        let b = store.add_vault("b", "/b", "").unwrap();
        let near = note("Near", "## S\n\nnear text\n");
        let far = note("Far", "## S\n\nfar text\n");
        let other = note("Other", "## S\n\nother vault text\n");
        store.replace_note(a.id, "Near.md", &near, &embed_all(&near, &[1.0, 0.1])).unwrap();
        store.replace_note(a.id, "Far.md", &far, &embed_all(&far, &[0.0, 1.0])).unwrap();
        store.replace_note(b.id, "Other.md", &other, &embed_all(&other, &[1.0, 0.0])).unwrap();

        let query = unit(&[1.0, 0.0]);
        let all = store.search(&query, None, 10).unwrap();
        assert_eq!(all.len(), 3);
        assert!(all[0].score >= all[1].score && all[1].score >= all[2].score);
        assert_eq!(all[2].title, "Far");

        let only_a = store.search(&query, Some(a.id), 10).unwrap();
        assert_eq!(only_a.len(), 2);
        assert_eq!(only_a[0].title, "Near");
        assert_eq!(only_a[0].vault, "a");
        assert_eq!(only_a[0].heading_path, vec!["S"]);

        assert_eq!(store.search(&query, None, 1).unwrap().len(), 1);
    }

    #[test]
    fn unembedded_chunks_are_skipped_by_search() {
        let mut store = Store::open_in_memory().unwrap();
        let v = store.add_vault("v", "/v", "").unwrap();
        let n = note("A", "## S\n\ntext\n");
        store.replace_note(v.id, "A.md", &n, &HashMap::new()).unwrap();
        assert_eq!(store.stats().unwrap().unembedded_chunks, 1);
        assert!(store.search(&unit(&[1.0, 0.0]), None, 5).unwrap().is_empty());
    }

    #[test]
    fn changing_embedding_model_clears_vectors() {
        let mut store = Store::open_in_memory().unwrap();
        let v = store.add_vault("v", "/v", "").unwrap();
        assert!(!store.ensure_embedding_model("bge-small", 384).unwrap());
        let n = note("A", "## S\n\ntext\n");
        store.replace_note(v.id, "A.md", &n, &embed_all(&n, &[1.0, 0.0])).unwrap();

        assert!(!store.ensure_embedding_model("bge-small", 384).unwrap());
        assert_eq!(store.stats().unwrap().unembedded_chunks, 0);

        assert!(store.ensure_embedding_model("other-model", 768).unwrap());
        assert_eq!(store.stats().unwrap().unembedded_chunks, 1);
    }

    #[test]
    fn removing_a_vault_removes_its_notes_and_chunks() {
        let mut store = Store::open_in_memory().unwrap();
        let v = store.add_vault("v", "/v", "").unwrap();
        let n = note("A", "## S\n\ntext\n");
        store.replace_note(v.id, "A.md", &n, &HashMap::new()).unwrap();
        assert!(store.remove_vault(v.id).unwrap());
        let stats = store.stats().unwrap();
        assert_eq!((stats.vaults, stats.notes, stats.chunks), (0, 0, 0));
    }

    #[test]
    fn deleting_a_note_removes_its_chunks() {
        let mut store = Store::open_in_memory().unwrap();
        let v = store.add_vault("v", "/v", "").unwrap();
        let n = note("A", "## S\n\ntext\n");
        store.replace_note(v.id, "A.md", &n, &HashMap::new()).unwrap();
        assert!(store.delete_note(v.id, "A.md").unwrap());
        assert!(!store.delete_note(v.id, "A.md").unwrap());
        assert_eq!(store.stats().unwrap().chunks, 0);
    }

    #[test]
    fn reopening_a_file_keeps_data_and_schema_version() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("index.db");
        {
            let store = Store::open(&path).unwrap();
            store.add_vault("v", "/v", "").unwrap();
        }
        let store = Store::open(&path).unwrap();
        assert_eq!(store.list_vaults().unwrap().len(), 1);
    }

    #[test]
    fn newer_schema_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("index.db");
        {
            let conn = Connection::open(&path).unwrap();
            conn.pragma_update(None, "user_version", 99).unwrap();
        }
        let err = Store::open(&path).err().unwrap().to_string();
        assert!(err.contains("newer"), "{err}");
    }
}
