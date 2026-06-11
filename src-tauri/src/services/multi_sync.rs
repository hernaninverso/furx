// spec-050 · Ola 8 P2 (FR-001) — Multi-machine sync.
//
// OPT-IN + FAIL-CLOSED. Sincroniza, entre las máquinas del MISMO usuario (vía cloud relay), tres
// clases de preferencias de usuario:
//   - overrides MCP   (`mcp_user_overrides`)
//   - targets de monitor (`monitor_targets`)
//   - gotchas/lecciones procedurales (`memory_entries kind='procedural'`)
//
// Tiebreaker LAST-WRITE-WINS `(updated_at, installation_id)` — NO CRDT (decisión del spec/council):
// para preferencias de usuario, "última escritura gana" con desempate determinista por
// installation_id alcanza. El merge es una función PURA y testeable (`merge_payloads`).
//
// GATING (cero regresión):
//   - Setting `sync.multi_machine_enabled` (default OFF). Con OFF, `sync_now` es un no-op explícito.
//   - FAIL-CLOSED: si el relay no está disponible / falla, cada máquina sigue con su estado LOCAL
//     (no se borra ni pisa nada). El error se reporta pero el estado local queda intacto.
//
// PRIVACIDAD: el payload de sync lleva SÓLO preferencias del usuario (nombres de MCP, addrs de
// monitor, texto de lecciones ya aprobadas) — NUNCA secrets/keys (BYOK F-I). El relay del cloud
// client ya sanitiza; acá no metemos prompts/diffs.

use parking_lot::Mutex;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Setting opt-in que habilita la sincronización multi-máquina. Default OFF.
pub const ENABLED_SETTING: &str = "sync.multi_machine_enabled";

/// Clases de item sincronizable. Strings estables (viajan en el payload y en `sync_meta.kind`).
pub const KIND_MCP_OVERRIDE: &str = "mcp_override";
pub const KIND_MONITOR_TARGET: &str = "monitor_target";
pub const KIND_LESSON: &str = "lesson";

/// `true` si la sync multi-máquina está habilitada (lee el setting; default OFF).
pub fn is_enabled(conn: &Connection) -> bool {
    match crate::settings::get(conn, ENABLED_SETTING).ok().flatten() {
        Some(v) => v
            .as_bool()
            .unwrap_or_else(|| matches!(v.as_str(), Some("1") | Some("true"))),
        None => false,
    }
}

/// Un item sincronizable con su metadata de last-write. `data` es el contenido serializado (JSON
/// específico por kind); `deleted` = tombstone. El merge usa `(updated_at, installation_id)`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SyncItem {
    pub kind: String,
    pub item_id: String,
    pub updated_at: String,
    pub installation_id: String,
    #[serde(default)]
    pub deleted: bool,
    /// Contenido del item (forma libre por kind). Para un tombstone puede ser `null`.
    #[serde(default)]
    pub data: serde_json::Value,
}

/// Payload de sync (lista de items + versión de shape para feature-detection futura).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SyncPayload {
    pub version: u32,
    pub items: Vec<SyncItem>,
}

/// Versión del shape del payload (bump si cambia la forma, no el contenido).
pub const SYNC_PAYLOAD_VERSION: u32 = 1;

/// ¿`a` gana sobre `b` por el tiebreaker LWW? Compara `(updated_at, installation_id)`
/// lexicográficamente: gana el `updated_at` mayor; si empatan, el `installation_id` mayor. Determinista
/// y simétrico (`wins(a,b) == !wins(b,a)` salvo empate TOTAL, donde devuelve false → se queda el actual).
fn wins(a: &SyncItem, b: &SyncItem) -> bool {
    (a.updated_at.as_str(), a.installation_id.as_str())
        > (b.updated_at.as_str(), b.installation_id.as_str())
}

