// Server monitors — configurable por el usuario (045 FR-001, Ola 5 P1).
//
// Antes (BLOQUE J): targets HARDCODEADOS (devserver Tailscale / hetzner WG / mac-local) en
// `default_targets()`, in-memory, sin UI. Ahora el SSOT es la tabla `monitor_targets` (migración
// 050): el daemon poller LEE de la DB; el usuario agrega/quita por UI (`monitor_add`/`monitor_remove`).
// El seed default es SOLO `mac-local` (localhost) — devserver/hetzner eran infra del autor y se sacaron;
// cualquiera los agrega como un target más. El poller corre con un semaphore (cap 10) para no spawnear
// 1000 tasks con 1000 targets.

use anyhow::Result;
use parking_lot::Mutex;
use rusqlite::{params, Connection};
use serde::Serialize;
use std::sync::Arc;
use std::time::Duration;

/// Cap de tasks de check concurrentes del poller (council: sin límite, 1000 targets = 1000 tasks).
pub const MAX_CONCURRENT_CHECKS: usize = 10;

#[derive(Debug, Clone, Serialize)]
pub struct MonitorTarget {
    pub id: String,
    pub label: String,
    pub kind: String, // "tcp" | "http"
    pub addr: String, // "192.0.2.10:22" | "https://x"
    /// Cada cuántos segundos chequear este target. >= 5.
    pub interval_s: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct MonitorResult {
    pub id: String,
    pub up: bool,
    pub latency_ms: Option<u64>,
    pub error: Option<String>,
    pub checked_at: String,
}

/// Lee los targets HABILITADOS de la DB (migración 050). El poller usa esto en vez del hardcode.
/// Si la query falla (DB ocupada / esquema viejo), devuelve `[]` — el poller hace un ciclo vacío y
/// reintenta al próximo tick (NUNCA paniquea ni cae al hardcode de la infra del autor).
pub fn load_targets(db: &Arc<Mutex<Connection>>) -> Vec<MonitorTarget> {
    let conn = db.lock();
    let mut stmt = match conn.prepare(
        "SELECT id, label, kind, addr, interval_s FROM monitor_targets WHERE enabled = 1 ORDER BY created_at",
    ) {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!("monitors: prepare load_targets failed: {e}");
            return vec![];
        }
    };
    let rows = stmt.query_map([], |r| {
        Ok(MonitorTarget {
            id: r.get(0)?,
            label: r.get(1)?,
            kind: r.get(2)?,
            addr: r.get(3)?,
            interval_s: r.get::<_, i64>(4)?.max(5) as u64,
        })
    });
    match rows {
        Ok(it) => it.filter_map(|r| r.ok()).collect(),
        Err(e) => {
            tracing::warn!("monitors: query_map load_targets failed: {e}");
            vec![]
        }
    }
}

/// Inserta un target nuevo, aplicando el cap por tier en la MISMA toma del lock (cierra el TOCTOU
/// que dos `monitor_add` concurrentes podrían explotar contando antes de insertar; audit deepseek
/// HIGH). `cap = None` ⇒ ilimitado (Team/Enterprise). Valida `kind`+`addr`+`label` antes de tocar
/// la DB. Devuelve el id generado.
pub fn insert_target_capped(
    db: &Arc<Mutex<Connection>>,
    label: &str,
    kind: &str,
    addr: &str,
    interval_s: u64,
    cap: Option<usize>,
    tier: &str,
) -> Result<String> {
    validate_target(kind, addr)?;
    let label = label.trim();
    if label.is_empty() {
        return Err(anyhow::anyhow!("label vacío"));
    }
    let interval_s = interval_s.max(5);
    let id = uuid::Uuid::new_v4().to_string();
    let conn = db.lock();
    // Cap + insert bajo el MISMO lock → atómico para el SSOT (la `Connection` es única y serializada).
    if let Some(max) = cap {
        let existing: i64 =
            conn.query_row("SELECT COUNT(*) FROM monitor_targets", [], |r| r.get(0))?;
        if existing as usize >= max {
            return Err(anyhow::anyhow!(
                "Tu plan ({tier}) permite hasta {max} monitores. Subí de plan para agregar más."
            ));
        }
    }
    conn.execute(
        "INSERT INTO monitor_targets (id, label, kind, addr, interval_s, tier_min, enabled)
         VALUES (?1, ?2, ?3, ?4, ?5, 'free', 1)",
        params![id, label, kind, addr, interval_s as i64],
    )
    .map_err(|e| {
        // UNIQUE(kind, addr) → mensaje claro en vez del SQLITE_CONSTRAINT crudo.
        if e.to_string().contains("UNIQUE") {
            anyhow::anyhow!("ya existe un monitor para {kind} {addr}")
        } else {
            anyhow::anyhow!("insert monitor falló: {e}")
        }
    })?;
    Ok(id)
}

