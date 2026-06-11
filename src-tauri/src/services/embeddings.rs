// 2.1 — Semantic codebase search via Ollama nomic-embed-text.
// Fallback graceful: si Ollama down, devolvemos error (caller cae a BM25/grep).
// Council V1: Ollama timeout 8s, no panic.
// V3: pool de 1 simultáneo, no spawn N processes.

use anyhow::{anyhow, Result};
use parking_lot::Mutex;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

// 041 FR-003 — default-localhost. No infra of el autor's in the distributed binary; a fresh install
// targets the local Ollama. Override via env `FURX_OLLAMA_URL` (the embedding service has no DB
// handle in scope, so env is the only knob today). Future: lift to settings_store from a State ctx.
const OLLAMA_URL_DEFAULT: &str = "http://localhost:11434";
fn ollama_url() -> String {
    std::env::var("FURX_OLLAMA_URL").unwrap_or_else(|_| OLLAMA_URL_DEFAULT.to_string())
}
const MODEL: &str = "nomic-embed-text";
const CHUNK_SIZE: usize = 1024;
const MAX_FILES_PER_INDEX: usize = 200;
const MAX_FILE_BYTES: usize = 256 * 1024;

#[derive(Debug, Clone, Serialize)]
pub struct SearchHit {
    pub file_path: String,
    pub chunk_id: i64,
    pub snippet: String,
    pub score: f32,
}

pub async fn embed_text(text: &str) -> Result<Vec<f32>> {
    if text.trim().is_empty() {
        return Err(anyhow!("empty text"));
    }
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(8))
        .build()?;
    #[derive(Deserialize)]
    struct EmbedResp {
        embedding: Vec<f32>,
    }
    let body = serde_json::json!({ "model": MODEL, "prompt": text });
    let resp = client
        .post(format!("{}/api/embeddings", ollama_url()))
        .json(&body)
        .send()
        .await?;
    if !resp.status().is_success() {
        return Err(anyhow!("ollama status {}", resp.status()));
    }
    let parsed: EmbedResp = resp.json().await?;
    Ok(parsed.embedding)
}

pub async fn index_project(db: Arc<Mutex<Connection>>, project: &Path) -> Result<usize> {
    let abs = project
        .canonicalize()
        .map_err(|e| anyhow!("canonicalize: {}", e))?;
    let home = dirs::home_dir().ok_or_else(|| anyhow!("no home"))?;
    if !abs.starts_with(&home) {
        return Err(anyhow!("project outside $HOME: {}", abs.display()));
    }
    let project_str = abs.to_string_lossy().to_string();
    let files = walk_text_files(&abs, MAX_FILES_PER_INDEX);
    let mut count = 0usize;
    for path in files.iter() {
        let Ok(text) = std::fs::read_to_string(path) else {
            continue;
        };
        if text.len() > MAX_FILE_BYTES {
            continue;
        }
        let chunks = chunk_text(&text);
        for (i, chunk) in chunks.iter().enumerate() {
            let hash = sha_hex(chunk);
            let already = {
                let conn = db.lock();
                conn.query_row(
                    "SELECT 1 FROM search_embeddings WHERE project_path=? AND file_path=? AND chunk_id=? AND chunk_hash=?",
                    params![project_str, path.to_string_lossy(), i as i64, hash],
                    |_| Ok(()),
                ).is_ok()
            };
            if already {
                continue;
            }
            let emb = match embed_text(chunk).await {
                Ok(e) => e,
                Err(e) => {
                    tracing::warn!("embed failed for {}: {}", path.display(), e);
                    continue;
                }
            };
            let blob: Vec<u8> = emb.iter().flat_map(|f| f.to_le_bytes()).collect();
            let conn = db.lock();
            conn.execute(
                "INSERT INTO search_embeddings (project_path, file_path, chunk_id, chunk_text, chunk_hash, embedding) \
                 VALUES (?, ?, ?, ?, ?, ?) \
                 ON CONFLICT(project_path, file_path, chunk_id) DO UPDATE SET \
                    chunk_text = excluded.chunk_text, chunk_hash = excluded.chunk_hash, \
                    embedding = excluded.embedding, indexed_at = datetime('now')",
                params![project_str, path.to_string_lossy(), i as i64, chunk, hash, blob],
            )?;
            count += 1;
        }
    }
    Ok(count)
}