/// MERGE PURO de dos payloads (local + remoto) con tiebreaker LWW `(updated_at, installation_id)`.
/// Devuelve el payload mergeado: para cada `(kind, item_id)`, gana el item con `(updated_at,
/// installation_id)` mayor. Un tombstone (`deleted=true`) participa del merge como cualquier item —
/// si gana, el merge lo marca borrado (el apply lo elimina localmente). Determinista, idempotente:
/// `merge(merge(a,b), b) == merge(a,b)`.
pub fn merge_payloads(local: &SyncPayload, remote: &SyncPayload) -> SyncPayload {
    // BTreeMap por (kind, item_id) → orden determinista de salida (importante para tests/idempotencia).
    let mut best: BTreeMap<(String, String), SyncItem> = BTreeMap::new();
    for item in local.items.iter().chain(remote.items.iter()) {
        let key = (item.kind.clone(), item.item_id.clone());
        match best.get(&key) {
            Some(cur) if !wins(item, cur) => { /* el actual gana o empatan → conservar */ }
            _ => {
                best.insert(key, item.clone());
            }
        }
    }
    SyncPayload {
        version: SYNC_PAYLOAD_VERSION,
        items: best.into_values().collect(),
    }
}

// ── Build del payload LOCAL desde la DB ──────────────────────────────────────────────────────────

