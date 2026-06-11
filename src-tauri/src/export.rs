// F43 — .furxexport (tar.zst con SHA-256 + secrets filtered via F32 Guardrail).

use crate::bases::guardrail;
use anyhow::{anyhow, Result};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Read;
use std::path::Path;

#[derive(Debug, Serialize)]
pub struct ExportReport {
    pub path: String,
    pub size_bytes: u64,
    pub sha256: String,
    pub items: Vec<String>,
    pub filtered: Vec<String>, // settings keys filtered out por guardrail
}

/// Empaqueta ~/.furx/furx.db (sin WAL/SHM, hace VACUUM INTO un temp primero) en tar.zst.
/// Filtra valores de settings que tengan secretos detectables (via guardrail).
pub fn export_state(out_path: &Path) -> Result<ExportReport> {
    let home = dirs::home_dir().ok_or_else(|| anyhow!("no home"))?;
    let furx_dir = home.join(".furx");
    let db_path = furx_dir.join("furx.db");
    if !db_path.exists() {
        return Err(anyhow!("furx.db not found at {}", db_path.display()));
    }

    // VACUUM INTO temp dump — atomic snapshot sin tocar WAL/SHM.
    let tmp_db =
        std::env::temp_dir().join(format!("furx-export-{}.db", chrono::Utc::now().timestamp()));
    {
        let conn = rusqlite::Connection::open(&db_path)?;
        conn.execute("VACUUM INTO ?", rusqlite::params![tmp_db.to_string_lossy()])?;
    }

    // Scan settings rows en el dump para detectar secretos a filtrar.
    let mut filtered = Vec::new();
    {
        let conn = rusqlite::Connection::open(&tmp_db)?;
        let mut stmt = conn.prepare("SELECT key, value FROM settings")?;
        let rows: Vec<(String, String)> = stmt
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
            .collect::<Result<Vec<_>, _>>()?;
        drop(stmt);
        for (k, v) in &rows {
            if !guardrail::scan(v).is_empty() {
                conn.execute(
                    "UPDATE settings SET value = '\"<redacted by guardrail>\"' WHERE key = ?",
                    rusqlite::params![k],
                )?;
                filtered.push(k.clone());
            }
        }
    }

    // Tar the temp db + a manifest.
    let mut items = Vec::new();
    let manifest = serde_json::json!({
        "version": env!("CARGO_PKG_VERSION"),
        "exported_at": chrono::Utc::now().to_rfc3339(),
        "filtered_settings_keys": filtered,
    });

    // Build tar in memory, then zstd-compress, then write to disk.
    let mut tar_buf: Vec<u8> = Vec::new();
    {
        let mut tar = tar::Builder::new(&mut tar_buf);

        let db_bytes = fs::read(&tmp_db)?;
        let mut hdr = tar::Header::new_gnu();
        hdr.set_size(db_bytes.len() as u64);
        hdr.set_mode(0o600);
        hdr.set_cksum();
        tar.append_data(&mut hdr, "furx.db", &db_bytes[..])?;
        items.push("furx.db".into());

        let mfst = serde_json::to_vec_pretty(&manifest)?;
        let mut hdr2 = tar::Header::new_gnu();
        hdr2.set_size(mfst.len() as u64);
        hdr2.set_mode(0o600);
        hdr2.set_cksum();
        tar.append_data(&mut hdr2, "manifest.json", &mfst[..])?;
        items.push("manifest.json".into());

        tar.finish()?;
    }

    let compressed = zstd::stream::encode_all(&tar_buf[..], 19)?;

    fs::write(out_path, &compressed)?;
    let _ = fs::remove_file(&tmp_db);

    let mut hasher = Sha256::new();
    let mut f = fs::File::open(out_path)?;
    let mut buf = [0u8; 8192];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    let sha = hex(&hasher.finalize());

    Ok(ExportReport {
        path: out_path.display().to_string(),
        size_bytes: compressed.len() as u64,
        sha256: sha,
        items,
        filtered,
    })
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}