pub async fn search(
    db: Arc<Mutex<Connection>>,
    project: &Path,
    query: &str,
    top_k: usize,
) -> Result<Vec<SearchHit>> {
    if query.trim().is_empty() {
        return Err(anyhow!("empty query"));
    }
    let abs = project.canonicalize()?;
    let project_str = abs.to_string_lossy().to_string();
    let query_emb = embed_text(query).await?;
    let rows: Vec<(String, i64, String, Vec<u8>)> = {
        let conn = db.lock();
        let mut stmt = conn.prepare(
            "SELECT file_path, chunk_id, chunk_text, embedding FROM search_embeddings \
             WHERE project_path=? LIMIT 5000",
        )?;
        let v: Vec<(String, i64, String, Vec<u8>)> = stmt
            .query_map([&project_str], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
            })?
            .filter_map(|x| x.ok())
            .collect();
        v
    };
    let mut scored: Vec<SearchHit> = rows
        .into_iter()
        .map(|(fp, cid, text, blob)| {
            let emb = parse_emb_blob(&blob);
            let score = cosine(&query_emb, &emb);
            let snippet = if text.len() > 240 {
                format!("{}…", &text[..240])
            } else {
                text
            };
            SearchHit {
                file_path: fp,
                chunk_id: cid,
                snippet,
                score,
            }
        })
        .collect();
    scored.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    scored.truncate(top_k);
    Ok(scored)
}

fn parse_emb_blob(blob: &[u8]) -> Vec<f32> {
    blob.chunks(4)
        .filter_map(|c| c.try_into().ok().map(f32::from_le_bytes))
        .collect()
}

/// Similitud coseno entre dos vectores. `pub` para reuso (procedural_gotchas: similitud de
/// síntomas por TF-vector, sin red). 0.0 si vacíos / dim distinta / norma cero.
pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
    if a.is_empty() || b.is_empty() || a.len() != b.len() {
        return 0.0;
    }
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let na: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let nb: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if na == 0.0 || nb == 0.0 {
        0.0
    } else {
        dot / (na * nb)
    }
}

fn chunk_text(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut buf = String::new();
    for line in text.lines() {
        buf.push_str(line);
        buf.push('\n');
        if buf.len() >= CHUNK_SIZE {
            out.push(std::mem::take(&mut buf));
        }
    }
    if !buf.is_empty() {
        out.push(buf);
    }
    out
}

fn sha_hex(s: &str) -> String {
    let mut h = Sha256::new();
    h.update(s.as_bytes());
    hex::encode(h.finalize())
}

fn walk_text_files(root: &Path, cap: usize) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    let mut queue: std::collections::VecDeque<(std::path::PathBuf, usize)> =
        std::collections::VecDeque::new();
    queue.push_back((root.to_path_buf(), 0));
    let excluded = [
        "node_modules",
        ".venv",
        "target",
        "dist",
        "build",
        ".git",
        "__pycache__",
        ".next",
        ".cache",
    ];
    while let Some((dir, depth)) = queue.pop_front() {
        if out.len() >= cap || depth > 5 {
            break;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let p = entry.path();
            let Some(name) = p.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if name.starts_with('.') && name != ".specify" {
                continue;
            }
            if excluded.contains(&name) {
                continue;
            }
            if p.is_dir() {
                queue.push_back((p, depth + 1));
                continue;
            }
            // Only text extensions.
            let ext = p.extension().and_then(|x| x.to_str()).unwrap_or("");
            if matches!(
                ext,
                "rs" | "ts"
                    | "tsx"
                    | "js"
                    | "jsx"
                    | "py"
                    | "md"
                    | "css"
                    | "html"
                    | "json"
                    | "yml"
                    | "yaml"
                    | "toml"
                    | "sh"
                    | "sql"
            ) {
                out.push(p);
                if out.len() >= cap {
                    return out;
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cosine_identical() {
        let v = vec![1.0, 0.0, 0.5];
        assert!((cosine(&v, &v) - 1.0).abs() < 1e-5);
    }

    #[test]
    fn cosine_orthogonal() {
        assert_eq!(cosine(&[1.0, 0.0], &[0.0, 1.0]), 0.0);
    }

    #[test]
    fn chunk_text_short() {
        let chunks = chunk_text("a\nb\nc\n");
        assert_eq!(chunks.len(), 1);
    }

    #[test]
    fn parse_blob_roundtrip() {
        let v = [1.5f32, -2.0, 0.25];
        let blob: Vec<u8> = v.iter().flat_map(|f| f.to_le_bytes()).collect();
        let parsed = parse_emb_blob(&blob);
        assert_eq!(parsed.len(), 3);
        assert!((parsed[0] - 1.5).abs() < 1e-5);
    }
}