/// Construye el payload LOCAL desde la DB: overrides MCP + targets de monitor + lecciones procedurales,
/// cada item con su `(updated_at, installation_id)` de `sync_meta` (o el `updated_at`/`created_at` de la
/// tabla de origen + el `installation_id` local como fallback si nunca pasó por sync). Tombstones salen
/// de filas `sync_meta.deleted=1`.
pub fn build_local_payload(db: &Mutex<Connection>) -> SyncPayload {
    let conn = db.lock();
    let local_iid = crate::services::identity::installation_id();
    let mut items: Vec<SyncItem> = Vec::new();

    // Helper: lee la metadata de sync de un item (si existe) → (updated_at, installation_id, deleted).
    let meta_of = |conn: &Connection, kind: &str, id: &str| -> Option<(String, String, bool)> {
        conn.query_row(
            "SELECT updated_at, installation_id, deleted FROM sync_meta WHERE kind = ? AND item_id = ?",
            params![kind, id],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)? != 0)),
        )
        .ok()
    };

    // 1) MCP overrides.
    if let Ok(mut stmt) =
        conn.prepare("SELECT name, enabled, source, updated_at FROM mcp_user_overrides")
    {
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
            ))
        });
        if let Ok(it) = rows {
            for (name, enabled, source, updated_at) in it.flatten() {
                let (ua, iid, deleted) = meta_of(&conn, KIND_MCP_OVERRIDE, &name)
                    .unwrap_or((updated_at, local_iid.clone(), false));
                items.push(SyncItem {
                    kind: KIND_MCP_OVERRIDE.into(),
                    item_id: name,
                    updated_at: ua,
                    installation_id: iid,
                    deleted,
                    data: serde_json::json!({"enabled": enabled != 0, "source": source}),
                });
            }
        }
    }

    // 2) Monitor targets.
    if let Ok(mut stmt) = conn.prepare(
        "SELECT id, label, kind, addr, interval_s, tier_min, enabled, created_at FROM monitor_targets",
    ) {
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, i64>(4)?,
                r.get::<_, String>(5)?,
                r.get::<_, i64>(6)?,
                r.get::<_, String>(7)?,
            ))
        });
        if let Ok(it) = rows {
            for (id, label, mkind, addr, interval_s, tier_min, enabled, created_at) in it.flatten() {
                // IDENTIDAD LÓGICA de sync = (kind, addr), NO el UUID `id` (audit deepseek HIGH F1): el
                // `id` es un UUID generado POR MÁQUINA, así que el MISMO monitor (kind,addr) tiene ids
                // distintos en A y B → merge-por-id los trataría como 2 items y el INSERT del 2do
                // chocaría con UNIQUE(kind,addr). Usar `kind|addr` como item_id hace que el mismo
                // monitor converja entre máquinas. El UUID local viaja en `data.local_id` (informativo).
                let sync_id = format!("{mkind}|{addr}");
                let (ua, iid, deleted) = meta_of(&conn, KIND_MONITOR_TARGET, &sync_id)
                    .unwrap_or((created_at, local_iid.clone(), false));
                items.push(SyncItem {
                    kind: KIND_MONITOR_TARGET.into(),
                    item_id: sync_id,
                    updated_at: ua,
                    installation_id: iid,
                    deleted,
                    data: serde_json::json!({
                        "label": label, "kind": mkind, "addr": addr,
                        "interval_s": interval_s, "tier_min": tier_min, "enabled": enabled != 0,
                        "local_id": id
                    }),
                });
            }
        }
    }

    // 3) Lecciones procedurales aprobadas (gotchas).
    if let Ok(mut stmt) = conn.prepare(
        "SELECT id, content, project_key, COALESCE(updated_at, created_at, '') \
         FROM memory_entries WHERE kind = 'procedural'",
    ) {
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
            ))
        });
        if let Ok(it) = rows {
            for (id, content, project_key, updated_at) in it.flatten() {
                let (ua, iid, deleted) = meta_of(&conn, KIND_LESSON, &id)
                    .unwrap_or((updated_at, local_iid.clone(), false));
                items.push(SyncItem {
                    kind: KIND_LESSON.into(),
                    item_id: id,
                    updated_at: ua,
                    installation_id: iid,
                    deleted,
                    data: serde_json::json!({"content": content, "project_key": project_key}),
                });
            }
        }
    }

    // Tombstones puros: filas en sync_meta marcadas deleted que YA no están en su tabla de origen
    // (para que el borrado se propague). Las traemos sin data.
    if let Ok(mut stmt) =
        conn.prepare("SELECT kind, item_id, updated_at, installation_id FROM sync_meta WHERE deleted = 1")
    {
        let existing: std::collections::HashSet<(String, String)> =
            items.iter().map(|i| (i.kind.clone(), i.item_id.clone())).collect();
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
            ))
        });
        if let Ok(it) = rows {
            for (kind, item_id, updated_at, installation_id) in it.flatten() {
                if existing.contains(&(kind.clone(), item_id.clone())) {
                    continue; // ya emitido arriba (con su deleted flag).
                }
                items.push(SyncItem {
                    kind,
                    item_id,
                    updated_at,
                    installation_id,
                    deleted: true,
                    data: serde_json::Value::Null,
                });
            }
        }
    }

    SyncPayload {
        version: SYNC_PAYLOAD_VERSION,
        items,
    }
}

// ── Apply del payload MERGEADO a la DB local ─────────────────────────────────────────────────────

/// Resultado de aplicar un merge: cuántos items se escribieron / borraron por kind (para el status UI).
#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct ApplyReport {
    pub upserted: usize,
    pub deleted: usize,
}

