// spec-050 · Ola 8 P2 (FR-003) — Reliability board.
//
// OBSERVACIONAL + OPT-IN. Mide CALIDAD (no ahorro $): tasa de éxito, latencia y costo por AGENTE y
// por MODELO, agregando la tabla append-only `reliability_events` (055). Distinto del cost-router
// (053 `cost_router_events` = savings); por eso vive en su propia tabla/servicio — las dos features
// evolucionan en paralelo sin acoplarse.
//
// Gating (cero regresión): el recorder solo persiste si el setting `reliability.board_enabled` está
// ON (default OFF). El board (lectura) devuelve `enabled=false` + agregados vacíos si está OFF →
// nada nuevo aparece hasta que el usuario lo active explícitamente. Solo-medido: NUNCA proyecta ni
// promete; reporta los números observados y listo.
//
// PRIVACIDAD: la tabla no tiene texto libre (mismo invariante que 053). Acá no hay scrub porque no
// hay payloads — solo enum de agente, ids de modelo/provider y números.

use parking_lot::Mutex;
use rusqlite::{params, Connection};
use serde::Serialize;

/// Setting que habilita el board (recorder + lectura). Default OFF.
pub const ENABLED_SETTING: &str = "reliability.board_enabled";

/// `true` si el board está habilitado (lee el setting; default OFF). Acepta el valor como bool JSON
/// (toggle de la UI) o string `"1"`/`"true"` (defensa; settings es un KV de `serde_json::Value`).
pub fn is_enabled(conn: &Connection) -> bool {
    match crate::settings::get(conn, ENABLED_SETTING).ok().flatten() {
        Some(v) => v
            .as_bool()
            .unwrap_or_else(|| matches!(v.as_str(), Some("1") | Some("true"))),
        None => false,
    }
}

/// Un outcome de corrida a registrar. Construido por el caller en el punto donde ya conoce el
/// resultado (verdict + modelo/provider/latencia/costo). Campos opcionales = `None` cuando no medible.
#[derive(Debug, Clone)]
pub struct Outcome<'a> {
    pub agent_kind: &'a str,
    pub model: Option<&'a str>,
    pub provider: Option<&'a str>,
    pub success: bool,
    pub latency_ms: Option<i64>,
    pub cost_usd: Option<f64>,
}

/// Persiste UN outcome (append-only). NO-OP si el board está OFF (opt-in → cero regresión). Best-effort:
/// cualquier error de DB se traga (no rompe el hot-path del agente). Devuelve `Some(event_id)` si
/// persistió, `None` si estaba OFF o falló.
pub fn record(db: &Mutex<Connection>, o: &Outcome<'_>) -> Option<String> {
    let conn = db.lock();
    if !is_enabled(&conn) {
        return None;
    }
    // Saneo del enum de agente: cae a 'unknown' si viene vacío (defensa; no es texto libre).
    let agent = {
        let a = o.agent_kind.trim();
        if a.is_empty() {
            "unknown"
        } else {
            a
        }
    };
    let id = uuid::Uuid::new_v4().to_string();
    let res = conn.execute(
        "INSERT INTO reliability_events \
           (event_id, agent_kind, model, provider, success, latency_ms, cost_usd) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            id,
            agent,
            o.model,
            o.provider,
            o.success as i64,
            o.latency_ms,
            o.cost_usd,
        ],
    );
    match res {
        Ok(_) => Some(id),
        Err(e) => {
            tracing::warn!("reliability: insert failed: {e}");
            None
        }
    }
}

/// Fila agregada del board: métricas por una dimensión (agente o modelo).
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ReliabilityRow {
    /// Etiqueta de la dimensión (agent_kind, o "model@provider" / model). "(desconocido)" si NULL.
    pub label: String,
    pub runs: i64,
    pub successes: i64,
    /// Tasa de éxito 0..100 (solo-medido). 0 si runs=0.
    pub success_pct: f64,
    /// Latencia mediana aproximada por promedio (no percentil; AVG sobre filas con latency no-NULL).
    pub avg_latency_ms: Option<f64>,
    pub p95_latency_ms: Option<i64>,
    /// Costo total observado (suma de cost_usd no-NULL). 0 si todo NULL.
    pub total_cost_usd: f64,
}

/// Resumen del board. `enabled=false` ⇒ filas vacías (gating simétrico con el recorder).
#[derive(Debug, Clone, Serialize)]
pub struct ReliabilitySummary {
    pub enabled: bool,
    pub window_days: i64,
    pub total_runs: i64,
    pub by_agent: Vec<ReliabilityRow>,
    pub by_model: Vec<ReliabilityRow>,
}

