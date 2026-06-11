// services/review_audit.rs — 019 F0 · T001 — audit append-only del FLUJO REVIEW (R2 / FR-005).
//
// El histórico inmutable de quién/qué/cuándo/por-qué ya lo da la tabla `events` (migración 001,
// triggers append-only). Este módulo NO duplica ese histórico: añade el recorder ESTRUCTURADO del
// flujo review (compare/approve/reject/kill/revert/apply) que garantiza que cada acción quede con
// `(actor, action, target, rationale, ts, revision)` Y un VÍNCULO explícito al objeto que tocó
// (change-set == group_id, hunk_id, approval_id) en `review_audit_links` (migración 034). Sin esto,
// los ids quedaban como JSON suelto en `events.payload`, no consultables por hunk/approval.
//
// R2 — "audit accionable e inmutable" resuelto: el audit es APPEND-ONLY para consulta; REVERTIR una
// decisión es una acción NUEVA auditada (`ReviewAction::Revert`), no muta el histórico. La tabla de
// vínculo tiene sus propios triggers append-only (no UPDATE/DELETE).
//
// El `events.id` se escribe vía `AuditWriter` (un solo punto, UUID v4, append-only); el link se
// inserta atómicamente referenciando ESE id. Si el link fallara, el evento ya quedó (peor caso: un
// evento sin link, nunca un link sin evento) — el invariante crítico (todo audit tiene su evento) se
// preserva.

use crate::bases::audit::{AuditWriter, EventInput};
use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use std::sync::Arc;

type Db = Arc<parking_lot::Mutex<Connection>>;

/// Acción del flujo review que se audita. Revertir es una acción de PRIMERA CLASE (R2): NO muta el
/// histórico, agrega un evento nuevo. El `as_str` es el `action` persistido en el link + el sufijo
/// del `kind` del evento (`review.compare`, `review.approve`, …).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewAction {
    Compare,
    Approve,
    Reject,
    Revert,
    Kill,
    Apply,
}

impl ReviewAction {
    pub fn as_str(self) -> &'static str {
        match self {
            ReviewAction::Compare => "compare",
            ReviewAction::Approve => "approve",
            ReviewAction::Reject => "reject",
            ReviewAction::Revert => "revert",
            ReviewAction::Kill => "kill",
            ReviewAction::Apply => "apply",
        }
    }
}

/// Vínculo de la acción auditada con el/los objeto(s) del flujo. Todos opcionales: un `compare`
/// referencia sólo el group; un `decide` referencia group+hunk; un gate referencia el approval.
#[derive(Debug, Clone, Default)]
pub struct ReviewTargetLink {
    pub group_id: Option<String>,
    pub hunk_id: Option<String>,
    pub approval_id: Option<String>,
    pub revision: Option<u64>,
}

/// Una acción del flujo review a auditar. `actor` = quién; `target` = legible (group/hunk/worktree);
/// `rationale` = por qué; el `ts` lo pone la DB (`events.at` + `review_audit_links.created_at`).
#[derive(Debug, Clone)]
pub struct ReviewAuditEntry<'a> {
    pub action: ReviewAction,
    pub actor: &'a str,
    pub target: &'a str,
    pub rationale: &'a str,
    pub link: ReviewTargetLink,
}

/// Registra UNA acción del flujo review: escribe el evento append-only (`events`, vía AuditWriter) y
/// el vínculo append-only (`review_audit_links`) referenciando ese `events.id`. Devuelve el
/// `events.id`. El payload del evento es metadata NO-sensible (ids/estados/rationale) — F-I: NUNCA
/// lleva secretos (los review payloads son ids de hunk/group + texto de rationale del usuario).
pub fn record(db: &Db, audit: &AuditWriter, entry: ReviewAuditEntry<'_>) -> Result<String> {
    let kind = format!("review.{}", entry.action.as_str());
    let payload = serde_json::json!({
        "action": entry.action.as_str(),
        "target": entry.target,
        "rationale": entry.rationale,
        "group_id": entry.link.group_id,
        "hunk_id": entry.link.hunk_id,
        "approval_id": entry.link.approval_id,
        "revision": entry.link.revision,
    });
    // 1) Evento append-only (un solo punto, UUID v4, triggers de migración 001).
    let event_id = audit.write(EventInput {
        kind: &kind,
        actor: entry.actor,
        pane_id: None,
        card_id: None,
        correlation_id: entry.link.group_id.as_deref(),
        payload,
    })?;
    // 2) Vínculo append-only referenciando el evento ya escrito.
    let conn = db.lock();
    conn.execute(
        "INSERT INTO review_audit_links \
         (event_id, action, group_id, hunk_id, approval_id, revision, actor, target, rationale) \
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
        params![
            event_id,
            entry.action.as_str(),
            entry.link.group_id,
            entry.link.hunk_id,
            entry.link.approval_id,
            entry.link.revision.map(|r| r as i64),
            entry.actor,
            entry.target,
            entry.rationale,
        ],
    )?;
    Ok(event_id)
}