/// Aplica el payload MERGEADO a la DB local: por cada item ganador, upsert (o delete si tombstone) en
/// la tabla de origen + actualiza `sync_meta`. Sólo escribe los items cuyo `(updated_at,
/// installation_id)` es ESTRICTAMENTE mayor al `sync_meta` local (no re-pisa lo que ya es nuestro o
/// más nuevo → idempotente, y un re-apply no genera churn). Best-effort por item: un fallo en uno no
/// aborta el resto. Devuelve el conteo.
pub fn apply_merged(db: &Mutex<Connection>, merged: &SyncPayload) -> ApplyReport {
    let conn = db.lock();
    let mut report = ApplyReport::default();
    // Atomicidad (audit deepseek MED F3): todo el apply va en UNA transacción. Si algo revienta el
    // commit, el estado local queda intacto (fail-closed). Si no se puede abrir la txn, no aplicamos.
    let tx = match conn.unchecked_transaction() {
        Ok(t) => t,
        Err(e) => {
            tracing::warn!("multi_sync: no se pudo abrir txn de apply: {e}");
            return report;
        }
    };
    for item in &merged.items {
        // Validación (audit deepseek MED F2): un item con installation_id o item_id vacío es inválido
        // (no puede haber sido escrito por una máquina real) → lo saltamos (fail-closed).
        if item.installation_id.trim().is_empty() || item.item_id.trim().is_empty() {
            continue;
        }
        // ¿el item entrante gana al sync_meta local? Si el local es >= no escribimos (idempotencia).
        let local_meta: Option<(String, String)> = tx
            .query_row(
                "SELECT updated_at, installation_id FROM sync_meta WHERE kind = ? AND item_id = ?",
                params![item.kind, item.item_id],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
            )
            .ok();
        if let Some((lua, liid)) = &local_meta {
            let local_item = SyncItem {
                kind: item.kind.clone(),
                item_id: item.item_id.clone(),
                updated_at: lua.clone(),
                installation_id: liid.clone(),
                deleted: false,
                data: serde_json::Value::Null,
            };
            if !wins(item, &local_item) {
                continue; // el local es >= → no re-pisar.
            }
        }
        let ok = if item.deleted {
            apply_delete(&tx, item)
        } else {
            apply_upsert(&tx, item)
        };
        if ok {
            if item.deleted {
                report.deleted += 1;
            } else {
                report.upserted += 1;
            }
            // Actualizar sync_meta con el ganador.
            let _ = tx.execute(
                "INSERT INTO sync_meta (kind, item_id, updated_at, installation_id, deleted) \
                 VALUES (?1, ?2, ?3, ?4, ?5) \
                 ON CONFLICT(kind, item_id) DO UPDATE SET \
                   updated_at = excluded.updated_at, installation_id = excluded.installation_id, \
                   deleted = excluded.deleted",
                params![
                    item.kind,
                    item.item_id,
                    item.updated_at,
                    item.installation_id,
                    item.deleted as i64
                ],
            );
        }
    }
    // Commit atómico: si falla, ROLLBACK total → estado local intacto, report se descarta (vacío).
    if let Err(e) = tx.commit() {
        tracing::warn!("multi_sync: commit del apply falló (rollback): {e}");
        return ApplyReport::default();
    }
    report
}