fn empty_summary(window_days: i64) -> ReliabilitySummary {
    ReliabilitySummary {
        enabled: false,
        window_days,
        total_runs: 0,
        by_agent: Vec::new(),
        by_model: Vec::new(),
    }
}

/// Agrega el board dentro de la ventana (`window_days`). OFF ⇒ resumen vacío. Solo-medido.
pub fn compute_summary(conn: &Connection, window_days: i64) -> ReliabilitySummary {
    if !is_enabled(conn) {
        return empty_summary(window_days);
    }
    let cutoff = format!("-{} days", window_days.max(0));
    let by_agent = aggregate(conn, "agent_kind", &cutoff);
    let by_model = aggregate(conn, "model_provider", &cutoff);
    let total_runs: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM reliability_events WHERE ts >= datetime('now', ?1)",
            params![cutoff],
            |r| r.get(0),
        )
        .unwrap_or(0);
    ReliabilitySummary {
        enabled: true,
        window_days,
        total_runs,
        by_agent,
        by_model,
    }
}

/// Agrega por la dimensión pedida. `dim` ∈ {"agent_kind","model_provider"}; cualquier otro cae a
/// agent_kind (defensa; `dim` NO se interpola crudo — se elige el SQL por match, sin inyección).
fn aggregate(conn: &Connection, dim: &str, cutoff: &str) -> Vec<ReliabilityRow> {
    // El GROUP BY usa una expresión fija elegida por match (no string-interp del input).
    let group_expr = match dim {
        "model_provider" => {
            // model + provider como una sola etiqueta legible; NULLs → marcador.
            "COALESCE(model, '(desconocido)') || \
             CASE WHEN provider IS NOT NULL AND provider <> '' \
                  THEN '@' || provider ELSE '' END"
        }
        _ => "agent_kind",
    };
    let sql = format!(
        "SELECT {grp} AS label, \
                COUNT(*) AS runs, \
                SUM(success) AS successes, \
                AVG(latency_ms) AS avg_lat, \
                SUM(COALESCE(cost_usd, 0)) AS total_cost \
         FROM reliability_events \
         WHERE ts >= datetime('now', ?1) \
         GROUP BY {grp} \
         ORDER BY runs DESC",
        grp = group_expr
    );
    let mut stmt = match conn.prepare(&sql) {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!("reliability: prepare aggregate({dim}) failed: {e}");
            return Vec::new();
        }
    };
    let rows = stmt.query_map(params![cutoff], |r| {
        let label: String = r.get(0)?;
        let runs: i64 = r.get(1)?;
        let successes: i64 = r.get::<_, Option<i64>>(2)?.unwrap_or(0);
        let avg_lat: Option<f64> = r.get(3)?;
        let total_cost: f64 = r.get::<_, Option<f64>>(4)?.unwrap_or(0.0);
        Ok((label, runs, successes, avg_lat, total_cost))
    });
    let mut out = Vec::new();
    if let Ok(it) = rows {
        for r in it.flatten() {
            let (label, runs, successes, avg_lat, total_cost) = r;
            let success_pct = if runs > 0 {
                (successes as f64 / runs as f64) * 100.0
            } else {
                0.0
            };
            let p95 = p95_latency_for(conn, dim, &label, cutoff);
            out.push(ReliabilityRow {
                label,
                runs,
                successes,
                success_pct,
                avg_latency_ms: avg_lat,
                p95_latency_ms: p95,
                total_cost_usd: total_cost,
            });
        }
    }
    out
}