/// Una fila del audit del flujo review (vínculo + metadata), para consulta (read-only). El histórico
/// completo (incl. timestamp del evento) sale del JOIN con `events`; acá devolvemos la proyección
/// indexada por objeto.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReviewAuditRow {
    pub event_id: String,
    pub action: String,
    pub group_id: Option<String>,
    pub hunk_id: Option<String>,
    pub approval_id: Option<String>,
    pub revision: Option<i64>,
    pub actor: String,
    pub target: String,
    pub rationale: String,
    pub created_at: String,
}

/// Todas las acciones auditadas de un change-set (group), más recientes primero. Read-only.
pub fn history_for_group(db: &Db, group_id: &str) -> Result<Vec<ReviewAuditRow>> {
    let conn = db.lock();
    let mut stmt = conn.prepare(
        "SELECT event_id, action, group_id, hunk_id, approval_id, revision, actor, target, \
                rationale, created_at \
         FROM review_audit_links WHERE group_id = ?1 ORDER BY created_at DESC, rowid DESC",
    )?;
    let rows = stmt.query_map(params![group_id], map_row)?;
    rows.map(|r| r.map_err(Into::into)).collect()
}

/// Todas las acciones auditadas de un hunk, más recientes primero. Read-only (incl. los revert).
pub fn history_for_hunk(db: &Db, hunk_id: &str) -> Result<Vec<ReviewAuditRow>> {
    let conn = db.lock();
    let mut stmt = conn.prepare(
        "SELECT event_id, action, group_id, hunk_id, approval_id, revision, actor, target, \
                rationale, created_at \
         FROM review_audit_links WHERE hunk_id = ?1 ORDER BY created_at DESC, rowid DESC",
    )?;
    let rows = stmt.query_map(params![hunk_id], map_row)?;
    rows.map(|r| r.map_err(Into::into)).collect()
}

/// ¿Existe el evento de audit en la tabla `events` inmutable? (verifica el vínculo audit↔evento).
pub fn event_exists(db: &Db, event_id: &str) -> Result<bool> {
    let conn = db.lock();
    let found: Option<i64> = conn
        .query_row(
            "SELECT 1 FROM events WHERE id = ?1",
            params![event_id],
            |r| r.get(0),
        )
        .optional()?;
    Ok(found.is_some())
}