/// Cuenta los targets existentes (lectura informativa para UI/tests). El cap real lo aplica
/// `insert_target_capped` bajo lock; esta lectura es sólo para mostrar "N/M" en la UI.
#[cfg_attr(not(test), allow(dead_code))]
pub fn count_targets(db: &Arc<Mutex<Connection>>) -> Result<usize> {
    let conn = db.lock();
    let n: i64 = conn.query_row("SELECT COUNT(*) FROM monitor_targets", [], |r| r.get(0))?;
    Ok(n as usize)
}

/// Borra un target por id. Devuelve true si borró una fila.
pub fn delete_target(db: &Arc<Mutex<Connection>>, id: &str) -> Result<bool> {
    let conn = db.lock();
    let n = conn.execute("DELETE FROM monitor_targets WHERE id = ?1", params![id])?;
    Ok(n > 0)
}

/// Valida `kind` ∈ {tcp,http} y que `addr` tenga forma plausible para ese kind. NO hace red
/// (sólo forma); la red la prueba el poller. Conservador: rechaza addr vacío / con espacios.
pub fn validate_target(kind: &str, addr: &str) -> Result<()> {
    let addr = addr.trim();
    if addr.is_empty() || addr.chars().any(|c| c.is_whitespace()) {
        return Err(anyhow::anyhow!("addr vacío o con espacios"));
    }
    match kind {
        "tcp" => {
            // host:port — exigimos un ':' y un puerto numérico 1..=65535.
            let (host, port) = addr
                .rsplit_once(':')
                .ok_or_else(|| anyhow::anyhow!("tcp addr debe ser host:port"))?;
            if host.is_empty() {
                return Err(anyhow::anyhow!("tcp addr sin host"));
            }
            let p: u32 = port
                .parse()
                .map_err(|_| anyhow::anyhow!("puerto no numérico: {port}"))?;
            if p == 0 || p > 65535 {
                return Err(anyhow::anyhow!("puerto fuera de rango: {p}"));
            }
            Ok(())
        }
        "http" => {
            let url = url::Url::parse(addr)
                .map_err(|e| anyhow::anyhow!("http addr no es URL válida: {e}"))?;
            if url.scheme() != "http" && url.scheme() != "https" {
                return Err(anyhow::anyhow!("http monitor requiere esquema http(s)"));
            }
            if url.host_str().is_none() {
                return Err(anyhow::anyhow!("http addr sin host"));
            }
            Ok(())
        }
        other => Err(anyhow::anyhow!("kind no soportado: {other}")),
    }
}

pub async fn check(target: &MonitorTarget) -> MonitorResult {
    let start = std::time::Instant::now();
    let now = chrono::Utc::now().to_rfc3339();
    let outcome: Result<()> = match target.kind.as_str() {
        "tcp" => check_tcp(&target.addr).await,
        "http" => check_http(&target.addr).await,
        _ => Err(anyhow::anyhow!("unsupported kind: {}", target.kind)),
    };
    let elapsed = start.elapsed().as_millis() as u64;
    match outcome {
        Ok(_) => MonitorResult {
            id: target.id.clone(),
            up: true,
            latency_ms: Some(elapsed),
            error: None,
            checked_at: now,
        },
        Err(e) => MonitorResult {
            id: target.id.clone(),
            up: false,
            latency_ms: None,
            error: Some(e.to_string()),
            checked_at: now,
        },
    }
}

async fn check_tcp(addr: &str) -> Result<()> {
    let connect = tokio::net::TcpStream::connect(addr);
    tokio::time::timeout(Duration::from_secs(3), connect).await??;
    Ok(())
}

