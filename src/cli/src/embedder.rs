//! The local embedding model (quantized bge-small-en-v1.5 via ONNX).
//!
//! The model is loaded on first use, so a session that never searches stays
//! tiny. Measured on macOS: ~10 MB before loading, ~175 MB after. The
//! unquantized model needs ~630 MB for nearly identical results. The runtime
//! does not give the memory back when the model is dropped, so there is no
//! idle unloading.

use std::path::PathBuf;

use anyhow::{Context, Result};
use fastembed::{EmbeddingModel, TextEmbedding, TextInitOptions};

use crate::ingest::{normalize, Embedder};

pub const MODEL_NAME: &str = "bge-small-en-v1.5-q";
pub const DIMS: usize = 384;
const BATCH_SIZE: usize = 16;
/// bge models retrieve better when queries (not documents) carry this instruction.
const QUERY_PREFIX: &str = "Represent this sentence for searching relevant passages: ";

pub struct LocalEmbedder {
    cache_dir: PathBuf,
    show_progress: bool,
    model: Option<TextEmbedding>,
}

impl LocalEmbedder {
    /// Does not load the model; that happens on the first embed.
    pub fn new(cache_dir: PathBuf, show_progress: bool) -> Self {
        Self { cache_dir, show_progress, model: None }
    }

    pub fn is_loaded(&self) -> bool {
        self.model.is_some()
    }

    fn model(&mut self) -> Result<&mut TextEmbedding> {
        if self.model.is_none() {
            let options = TextInitOptions::new(EmbeddingModel::BGESmallENV15Q)
                .with_cache_dir(self.cache_dir.clone())
                .with_show_download_progress(self.show_progress);
            let model = TextEmbedding::try_new(options)
                .context("loading the embedding model (the first run downloads it)")?;
            self.model = Some(model);
        }
        Ok(self.model.as_mut().expect("just loaded"))
    }
}

impl Embedder for LocalEmbedder {
    fn model(&self) -> &str {
        MODEL_NAME
    }

    fn dims(&self) -> usize {
        DIMS
    }

    fn embed(&mut self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        let vectors = self.model()?.embed(texts, Some(BATCH_SIZE))?;
        Ok(vectors)
    }

    fn embed_query(&mut self, query: &str) -> Result<Vec<f32>> {
        let prefixed = format!("{QUERY_PREFIX}{query}");
        let mut vector = self
            .model()?
            .embed(vec![prefixed], Some(1))?
            .pop()
            .context("embedder returned no vector")?;
        normalize(&mut vector);
        Ok(vector)
    }
}

/// Where downloaded models are kept: next to the index, outside any vault.
pub fn default_cache_dir() -> Result<PathBuf> {
    let db = crate::store::default_db_path()?;
    let dir = db.parent().context("index path has no parent directory")?;
    Ok(dir.join("models"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rss_mb() -> f64 {
        let out = std::process::Command::new("ps")
            .args(["-o", "rss=", "-p", &std::process::id().to_string()])
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).trim().parse::<f64>().unwrap_or(0.0) / 1024.0
    }

    fn dot(a: &[f32], b: &[f32]) -> f32 {
        a.iter().zip(b).map(|(x, y)| x * y).sum()
    }

    /// Downloads the model (~130 MB) on first run, so it only runs on request:
    /// `cargo test embedder -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn real_model_ranks_related_text_higher_and_reports_memory() {
        let dir = tempfile::tempdir().unwrap();
        let cache = std::env::var_os("KNOWLEDGE_BAE_TEST_MODEL_CACHE")
            .map(PathBuf::from)
            .unwrap_or_else(|| dir.path().join("models"));
        let mut embedder = LocalEmbedder::new(cache, true);

        println!("RSS before load: {:.0} MB", rss_mb());
        assert!(!embedder.is_loaded());

        let docs: Vec<String> = [
            "Backing up and restoring the Postgres database with pg_dump",
            "How to deploy the web client to CloudFront",
            "Recipe for sourdough bread with a long cold ferment",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let mut vectors = embedder.embed(&docs).unwrap();
        vectors.iter_mut().for_each(|v| normalize(v));
        assert!(embedder.is_loaded());
        assert_eq!(vectors[0].len(), DIMS);
        println!("RSS with model loaded: {:.0} MB", rss_mb());

        let query = embedder.embed_query("how do I restore a database backup").unwrap();
        let scores: Vec<f32> = vectors.iter().map(|v| dot(&query, v)).collect();
        println!("scores: {scores:?}");
        assert!(scores[0] > scores[1] && scores[0] > scores[2]);

    }
}