/// p95 de latencia para una etiqueta de la dimensión (sobre filas con latency no-NULL en la ventana).
/// SQLite no tiene PERCENTILE; lo aproximamos por offset sobre la lista ordenada. `None` si no hay
/// muestras de latencia.
fn p95_latency_for(conn: &Connection, dim: &str, label: &str, cutoff: &str) -> Option<i64> {
    // El WHERE de la dimensión se construye por match (no interp del input crudo); el `label` va
    // siempre como parámetro bound.
    let (where_dim, bind_label): (&str, bool) = match dim {
        "model_provider" => (
            "(COALESCE(model, '(desconocido)') || \
              CASE WHEN provider IS NOT NULL AND provider <> '' \
                   THEN '@' || provider ELSE '' END) = ?2",
            true,
        ),
        _ => ("agent_kind = ?2", true),
    };
    debug_assert!(bind_label);
    let n: i64 = conn
        .query_row(
            &format!(
                "SELECT COUNT(*) FROM reliability_events \
                 WHERE ts >= datetime('now', ?1) AND latency_ms IS NOT NULL AND {where_dim}"
            ),
            params![cutoff, label],
            |r| r.get(0),
        )
        .unwrap_or(0);
    if n == 0 {
        return None;
    }
    // índice del p95 (0-based) sobre n muestras ordenadas ascendente.
    let idx = (((n as f64) * 0.95).ceil() as i64 - 1).clamp(0, n - 1);
    conn.query_row(
        &format!(
            "SELECT latency_ms FROM reliability_events \
             WHERE ts >= datetime('now', ?1) AND latency_ms IS NOT NULL AND {where_dim} \
             ORDER BY latency_ms ASC LIMIT 1 OFFSET ?3"
        ),
        params![cutoff, label, idx],
        |r| r.get(0),
    )
    .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mem_db() -> Mutex<Connection> {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(include_str!("../../migrations/001_init.sql"))
            .unwrap();
        conn.execute_batch(include_str!("../../migrations/002_settings.sql"))
            .unwrap();
        conn.execute_batch(include_str!("../../migrations/055_reliability_events.sql"))
            .unwrap();
        Mutex::new(conn)
    }

    fn set_enabled(db: &Mutex<Connection>, on: bool) {
        let conn = db.lock();
        crate::settings::set(&conn, ENABLED_SETTING, &serde_json::Value::Bool(on)).unwrap();
    }

    fn out<'a>(agent: &'a str, model: Option<&'a str>, ok: bool, lat: Option<i64>, cost: Option<f64>) -> Outcome<'a> {
        Outcome {
            agent_kind: agent,
            model,
            provider: model.map(|_| "aie"),
            success: ok,
            latency_ms: lat,
            cost_usd: cost,
        }
    }

    #[test]
    fn record_is_noop_when_disabled() {
        // SC-002 / cero regresión: OFF default → record no persiste.
        let db = mem_db();
        assert!(record(&db, &out("claude", Some("m"), true, Some(100), Some(0.0))).is_none());
        let conn = db.lock();
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM reliability_events", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 0, "OFF no debe persistir nada");
    }

    #[test]
    fn summary_off_is_empty() {
        let db = mem_db();
        let conn = db.lock();
        let s = compute_summary(&conn, 30);
        assert!(!s.enabled);
        assert_eq!(s.total_runs, 0);
        assert!(s.by_agent.is_empty() && s.by_model.is_empty());
    }

    #[test]
    fn aggregates_by_agent_and_model() {
        let db = mem_db();
        set_enabled(&db, true);
        // claude: 2 runs, 1 success; gemini: 1 run, 1 success.
        record(&db, &out("claude", Some("sonnet"), true, Some(100), Some(0.01))).unwrap();
        record(&db, &out("claude", Some("sonnet"), false, Some(300), Some(0.02))).unwrap();
        record(&db, &out("gemini", Some("flash"), true, Some(50), None)).unwrap();

        let conn = db.lock();
        let s = compute_summary(&conn, 30);
        assert!(s.enabled);
        assert_eq!(s.total_runs, 3);

        let claude = s.by_agent.iter().find(|r| r.label == "claude").unwrap();
        assert_eq!(claude.runs, 2);
        assert_eq!(claude.successes, 1);
        assert!((claude.success_pct - 50.0).abs() < 1e-9);
        assert!((claude.total_cost_usd - 0.03).abs() < 1e-9);

        // by_model: sonnet@aie tiene 2 runs.
        let sonnet = s.by_model.iter().find(|r| r.label == "sonnet@aie").unwrap();
        assert_eq!(sonnet.runs, 2);
        assert_eq!(sonnet.successes, 1);
    }

    #[test]
    fn p95_latency_picks_high_sample() {
        let db = mem_db();
        set_enabled(&db, true);
        for ms in [10, 20, 30, 40, 1000] {
            record(&db, &out("claude", Some("m"), true, Some(ms), None)).unwrap();
        }
        let conn = db.lock();
        let s = compute_summary(&conn, 30);
        let claude = s.by_agent.iter().find(|r| r.label == "claude").unwrap();
        // p95 de 5 muestras → idx ceil(5*0.95)-1 = ceil(4.75)-1 = 5-1 = 4 → la mayor (1000).
        assert_eq!(claude.p95_latency_ms, Some(1000));
    }

    #[test]
    fn append_only_blocks_mutation() {
        let db = mem_db();
        set_enabled(&db, true);
        record(&db, &out("claude", Some("m"), true, Some(1), None)).unwrap();
        let conn = db.lock();
        let upd = conn.execute("UPDATE reliability_events SET success = 0", []);
        assert!(upd.is_err(), "UPDATE debe abortar (append-only)");
        let del = conn.execute("DELETE FROM reliability_events", []);
        assert!(del.is_err(), "DELETE debe abortar (append-only)");
    }
}