fn apply_upsert(conn: &Connection, item: &SyncItem) -> bool {
    match item.kind.as_str() {
        KIND_MCP_OVERRIDE => {
            let enabled = item.data.get("enabled").and_then(|v| v.as_bool()).unwrap_or(true);
            let source = item
                .data
                .get("source")
                .and_then(|v| v.as_str())
                .filter(|s| *s == "user" || *s == "discovery")
                .unwrap_or("user");
            conn.execute(
                "INSERT INTO mcp_user_overrides (name, enabled, source, updated_at) \
                 VALUES (?1, ?2, ?3, ?4) \
                 ON CONFLICT(name) DO UPDATE SET enabled = excluded.enabled, source = excluded.source, \
                   updated_at = excluded.updated_at",
                params![item.item_id, enabled as i64, source, item.updated_at],
            )
            .is_ok()
        }
        KIND_MONITOR_TARGET => {
            let g = |k: &str| item.data.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
            let label = g("label");
            let mkind = g("kind");
            let addr = g("addr");
            let tier_min = {
                let t = g("tier_min");
                if matches!(t.as_str(), "free" | "pro" | "team" | "enterprise") { t } else { "free".into() }
            };
            // valida kind ∈ {tcp,http} (CHECK de la tabla); si no, no escribimos (fail-closed).
            if !matches!(mkind.as_str(), "tcp" | "http") || addr.is_empty() {
                return false;
            }
            let interval_s = item
                .data
                .get("interval_s")
                .and_then(|v| v.as_i64())
                .filter(|n| *n >= 5)
                .unwrap_or(30);
            let enabled = item.data.get("enabled").and_then(|v| v.as_bool()).unwrap_or(true);
            // Upsert por la IDENTIDAD LÓGICA (kind, addr), NO por el UUID (audit deepseek HIGH F1). Si
            // la fila NO existe se genera un id local nuevo; si existe (mismo kind,addr) se actualiza —
            // sin chocar el UNIQUE(kind,addr). El UUID `id` es local-por-máquina y no se sincroniza.
            let new_id = uuid::Uuid::new_v4().to_string();
            conn.execute(
                "INSERT INTO monitor_targets (id, label, kind, addr, interval_s, tier_min, enabled) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7) \
                 ON CONFLICT(kind, addr) DO UPDATE SET label = excluded.label, \
                   interval_s = excluded.interval_s, tier_min = excluded.tier_min, \
                   enabled = excluded.enabled",
                params![new_id, label, mkind, addr, interval_s, tier_min, enabled as i64],
            )
            .is_ok()
        }
        KIND_LESSON => {
            let content = item.data.get("content").and_then(|v| v.as_str()).unwrap_or("");
            let project_key = item
                .data
                .get("project_key")
                .and_then(|v| v.as_str())
                .unwrap_or("__global__");
            if content.is_empty() {
                return false;
            }
            conn.execute(
                "INSERT INTO memory_entries (id, content, project_key, kind, created_at, updated_at) \
                 VALUES (?1, ?2, ?3, 'procedural', ?4, ?4) \
                 ON CONFLICT(id) DO UPDATE SET content = excluded.content, \
                   project_key = excluded.project_key, updated_at = excluded.updated_at",
                params![item.item_id, content, project_key, item.updated_at],
            )
            .is_ok()
        }
        _ => false, // kind desconocido → no escribimos (fail-closed).
    }
}