async fn check_http(url: &str) -> Result<()> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .danger_accept_invalid_certs(false)
        .build()?;
    let resp = client.get(url).send().await?;
    if !resp.status().is_success() && resp.status().as_u16() < 500 {
        // 4xx still means the host is alive; only treat 5xx and connection errors as down.
        return Ok(());
    }
    if resp.status().as_u16() >= 500 {
        return Err(anyhow::anyhow!("status {}", resp.status()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mem_db() -> Arc<Mutex<Connection>> {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(include_str!("../migrations/050_monitor_targets.sql"))
            .unwrap();
        Arc::new(Mutex::new(conn))
    }

    #[test]
    fn validate_tcp_addr() {
        assert!(validate_target("tcp", "127.0.0.1:22").is_ok());
        assert!(validate_target("tcp", "host.example:8250").is_ok());
        assert!(validate_target("tcp", "127.0.0.1").is_err()); // sin puerto
        assert!(validate_target("tcp", "127.0.0.1:0").is_err()); // puerto 0
        assert!(validate_target("tcp", "127.0.0.1:99999").is_err()); // fuera de rango
        assert!(validate_target("tcp", ":22").is_err()); // sin host
        assert!(validate_target("tcp", "").is_err());
        assert!(validate_target("tcp", "1.2.3.4 :22").is_err()); // espacio
    }

    #[test]
    fn validate_http_addr() {
        assert!(validate_target("http", "http://localhost:8250/health").is_ok());
        assert!(validate_target("http", "https://example.com").is_ok());
        assert!(validate_target("http", "ftp://x").is_err());
        assert!(validate_target("http", "not a url").is_err());
    }

    #[test]
    fn rejects_unknown_kind() {
        assert!(validate_target("icmp", "1.2.3.4").is_err());
        assert!(validate_target("", "x").is_err());
    }

    #[test]
    fn seed_is_only_mac_local_no_infra() {
        // SC-001: el seed default NO incluye devserver (100.64.0.10) ni hetzner (198.51.100.1).
        let db = mem_db();
        let targets = load_targets(&db);
        assert_eq!(targets.len(), 1, "seed default = SOLO mac-local");
        assert_eq!(targets[0].id, "mac-local");
        assert_eq!(targets[0].addr, "127.0.0.1:22");
        // El ÚNICO target del seed es exactamente mac-local; ningún host de la infra del autor
        // (cualquier puerto) puede colarse.
        for t in &targets {
            let host = t.addr.rsplit_once(':').map(|(h, _)| h).unwrap_or(&t.addr);
            assert!(
                host != "100.64.0.10" && host != "198.51.100.1",
                "seed no debe incluir infra del autor: {}",
                t.addr
            );
        }
    }

    #[test]
    fn insert_load_delete_roundtrip() {
        let db = mem_db();
        let id =
            insert_target_capped(&db, "AIE local", "http", "http://localhost:8250", 30, None, "team")
                .unwrap();
        let targets = load_targets(&db);
        assert_eq!(targets.len(), 2); // mac-local + el nuevo
        assert!(targets.iter().any(|t| t.id == id && t.label == "AIE local"));
        assert_eq!(count_targets(&db).unwrap(), 2);
        assert!(delete_target(&db, &id).unwrap());
        assert_eq!(load_targets(&db).len(), 1);
        assert!(!delete_target(&db, "nope").unwrap());
    }

    #[test]
    fn insert_rejects_invalid_and_duplicate() {
        let db = mem_db();
        assert!(insert_target_capped(&db, "x", "tcp", "no-port", 30, None, "team").is_err());
        assert!(insert_target_capped(&db, "", "tcp", "1.2.3.4:22", 30, None, "team").is_err()); // label vacío
                                                                                                // duplicado del seed (kind+addr) → UNIQUE.
        let err = insert_target_capped(&db, "dup", "tcp", "127.0.0.1:22", 30, None, "team")
            .unwrap_err();
        assert!(err.to_string().contains("ya existe"), "got: {err}");
    }

    #[test]
    fn cap_enforced_atomically() {
        // audit deepseek HIGH — el cap + insert van bajo el mismo lock (sin TOCTOU).
        let db = mem_db(); // arranca con 1 (mac-local).
        // cap=2: un insert más entra, el siguiente rebota.
        assert!(insert_target_capped(&db, "a", "tcp", "1.2.3.4:1", 30, Some(2), "free").is_ok());
        let err = insert_target_capped(&db, "b", "tcp", "1.2.3.4:2", 30, Some(2), "free")
            .unwrap_err();
        assert!(err.to_string().contains("permite hasta 2"), "got: {err}");
        // cap=None (team) ⇒ ilimitado.
        assert!(insert_target_capped(&db, "c", "tcp", "1.2.3.4:3", 30, None, "team").is_ok());
    }

    #[test]
    fn disabled_targets_not_loaded() {
        let db = mem_db();
        {
            let conn = db.lock();
            conn.execute(
                "UPDATE monitor_targets SET enabled = 0 WHERE id = 'mac-local'",
                [],
            )
            .unwrap();
        }
        assert!(load_targets(&db).is_empty());
    }
}