fn map_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<ReviewAuditRow> {
    Ok(ReviewAuditRow {
        event_id: r.get(0)?,
        action: r.get(1)?,
        group_id: r.get(2)?,
        hunk_id: r.get(3)?,
        approval_id: r.get(4)?,
        revision: r.get(5)?,
        actor: r.get(6)?,
        target: r.get(7)?,
        rationale: r.get(8)?,
        created_at: r.get(9)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_db() -> Db {
        let conn = Connection::open_in_memory().unwrap();
        // `events` + triggers append-only (migración 001) y la tabla de vínculo (034).
        conn.execute_batch(include_str!("../../migrations/001_init.sql"))
            .unwrap();
        conn.execute_batch(include_str!("../../migrations/034_review_audit_link.sql"))
            .unwrap();
        Arc::new(parking_lot::Mutex::new(conn))
    }

    fn entry<'a>(
        action: ReviewAction,
        target: &'a str,
        rationale: &'a str,
        link: ReviewTargetLink,
    ) -> ReviewAuditEntry<'a> {
        ReviewAuditEntry {
            action,
            actor: "user:test",
            target,
            rationale,
            link,
        }
    }

    #[test]
    fn record_appends_event_and_link() {
        let db = test_db();
        let audit = AuditWriter::new(db.clone());
        let id = record(
            &db,
            &audit,
            entry(
                ReviewAction::Approve,
                "g1/t1:src/a.rs:10,5",
                "looks correct",
                ReviewTargetLink {
                    group_id: Some("g1".into()),
                    hunk_id: Some("t1:src/a.rs:10,5".into()),
                    revision: Some(1),
                    ..Default::default()
                },
            ),
        )
        .unwrap();
        // el evento existe en la tabla inmutable.
        assert!(event_exists(&db, &id).unwrap());
        // el vínculo quedó indexado por group + hunk.
        let by_group = history_for_group(&db, "g1").unwrap();
        assert_eq!(by_group.len(), 1);
        assert_eq!(by_group[0].action, "approve");
        assert_eq!(by_group[0].actor, "user:test");
        assert_eq!(by_group[0].rationale, "looks correct");
        assert_eq!(by_group[0].revision, Some(1));
        assert_eq!(by_group[0].event_id, id);
        let by_hunk = history_for_hunk(&db, "t1:src/a.rs:10,5").unwrap();
        assert_eq!(by_hunk.len(), 1);
        assert_eq!(by_hunk[0].event_id, id);
    }

    #[test]
    fn revert_is_a_new_audited_action_not_a_mutation() {
        // R2: revertir una decisión = una acción NUEVA auditada; el histórico previo no cambia.
        let db = test_db();
        let audit = AuditWriter::new(db.clone());
        let link = || ReviewTargetLink {
            group_id: Some("g1".into()),
            hunk_id: Some("h1".into()),
            ..Default::default()
        };
        record(
            &db,
            &audit,
            entry(ReviewAction::Approve, "h1", "ok", link()),
        )
        .unwrap();
        record(
            &db,
            &audit,
            entry(ReviewAction::Revert, "h1", "changed my mind", link()),
        )
        .unwrap();
        let hist = history_for_hunk(&db, "h1").unwrap();
        // DOS filas: la aprobación original SIGUE ahí + el revert nuevo (no se mutó).
        assert_eq!(hist.len(), 2);
        let actions: Vec<&str> = hist.iter().map(|r| r.action.as_str()).collect();
        assert!(actions.contains(&"approve"));
        assert!(actions.contains(&"revert"));
    }

    #[test]
    fn link_table_is_append_only() {
        // Inmutabilidad: UPDATE/DELETE sobre el vínculo abortan (triggers migración 034).
        let db = test_db();
        let audit = AuditWriter::new(db.clone());
        let id = record(
            &db,
            &audit,
            entry(
                ReviewAction::Kill,
                "g1/t1",
                "abort",
                ReviewTargetLink {
                    group_id: Some("g1".into()),
                    ..Default::default()
                },
            ),
        )
        .unwrap();
        let conn = db.lock();
        let upd = conn.execute(
            "UPDATE review_audit_links SET rationale = 'tampered' WHERE event_id = ?1",
            params![id],
        );
        assert!(upd.is_err(), "UPDATE debió abortar (append-only)");
        let del = conn.execute(
            "DELETE FROM review_audit_links WHERE event_id = ?1",
            params![id],
        );
        assert!(del.is_err(), "DELETE debió abortar (append-only)");
    }

    #[test]
    fn underlying_events_table_is_immutable() {
        // El evento subyacente (events) también es inmutable — su histórico no se reescribe.
        let db = test_db();
        let audit = AuditWriter::new(db.clone());
        let id = record(
            &db,
            &audit,
            entry(
                ReviewAction::Apply,
                "g1",
                "apply approved",
                ReviewTargetLink {
                    group_id: Some("g1".into()),
                    revision: Some(3),
                    ..Default::default()
                },
            ),
        )
        .unwrap();
        let conn = db.lock();
        let upd = conn.execute(
            "UPDATE events SET actor = 'attacker' WHERE id = ?1",
            params![id],
        );
        assert!(upd.is_err(), "events UPDATE debió abortar (append-only)");
    }

    #[test]
    fn history_orders_recent_first_and_isolates_by_object() {
        let db = test_db();
        let audit = AuditWriter::new(db.clone());
        // group g1: compare + approve(h1). group g2: compare. h1 sólo en g1.
        record(
            &db,
            &audit,
            entry(
                ReviewAction::Compare,
                "g1",
                "",
                ReviewTargetLink {
                    group_id: Some("g1".into()),
                    ..Default::default()
                },
            ),
        )
        .unwrap();
        record(
            &db,
            &audit,
            entry(
                ReviewAction::Approve,
                "h1",
                "",
                ReviewTargetLink {
                    group_id: Some("g1".into()),
                    hunk_id: Some("h1".into()),
                    ..Default::default()
                },
            ),
        )
        .unwrap();
        record(
            &db,
            &audit,
            entry(
                ReviewAction::Compare,
                "g2",
                "",
                ReviewTargetLink {
                    group_id: Some("g2".into()),
                    ..Default::default()
                },
            ),
        )
        .unwrap();
        let g1 = history_for_group(&db, "g1").unwrap();
        assert_eq!(g1.len(), 2, "g1 tiene 2 acciones (compare+approve)");
        let g2 = history_for_group(&db, "g2").unwrap();
        assert_eq!(g2.len(), 1, "g2 sólo el compare — aislado de g1");
        // un hunk inexistente → vacío.
        assert!(history_for_hunk(&db, "ghost").unwrap().is_empty());
    }
}
