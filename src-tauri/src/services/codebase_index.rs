// services/codebase_index.rs — spec-011 · US2 / FR-004.
//
// Background indexing for the codebase-memory plugin. Triggered by project.opened /
// pty_spawn (commands.rs) via the bg_queue ("codebase_index" kind), so a large repo
// indexes WITHOUT blocking the UI (SC-004). The work:
//   1. re-load + re-verify the SIGNED plugin manifest (fail-closed: bad sig → abort);
//   2. confirm it declares an `mcp.index_command`;
//   3. resolve placeholders ($PROJECT_ROOT/$PROJECT_KEY/$FURX_DATA) → concrete argv+env;
//   4. ensure the per-project store dir exists ($FURX_DATA/codebase-memory/<slug>);
//   5. run the pinned indexer binary as a subprocess with a clean env (only PATH +
//      the resolved env), cwd = project_root, with a generous timeout.
//
// Security: the command is the manifest's run.sh (hash-bound at install), resolved
// inside the installed plugin dir — never an arbitrary command from caller args.

use anyhow::{anyhow, Result};
use parking_lot::Mutex;
use rusqlite::Connection;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;
use tokio::process::Command;

use super::mcp_inject;

/// Default indexing timeout. Large repos can take a while; FR-004 says background +
/// non-blocking, so a long ceiling is fine (the bg_queue worker is off the UI thread).
const INDEX_TIMEOUT_MS: u64 = 30 * 60 * 1000; // 30 min

/// Resolve the installed plugins base (~/.furx/plugins) — where verified plugins live.
fn installed_plugins_base() -> Result<std::path::PathBuf> {
    let home = dirs::home_dir().ok_or_else(|| anyhow!("no home"))?;
    Ok(home.join(".furx").join("plugins"))
}

fn furx_data() -> Result<String> {
    let home = dirs::home_dir().ok_or_else(|| anyhow!("no home"))?;
    Ok(home.join(".furx").to_string_lossy().into_owned())
}

/// Run one indexing pass for `plugin` over `project_root` (scoped by `project_key`).
/// Returns a short status string for the bg_job output. Fail-closed on a bad/absent
/// manifest, missing index_command, or a non-zero indexer exit.
pub async fn run_index(
    db: &Arc<Mutex<Connection>>,
    plugin: &str,
    project_root: &str,
    project_key: &str,
) -> Result<String> {
    if project_root.is_empty() {
        return Err(anyhow!("codebase_index: empty project_root"));
    }
    // spec-022 US1 — el estado DISABLED global gana: un plugin desactivado por el usuario
    // NO se ejecuta como indexer aunque un job viejo lo tenga encolado. Defense-in-depth
    // (la ruta de inyección ya lo filtra antes de encolar, pero un job stale o un reinicio
    // podría disparar uno encolado de cuando estaba enabled).
    if !super::plugins::is_enabled(db, plugin).map_err(|e| anyhow!("is_enabled: {e}"))? {
        return Err(anyhow!(
            "codebase_index: plugin '{}' is disabled — skipping index",
            plugin
        ));
    }
    let base = installed_plugins_base()?;
    let m = mcp_inject::load_verified_manifest(&base, plugin)
        .ok_or_else(|| anyhow!("codebase_index: plugin '{}' not installed/verified", plugin))?;
    let spec = m
        .mcp
        .as_ref()
        .ok_or_else(|| anyhow!("plugin '{}' is not an MCP server", plugin))?;
    let idx = spec.index_command.as_ref().ok_or_else(|| {
        anyhow!(
            "plugin '{}' declares no index_command (no auto-index)",
            plugin
        )
    })?;

    // FR-002 default-deny: re-assert the indexer's declared permissions before exec.
    mcp_inject::assert_codebase_permissions(&m.permissions)?;

    let data = furx_data()?;
    // Resolve the per-project store (the declared fs_write target) and create it.
    let store = mcp_inject::ensure_project_store(project_key)?;

    // Bind the indexer command to the SIGNED, HASH-VERIFIED entrypoint inside the
    // installed plugin dir (same content binding as the MCP launch / Plugin Host exec).
    // A relative `index_command.command` MUST be the signed entrypoint; anything else, a
    // missing entrypoint_sha256, or an on-disk hash mismatch → refuse (fail-closed).
    if idx.command.trim() != m.entrypoint {
        return Err(anyhow!(
            "plugin '{}' index_command.command ({:?}) must be the signed entrypoint ({:?})",
            plugin,
            idx.command,
            m.entrypoint
        ));
    }
    let prog = mcp_inject::verified_entrypoint_path(&base, &m)?
        .to_string_lossy()
        .into_owned();
    let argv: Vec<String> = idx
        .args
        .iter()
        .map(|a| expand(a, project_root, project_key, &data))
        .collect();

    let mut command = Command::new(&prog);
    command
        .args(&argv)
        .current_dir(project_root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .env(
            "HOME",
            dirs::home_dir()
                .map(|h| h.to_string_lossy().into_owned())
                .unwrap_or_default(),
        )
        .env("FURX_PLUGIN", "1");
    for (k, v) in &idx.env {
        command.env(k, expand(v, project_root, project_key, &data));
    }

    let out = tokio::time::timeout(
        Duration::from_millis(INDEX_TIMEOUT_MS),
        command.spawn()?.wait_with_output(),
    )
    .await
    .map_err(|_| anyhow!("codebase_index timed out after {}ms", INDEX_TIMEOUT_MS))?
    .map_err(|e| anyhow!("codebase_index wait: {e}"))?;

    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(anyhow!("indexer exited non-zero: {}", stderr.trim()));
    }
    Ok(format!("indexed {} → {}", project_root, store.display()))
}