fn apply_delete(conn: &Connection, item: &SyncItem) -> bool {
    match item.kind.as_str() {
        KIND_MCP_OVERRIDE => conn
            .execute("DELETE FROM mcp_user_overrides WHERE name = ?", params![item.item_id])
            .is_ok(),
        KIND_MONITOR_TARGET => {
            // El item_id de monitor es la identidad lógica `kind|addr` (NO el UUID). Borramos por
            // (kind, addr). `split_once` parte en el PRIMER `|`: como kind ∈ {tcp,http} NUNCA contiene
            // `|`, ese primer `|` SIEMPRE separa el kind de la addr completa — aunque la addr tuviera
            // `|`, el resto tras el 1er split queda íntegro en `addr` (roundtrip exacto con el
            // `format!("{kind}|{addr}")` del build). Por eso `split_once` (NO `rsplit_once`) es correcto.
            match item.item_id.split_once('|') {
                Some((mkind, addr)) => conn
                    .execute(
                        "DELETE FROM monitor_targets WHERE kind = ? AND addr = ?",
                        params![mkind, addr],
                    )
                    .is_ok(),
                None => false,
            }
        }
        KIND_LESSON => conn
            .execute("DELETE FROM memory_entries WHERE id = ?", params![item.item_id])
            .is_ok(),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(kind: &str, id: &str, ua: &str, iid: &str, enabled: bool) -> SyncItem {
        SyncItem {
            kind: kind.into(),
            item_id: id.into(),
            updated_at: ua.into(),
            installation_id: iid.into(),
            deleted: false,
            data: serde_json::json!({"enabled": enabled, "source": "user"}),
        }
    }

    #[test]
    fn newer_timestamp_wins() {
        let local = SyncPayload {
            version: 1,
            items: vec![item(KIND_MCP_OVERRIDE, "mnemo", "2026-06-01T00:00:00Z", "iidA", true)],
        };
        let remote = SyncPayload {
            version: 1,
            items: vec![item(KIND_MCP_OVERRIDE, "mnemo", "2026-06-02T00:00:00Z", "iidB", false)],
        };
        let m = merge_payloads(&local, &remote);
        assert_eq!(m.items.len(), 1);
        // gana el remoto (timestamp mayor): enabled=false.
        assert_eq!(m.items[0].data.get("enabled").and_then(|v| v.as_bool()), Some(false));
        assert_eq!(m.items[0].installation_id, "iidB");
    }

    #[test]
    fn installation_id_breaks_timestamp_tie() {
        // SC-001: mismo timestamp → desempata el installation_id mayor (determinista).
        let local = SyncPayload {
            version: 1,
            items: vec![item(KIND_MCP_OVERRIDE, "x", "2026-06-01T00:00:00Z", "iidA", true)],
        };
        let remote = SyncPayload {
            version: 1,
            items: vec![item(KIND_MCP_OVERRIDE, "x", "2026-06-01T00:00:00Z", "iidB", false)],
        };
        let m = merge_payloads(&local, &remote);
        assert_eq!(m.items.len(), 1);
        // iidB > iidA → gana remoto.
        assert_eq!(m.items[0].installation_id, "iidB");
        assert_eq!(m.items[0].data.get("enabled").and_then(|v| v.as_bool()), Some(false));
    }

    #[test]
    fn total_tie_keeps_one_deterministically() {
        // Empate TOTAL (mismo ua + mismo iid): se conserva UNO (no duplica). Determinista.
        let a = item(KIND_MCP_OVERRIDE, "x", "2026-06-01T00:00:00Z", "iid", true);
        let local = SyncPayload { version: 1, items: vec![a.clone()] };
        let remote = SyncPayload { version: 1, items: vec![a.clone()] };
        let m = merge_payloads(&local, &remote);
        assert_eq!(m.items.len(), 1);
        assert_eq!(m.items[0], a);
    }

    #[test]
    fn merge_is_idempotent() {
        let local = SyncPayload {
            version: 1,
            items: vec![
                item(KIND_MCP_OVERRIDE, "a", "2026-06-01T00:00:00Z", "iidA", true),
                item(KIND_MONITOR_TARGET, "b", "2026-06-01T00:00:00Z", "iidA", true),
            ],
        };
        let remote = SyncPayload {
            version: 1,
            items: vec![item(KIND_MCP_OVERRIDE, "a", "2026-06-03T00:00:00Z", "iidB", false)],
        };
        let m1 = merge_payloads(&local, &remote);
        let m2 = merge_payloads(&m1, &remote);
        assert_eq!(m1, m2, "merge(merge(a,b),b) == merge(a,b)");
    }

    #[test]
    fn tombstone_wins_when_newer() {
        let mut tomb = item(KIND_LESSON, "L1", "2026-06-05T00:00:00Z", "iidB", true);
        tomb.deleted = true;
        tomb.data = serde_json::Value::Null;
        let local = SyncPayload {
            version: 1,
            items: vec![item(KIND_LESSON, "L1", "2026-06-01T00:00:00Z", "iidA", true)],
        };
        let remote = SyncPayload { version: 1, items: vec![tomb] };
        let m = merge_payloads(&local, &remote);
        assert_eq!(m.items.len(), 1);
        assert!(m.items[0].deleted, "el tombstone más nuevo gana → item borrado");
    }

    // ── apply / build contra una DB en memoria ──────────────────────────────────────────────────
    fn mem_db() -> Mutex<Connection> {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(include_str!("../../migrations/001_init.sql")).unwrap();
        conn.execute_batch(include_str!("../../migrations/002_settings.sql")).unwrap();
        conn.execute_batch(include_str!("../../migrations/050_monitor_targets.sql")).unwrap();
        conn.execute_batch(include_str!("../../migrations/051_mcp_user_overrides.sql")).unwrap();
        // memory_entries (subset suficiente para lessons).
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS memory_entries (
                id TEXT PRIMARY KEY NOT NULL, source TEXT, source_id TEXT, content TEXT NOT NULL,
                tags TEXT, created_at TEXT, updated_at TEXT,
                project_key TEXT NOT NULL DEFAULT '__global__',
                rationale TEXT, kind TEXT NOT NULL DEFAULT 'episodic', cli_kind TEXT, session_id TEXT
            );",
        )
        .unwrap();
        conn.execute_batch(include_str!("../../migrations/057_sync_meta.sql")).unwrap();
        Mutex::new(conn)
    }

    #[test]
    fn apply_upserts_a_remote_override_and_is_idempotent() {
        let db = mem_db();
        let remote = SyncItem {
            kind: KIND_MCP_OVERRIDE.into(),
            item_id: "mnemo".into(),
            updated_at: "2026-06-10T00:00:00Z".into(),
            installation_id: "iidREMOTE".into(),
            deleted: false,
            data: serde_json::json!({"enabled": false, "source": "user"}),
        };
        let merged = SyncPayload { version: 1, items: vec![remote.clone()] };
        let r1 = apply_merged(&db, &merged);
        assert_eq!(r1.upserted, 1);
        {
            let conn = db.lock();
            let enabled: i64 = conn
                .query_row("SELECT enabled FROM mcp_user_overrides WHERE name='mnemo'", [], |r| r.get(0))
                .unwrap();
            assert_eq!(enabled, 0, "remote disabled aplicado");
        }
        // re-apply del MISMO merge → idempotente (no re-escribe: local == remoto).
        let r2 = apply_merged(&db, &merged);
        assert_eq!(r2.upserted, 0, "re-apply no re-escribe (idempotente)");
    }

    #[test]
    fn apply_does_not_overwrite_newer_local() {
        // SC-001 fail-safe: un item remoto VIEJO no pisa el local más nuevo.
        let db = mem_db();
        // estado local más nuevo via sync_meta + tabla.
        {
            let conn = db.lock();
            conn.execute(
                "INSERT INTO mcp_user_overrides (name, enabled, source, updated_at) VALUES ('x', 1, 'user', '2026-06-20T00:00:00Z')",
                [],
            ).unwrap();
            conn.execute(
                "INSERT INTO sync_meta (kind, item_id, updated_at, installation_id, deleted) VALUES (?,?,?,?,0)",
                params![KIND_MCP_OVERRIDE, "x", "2026-06-20T00:00:00Z", "iidLOCAL"],
            ).unwrap();
        }
        let old_remote = SyncItem {
            kind: KIND_MCP_OVERRIDE.into(),
            item_id: "x".into(),
            updated_at: "2026-06-01T00:00:00Z".into(),
            installation_id: "iidREMOTE".into(),
            deleted: false,
            data: serde_json::json!({"enabled": false, "source": "user"}),
        };
        let r = apply_merged(&db, &SyncPayload { version: 1, items: vec![old_remote] });
        assert_eq!(r.upserted, 0, "el remoto viejo NO pisa el local más nuevo");
        let conn = db.lock();
        let enabled: i64 = conn
            .query_row("SELECT enabled FROM mcp_user_overrides WHERE name='x'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(enabled, 1, "local intacto");
    }

    #[test]
    fn build_local_payload_includes_seeded_target() {
        // El seed de 050 mete 'mac-local' (tcp 127.0.0.1:22). build lo emite como monitor_target con
        // item_id = identidad lógica `kind|addr` (NO el UUID), y el UUID local viaja en data.local_id.
        let db = mem_db();
        let p = build_local_payload(&db);
        let t = p
            .items
            .iter()
            .find(|i| i.kind == KIND_MONITOR_TARGET && i.item_id == "tcp|127.0.0.1:22");
        assert!(t.is_some(), "monitor seed debe emitirse con item_id=kind|addr");
        assert_eq!(
            t.unwrap().data.get("local_id").and_then(|v| v.as_str()),
            Some("mac-local"),
            "el UUID local del seed viaja en data.local_id"
        );
    }

    // 050 FR-001 (audit deepseek HIGH F1) — un monitor con MISMO (kind,addr) pero UUID distinto NO
    // duplica ni choca el UNIQUE(kind,addr): el upsert es por identidad lógica.
    #[test]
    fn monitor_sync_converges_on_kind_addr_not_uuid() {
        let db = mem_db();
        // El seed mac-local ya está (tcp 127.0.0.1:22, uuid 'mac-local'). Un remoto trae el MISMO
        // (kind,addr) con otro uuid + más nuevo + label distinto → debe ACTUALIZAR la fila existente.
        let remote = SyncItem {
            kind: KIND_MONITOR_TARGET.into(),
            item_id: "tcp|127.0.0.1:22".into(),
            updated_at: "2026-06-30T00:00:00Z".into(),
            installation_id: "iidB".into(),
            deleted: false,
            data: serde_json::json!({
                "label": "Mac (renombrado en B)", "kind": "tcp", "addr": "127.0.0.1:22",
                "interval_s": 60, "tier_min": "free", "enabled": true, "local_id": "uuid-de-B"
            }),
        };
        let r = apply_merged(&db, &SyncPayload { version: 1, items: vec![remote] });
        assert_eq!(r.upserted, 1);
        let conn = db.lock();
        // sigue habiendo UNA sola fila para (tcp, 127.0.0.1:22), con el label nuevo.
        let (n, label): (i64, String) = conn
            .query_row(
                "SELECT COUNT(*), MAX(label) FROM monitor_targets WHERE kind='tcp' AND addr='127.0.0.1:22'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(n, 1, "NO se duplicó la fila (upsert por kind,addr)");
        assert_eq!(label, "Mac (renombrado en B)");
    }

    // 050 FR-001 — un tombstone de monitor borra la fila por (kind,addr) reconstruida del item_id.
    #[test]
    fn monitor_tombstone_deletes_by_kind_addr() {
        let db = mem_db();
        // El seed mac-local existe (tcp 127.0.0.1:22). Un tombstone más nuevo debe borrarlo.
        let tomb = SyncItem {
            kind: KIND_MONITOR_TARGET.into(),
            item_id: "tcp|127.0.0.1:22".into(),
            updated_at: "2026-07-01T00:00:00Z".into(),
            installation_id: "iidB".into(),
            deleted: true,
            data: serde_json::Value::Null,
        };
        let r = apply_merged(&db, &SyncPayload { version: 1, items: vec![tomb] });
        assert_eq!(r.deleted, 1);
        let conn = db.lock();
        let n: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM monitor_targets WHERE kind='tcp' AND addr='127.0.0.1:22'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 0, "el tombstone borró el monitor por (kind,addr)");
    }

    // 050 FR-001 (audit deepseek MED F2) — un item con installation_id vacío se saltea (fail-closed).
    #[test]
    fn apply_skips_empty_installation_id() {
        let db = mem_db();
        let bad = SyncItem {
            kind: KIND_MCP_OVERRIDE.into(),
            item_id: "mnemo".into(),
            updated_at: "2026-06-30T00:00:00Z".into(),
            installation_id: "".into(), // inválido
            deleted: false,
            data: serde_json::json!({"enabled": false, "source": "user"}),
        };
        let r = apply_merged(&db, &SyncPayload { version: 1, items: vec![bad] });
        assert_eq!(r.upserted, 0, "item con installation_id vacío no se aplica");
    }

    #[test]
    fn is_enabled_default_off() {
        let db = mem_db();
        let conn = db.lock();
        assert!(!is_enabled(&conn), "sync multi-máquina default OFF");
    }
}