/// Same placeholder semantics as `mcp_inject` (kept in sync via the shared
/// `project_key_slug`): $PROJECT_ROOT, $FURX_DATA, and the per-project store combo.
fn expand(s: &str, root: &str, key: &str, data: &str) -> String {
    s.replace("$PROJECT_ROOT", root)
        .replace("$FURX_DATA", data)
        .replace(
            "codebase-memory/$PROJECT_KEY",
            &format!("codebase-memory/{}", mcp_inject::project_key_slug(key)),
        )
        .replace("$PROJECT_KEY", key)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// In-memory DB with the migration that creates the `plugins` table (010).
    fn test_db() -> Arc<Mutex<Connection>> {
        let conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", "ON").unwrap();
        conn.execute_batch(include_str!("../../migrations/010_b5.sql"))
            .unwrap();
        conn.execute_batch(include_str!("../../migrations/039_plugins_unique_name.sql"))
            .unwrap();
        Arc::new(Mutex::new(conn))
    }

    #[test]
    fn expand_matches_mcp_inject() {
        let got = expand(
            "$FURX_DATA/codebase-memory/$PROJECT_KEY",
            "/r",
            "/a/b",
            "/h/.furx",
        );
        assert!(got.starts_with("/h/.furx/codebase-memory/"));
        assert!(!got.contains("$PROJECT_KEY"));
        assert_eq!(expand("$PROJECT_ROOT", "/repo", "/repo", "/d"), "/repo");
    }

    #[tokio::test]
    async fn empty_root_rejected() {
        let db = test_db();
        assert!(run_index(&db, "codebase-memory", "", "").await.is_err());
    }

    #[tokio::test]
    async fn missing_plugin_rejected() {
        let db = test_db();
        // A plugin name that isn't installed → fail-closed. (Default-enabled in DB since
        // there's no row, so it passes the enable gate and fails at the manifest gate.)
        let r = run_index(&db, "definitely-not-installed-xyz", "/tmp", "/tmp").await;
        assert!(r.is_err());
        assert!(r
            .unwrap_err()
            .to_string()
            .contains("not installed/verified"));
    }

    #[tokio::test]
    async fn disabled_plugin_skips_indexing() {
        // spec-022 US1 — un plugin DISABLED no se indexa aunque el job esté encolado.
        let db = test_db();
        crate::services::plugins::set_enabled(&db, "codebase-memory", false).unwrap();
        let r = run_index(&db, "codebase-memory", "/tmp", "/tmp").await;
        assert!(r.is_err());
        assert!(
            r.unwrap_err().to_string().contains("is disabled"),
            "el plugin disabled debe rechazarse en el gate de enable (antes del exec)"
        );
    }
}
