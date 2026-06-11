use anyhow::Result;
use once_cell::sync::Lazy;
use rusqlite::Connection;
use rusqlite_migration::{Migrations, M};
use std::path::Path;

static MIGRATIONS: Lazy<Migrations<'static>> = Lazy::new(|| {
    Migrations::new(vec![
        M::up(include_str!("../migrations/001_init.sql")),
        M::up(include_str!("../migrations/002_settings.sql")),
        M::up(include_str!("../migrations/003_layout_default.sql")),
        M::up(include_str!("../migrations/004_sprint_tables.sql")),
        M::up(include_str!("../migrations/005_cards_context.sql")),
        M::up(include_str!("../migrations/006_grafana_default.sql")),
        M::up(include_str!("../migrations/007_provider_latency.sql")),
        M::up(include_str!("../migrations/008_bg_jobs.sql")),
        M::up(include_str!("../migrations/009_b4_extras.sql")),
        M::up(include_str!("../migrations/010_b5.sql")),
        M::up(include_str!("../migrations/011_provider_credentials.sql")),
        M::up(include_str!("../migrations/012_preset_overrides.sql")),
        M::up(include_str!("../migrations/013_claude_accounts.sql")),
        M::up(include_str!(
            "../migrations/014_cli_accounts_generalize.sql"
        )),
        M::up(include_str!("../migrations/015_skills.sql")),
        M::up(include_str!("../migrations/016_memory.sql")),
        M::up(include_str!("../migrations/017_knowledge_graph.sql")),
        M::up(include_str!("../migrations/018_council_templates.sql")),
        M::up(include_str!("../migrations/019_agent_profiles.sql")),
        M::up(include_str!("../migrations/020_agent_engine_presets.sql")),
        M::up(include_str!(
            "../migrations/021_memory_project_namespace.sql"
        )),
        M::up(include_str!("../migrations/022_orchestration.sql")),
        M::up(include_str!("../migrations/023_signals.sql")),
        M::up(include_str!("../migrations/024_done_detection.sql")),
        M::up(include_str!("../migrations/025_orchestration_ux.sql")),
        // NOTE: 026 lo usa otra ventana (015 reform kernel). 026 = US6 layout config (026 estaba libre).
        M::up(include_str!("../migrations/026_layout_config.sql")),
        // 015 reform kernel · US5 — process/task lifecycle registry (procesos que sobreviven a la UI).
        M::up(include_str!("../migrations/027_process_registry.sql")),
        // 015 US4 — capability/approval gate: estado `pending_approval` de primera clase.
        // 027 la usa otra ventana del reform kernel; 028 es la próxima libre acá.
        M::up(include_str!("../migrations/028_approvals.sql")),
        // 015 T014 (audit final) — run_token: scope de generación para `finish`, cierra la race
        // wait-thread-hijack del respawn del mismo pane_id.
        M::up(include_str!("../migrations/029_process_run_token.sql")),
        // 015 T015 — enforcement universal del gate US4: args_hash + consumed_at para el
        // protocolo approve→execute (consume single-use, sin replay).
        M::up(include_str!("../migrations/030_approval_consume.sql")),
        // 018 Fase 2 B0 (T063) — revisión monotónica del layout (optimistic concurrency).
        M::up(include_str!("../migrations/031_layout_revision.sql")),
        // 019 F0 — review projection (hunk-level sobre orchestration, council opción L).
        M::up(include_str!("../migrations/032_review_projection.sql")),
        // 019 F0 (audit codex #1) — cuerpo del hunk para apply-desde-snapshot (anti-drift).
        M::up(include_str!("../migrations/033_review_hunk_body.sql")),
        // 019 F0 · T001 — audit append-only del flujo review: vínculo audit↔change_set/hunk/approval.
        M::up(include_str!("../migrations/034_review_audit_link.sql")),
        // 019 F0 · T005 — checkpoint-por-attempt para el kill-switch transaccional (R4).
        M::up(include_str!("../migrations/035_attempt_checkpoint.sql")),
        // 019 (audit codex r2) — fencing token (kill_token) para el claim del kill-switch.
        M::up(include_str!(
            "../migrations/036_attempt_checkpoint_kill_token.sql"
        )),
        // 019 F3 (T030/T031) — pause/resume por attempt + council history + custom-voices.
        M::up(include_str!(
            "../migrations/037_orch_pause_council_history.sql"
        )),
        // 019 T024 — retención + exportabilidad del audit (manifests append-only de export/rotación).
        M::up(include_str!("../migrations/038_audit_retention.sql")),
        // spec-022 US1 (audit MED 1) — dedup plugins por name + UNIQUE(name) para que
        // set_enabled sea un upsert atómico (sin SELECT previo, sin race read-then-write).
        M::up(include_str!("../migrations/039_plugins_unique_name.sql")),
        // spec-022 P1 · US6 — incidents inbox: snooze_until / read_at / dismissed_at /
        // last_activity_at / reopened sobre `cards` (mark-read, dismiss, auto-unsnooze).
        M::up(include_str!("../migrations/040_card_inbox.sql")),
        // spec-023 F0 — memoria auto-captura: procedencia/kind/rationale en memory_entries +
        // tabla `memory_proposals` (bandeja de revisión) + `incomplete_sessions` (resguardo TTL).
        M::up(include_str!("../migrations/041_memory_autocapture.sql")),
        // spec-025 F0/F1 — loop de gotchas procedurales: `failure_signals` (lado fallo del par
        // fallo->fix) + `lesson_activation` (activación por lección para la inyección por perfil).
        M::up(include_str!("../migrations/042_procedural_gotchas.sql")),
        // spec-026 F0 · T004 — preference loop sobre best-of-N: `preference_records` +
        // `variant_features` (append-only, la señal derivada del audit) + `context_priors` +
        // `context_prior_meta` (mutable, el prior Beta por contexto que enriquece el ranking 020).
        M::up(include_str!("../migrations/043_preference_loop.sql")),
        // 027 F2-wiring — `policy_rules` (reglas custom vigentes del gate, mutable, id UNIQUE) +
        // `policy_rule_changes` (audit append-only de los cambios de política, triggers anti
        // UPDATE/DELETE). Default OFF vía setting `policy.custom_enabled`.
        M::up(include_str!("../migrations/044_policy_custom_rules.sql")),
        // 028 F0 — `acp_agents`: registro declarativo de agentes ACP (Zed/JetBrains style),
        // reemplaza el binario ACP hardcodeado. Semilla default lazy = cero-regresión.
        M::up(include_str!("../migrations/045_acp_agents.sql")),
        // 033 U3 — `attention_dismissed`: descartes persistentes de la cola de atención (no molestar
        // hasta nueva actividad del pane). Comparación dismissed_at vs task.updated_at (RFC3339).
        M::up(include_str!("../migrations/046_attention_dismissed.sql")),
        // 038 Goose-C P1 (F1.0) — ejecución del DAG de pipelines: `pipeline_runs` (grafo congelado
        // por run) + `pipeline_edges` (depends_on normalizado, FKs ON DELETE CASCADE) + ALTER
        // orchestration_tasks ADD pipeline_run_id/dag_blocked DEFAULT 0/topo_index. ADITIVA:
        // single-task idéntico (dag_blocked=0, sin aristas).
        M::up(include_str!("../migrations/047_pipeline_dag.sql")),
        // 041 FR-004 — defaults patch (multi-usuario): NULL-ea cualquier `endpoints.*` que en un
        // install viejo hubiera quedado apuntando a infra específica del autor (IPs/dominios/repo
        // privados) → caen al default localhost. Idempotente. Los installs nuevos ya seedean limpio.
        M::up(include_str!("../migrations/048_defaults_patch.sql")),
        // 043 F2 (Ola 4 Skills) — registry de confianza local: ALTER plugins ADD
        // trust_level/inert/pending_verification/staging_path/last_verified_at/
        // status_message/tree_hash (ADITIVO, default NULL/0 → cero regresión legacy) + índice recovery.
        M::up(include_str!("../migrations/049_skill_trust.sql")),
        // 045 FR-001 (Ola 5 P1) — monitores configurables: tabla `monitor_targets` que el
        // daemon poller lee (reemplaza el hardcode). Seed = SOLO mac-local. ADITIVA.
        M::up(include_str!("../migrations/050_monitor_targets.sql")),
        // 045 FR-002 (Ola 5 P1) — overrides de MCP servers (enabled/disabled por usuario).
        // DB = fuente de verdad en runtime sobre lo que dice ~/.claude.json. ADITIVA.
        M::up(include_str!("../migrations/051_mcp_user_overrides.sql")),
        // 046 (Ola 7 Skills P1) — layout versionado de skills (tabla `skill_versions` para
        // update/rollback/GC, FR-001) + columna `scripts_cache_snapshot` (fast-path de
        // re-verificación por-archivo, FR-002). ADITIVA → cero regresión del install-only.
        M::up(include_str!("../migrations/052_skill_versions.sql")),
        // 048 (Cost-Router Fase 1 · Savings Meter) — tabla append-only `cost_router_events`
        // (mide el ahorro del routing local/free/premium, NO desvía) + `price_table` versionada
        // para el baseline premium BYOK. Triggers BEFORE UPDATE/DELETE → RAISE(ABORT). ADITIVA.
        M::up(include_str!("../migrations/053_cost_router_events.sql")),
        // 049 (Cost-Router Fase 2 · Router ACTIVO) — `router_quotas`: presupuesto de tokens FREE
        // por tenant/mes para el router que DESVÍA (OFF detrás de `FURX_COST_ROUTER_MODE`). El
        // consumo usa un lock no-bloqueante (SKIP-LOCKED equivalente en SQLite). ADITIVA.
        M::up(include_str!("../migrations/054_cost_router_quota.sql")),
        // 050 Ola 8 P2 (FR-003 · Reliability board) — tabla append-only `reliability_events`
        // (tasa de éxito / latencia / costo por agente y por modelo, observacional, opt-in via
        // setting `reliability.board_enabled`). DISTINTA de 053 (ahorro $); triggers no-update/delete.
        // 054 queda reservado a cost-router F2 (mergea aparte). ADITIVA → cero regresión.
        M::up(include_str!("../migrations/055_reliability_events.sql")),
        // 050 Ola 8 P2 (FR-002 · Gotcha feedback loop) — tabla `lesson_feedback` (votos útil/no-útil
        // por lección procedural aprobada). ADVISORY: NUNCA auto-desactiva (decisión humana). ADITIVA.
        M::up(include_str!("../migrations/056_lesson_feedback.sql")),
        // 050 Ola 8 P2 (FR-001 · Multi-machine sync) — tabla `sync_meta` (last-write `(updated_at,
        // installation_id)` por item sincronizable). Opt-in + fail-closed; tiebreaker LWW, no CRDT.
        // No toca las tablas de origen (overrides/targets/lessons). ADITIVA → cero regresión.
        M::up(include_str!("../migrations/057_sync_meta.sql")),
        // 051/US1 — brand: default coral en project_themes (supersede el legacy de 009).
        M::up(include_str!("../migrations/058_brand_accent_default.sql")),
        // 052 (Cost-Router Classifier v2 · bandit-ready) — `cost_router_state` (installation_id estable),
        // `bandit_state` (EWMA ε-greedy mutable), `cost_router_decisions` (decision_id generado por F2),
        // `cost_router_outcomes` (cierra el contrato F1→F2: success+latency que 053 no emite). TODO OFF
        // detrás de `FURX_COST_ROUTER_MODE`. ADITIVA → cero regresión.
        M::up(include_str!("../migrations/059_cost_router_v2.sql")),
        // 060 — first-run UX: el default legacy de 003 auto-spawneaba claude-A/B/codex en un
        // install nuevo (panes con errores de cuentas). Pasa a 1 pane zsh; solo pisa el seed
        // intacto (layout customizado por el usuario no se toca).
        M::up(include_str!("../migrations/060_default_layout_clean.sql")),
    ])
});

pub fn open(path: &Path) -> Result<Connection> {
    let mut conn = Connection::open(path)?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.pragma_update(None, "busy_timeout", 5000)?;
    MIGRATIONS.to_latest(&mut conn)?;
    Ok(conn)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Toda la cadena de migraciones aplica de cero, en orden, sin error (incluye 047 F1.0).
    /// rusqlite_migration valida internamente que la secuencia sea monotónica y bien formada.
    #[test]
    fn migrations_apply_clean() {
        let conn = open(Path::new(":memory:")).expect("migraciones aplican");
        let user_version: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .expect("user_version");
        assert!(user_version >= 48, "deben aplicarse ≥48 migraciones, fue {user_version}");
    }

    /// 060 — un install FRESCO no debe auto-spawnear CLIs de agentes: el layout 'default'
    /// queda en un único pane zsh (la primera impresión es el EmptyShellState + 1 shell,
    /// no 4 panes con errores de cuentas inexistentes).
    #[test]
    fn fresh_install_default_layout_is_single_zsh_pane() {
        let conn = open(Path::new(":memory:")).expect("migraciones aplican");
        let panes: String = conn
            .query_row("SELECT panes FROM layouts WHERE id='default'", [], |r| r.get(0))
            .expect("layout default existe");
        assert!(!panes.contains("claude"), "el default no debe referenciar cuentas claude: {panes}");
        assert!(!panes.contains("codex"), "el default no debe spawnear codex: {panes}");
        let parsed: serde_json::Value = serde_json::from_str(&panes).expect("panes JSON");
        let arr = parsed.as_array().expect("array de panes");
        assert_eq!(arr.len(), 1, "default = un solo pane, fue {}", arr.len());
        assert_eq!(arr[0]["mode"], "zsh");
    }

    /// 060 — guard de preservación: si el usuario customizó CUALQUIER campo del layout
    /// 'default' (aun dejando los panes legacy), la migración NO lo toca.
    #[test]
    fn migration_060_preserves_customized_default_layout() {
        let conn = open(Path::new(":memory:")).expect("migraciones aplican");
        let legacy_panes = r#"[{"id":"p1","mode":"claude-A","title":"Pane 1"},{"id":"p2","mode":"claude-B","title":"Pane 2"},{"id":"p3","mode":"codex","title":"Pane 3"},{"id":"p4","mode":"zsh","title":"Pane 4"}]"#;
        conn.execute(
            "UPDATE layouts SET name='Mi layout', panes=?1, grid_cols='2fr 1fr', grid_rows='1fr 1fr' WHERE id='default'",
            [legacy_panes],
        )
        .expect("customizar");
        conn.execute_batch(include_str!("../migrations/060_default_layout_clean.sql"))
            .expect("060 re-aplica");
        let (name, panes): (String, String) = conn
            .query_row("SELECT name, panes FROM layouts WHERE id='default'", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .expect("layout");
        assert_eq!(name, "Mi layout", "el nombre customizado no debe pisarse");
        assert_eq!(panes, legacy_panes, "los panes customizados no deben pisarse");
    }

    /// 060 — idempotencia: re-aplicar la migración sobre un default ya migrado es no-op.
    #[test]
    fn migration_060_is_idempotent() {
        let conn = open(Path::new(":memory:")).expect("migraciones aplican");
        let before: String = conn
            .query_row("SELECT panes FROM layouts WHERE id='default'", [], |r| r.get(0))
            .expect("panes");
        conn.execute_batch(include_str!("../migrations/060_default_layout_clean.sql"))
            .expect("060 re-aplica");
        let after: String = conn
            .query_row("SELECT panes FROM layouts WHERE id='default'", [], |r| r.get(0))
            .expect("panes");
        assert_eq!(before, after);
    }

    /// 041 SC-002 — un DB fresh (toda la cadena 001..048) NO deja activo ningún `endpoints.*` que
    /// apunte al IP/infra del autor. 002 ahora seedea defaults LIMPIOS (endpoints.aie=localhost,
    /// telemetry/updates vacíos); 048 limpia cualquier resto de infra del autor en installs viejos.
    #[test]
    fn fresh_db_has_no_hernan_endpoints_after_migration() {
        let conn = open(Path::new(":memory:")).expect("db");
        // endpoints.aie es el default seguro localhost (no la IP del autor).
        let aie: String = conn
            .query_row(
                "SELECT json_extract(value,'$') FROM settings WHERE key='endpoints.aie'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            aie, "http://localhost:8250",
            "endpoints.aie debe ser el default localhost limpio (seedeado por 002)"
        );
        // Ningún endpoints.* contiene la IP/dominios/repo del autor.
        let leaks: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM settings WHERE key LIKE 'endpoints.%' AND (
                    json_extract(value,'$') LIKE '%100.64.0.10%'
                 OR json_extract(value,'$') LIKE '%example.internal%'
                 OR json_extract(value,'$') LIKE '%example.test%'
                 OR json_extract(value,'$') LIKE '%devserver.local%'
                 OR json_extract(value,'$') LIKE '%furx-legacy/private%'
                 OR json_extract(value,'$') LIKE '%telemetry.example.internal%')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(leaks, 0, "ningún endpoints.* debe apuntar a infra del autor");
    }

    /// 051/US1 — el default de accent_hex es el coral FUNCIONAL accesible #bf3f18 (AA), NO el
    /// color de marca legacy de la migración 009 (que la 058 supersede) ni el #FF5C35 de logo
    /// (que falla AA como color funcional). accent_hex se renderiza como color funcional en runtime.
    #[test]
    fn project_theme_default_is_coral_not_legacy() {
        let conn = open(Path::new(":memory:")).expect("db");
        conn.execute("INSERT INTO project_themes (project) VALUES ('p1')", [])
            .expect("insert sin accent_hex usa el DEFAULT");
        let hex: String = conn
            .query_row(
                "SELECT accent_hex FROM project_themes WHERE project='p1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(hex, "#bf3f18", "el default debe ser el coral funcional AA, no marca ni legacy");
    }

    /// 041 SC-003 — la migración 048 es idempotente y selectiva: un settings con la IP del autor
    /// queda vacío; uno con localhost o con la infra PROPIA del usuario NO se toca; 2 corridas = mismo
    /// estado.
    #[test]
    fn migration_048_is_idempotent_and_selective() {
        let sql = include_str!("../migrations/048_defaults_patch.sql");
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT, updated_at TEXT DEFAULT (datetime('now')));
             INSERT INTO settings (key,value) VALUES ('endpoints.aie', json('\"http://100.64.0.10:8250\"'));
             INSERT INTO settings (key,value) VALUES ('endpoints.ollama', json('\"http://100.64.0.10:11434\"'));
             INSERT INTO settings (key,value) VALUES ('endpoints.updates', json('\"https://api.github.com/repos/furx-legacy/private-releases/latest\"'));
             INSERT INTO settings (key,value) VALUES ('endpoints.telemetry', json('\"https://telemetry.example.internal/v1/events\"'));
             -- infra PROPIA del usuario: NO se debe tocar.
             INSERT INTO settings (key,value) VALUES ('endpoints.mine', json('\"http://192.168.1.50:8250\"'));
             INSERT INTO settings (key,value) VALUES ('endpoints.local', json('\"http://localhost:8250\"'));
             -- un Tailscale 100.x del usuario (NO el del autor): permitido, NO se toca.
             INSERT INTO settings (key,value) VALUES ('endpoints.tailnet', json('\"http://100.99.1.2:8250\"'));
             -- valor NO-string (objeto JSON) que casualmente contiene la IP en una clave: json_type
             -- guard lo deja intacto (no es un endpoint string).
             INSERT INTO settings (key,value) VALUES ('endpoints.obj', json('{\"host\":\"100.64.0.10\"}'));",
        )
        .unwrap();

        let run = |c: &Connection| c.execute_batch(sql).unwrap();
        let extract = |c: &Connection, k: &str| -> String {
            c.query_row(
                "SELECT json_extract(value,'$') FROM settings WHERE key=?1",
                [k],
                |r| r.get(0),
            )
            .unwrap()
        };

        run(&conn);
        // Infra del autor → vacía.
        assert_eq!(extract(&conn, "endpoints.aie"), "");
        assert_eq!(extract(&conn, "endpoints.ollama"), "");
        assert_eq!(extract(&conn, "endpoints.updates"), "");
        assert_eq!(extract(&conn, "endpoints.telemetry"), "");
        // Infra propia del usuario → intacta.
        assert_eq!(extract(&conn, "endpoints.mine"), "http://192.168.1.50:8250");
        assert_eq!(extract(&conn, "endpoints.local"), "http://localhost:8250");
        assert_eq!(extract(&conn, "endpoints.tailnet"), "http://100.99.1.2:8250");
        // Valor no-string (objeto) NO se toca (json_type guard).
        let obj_type: String = conn
            .query_row(
                "SELECT json_type(value,'$') FROM settings WHERE key='endpoints.obj'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(obj_type, "object", "el objeto JSON debe quedar intacto");

        // Snapshot del estado tras la 1ª corrida.
        let snap1: Vec<(String, String)> = {
            let mut stmt = conn
                .prepare("SELECT key, COALESCE(json_extract(value,'$'),'') FROM settings ORDER BY key")
                .unwrap();
            stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
                .unwrap()
                .collect::<std::result::Result<_, _>>()
                .unwrap()
        };
        // 2ª corrida: idempotente.
        run(&conn);
        let snap2: Vec<(String, String)> = {
            let mut stmt = conn
                .prepare("SELECT key, COALESCE(json_extract(value,'$'),'') FROM settings ORDER BY key")
                .unwrap();
            stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
                .unwrap()
                .collect::<std::result::Result<_, _>>()
                .unwrap()
        };
        assert_eq!(snap1, snap2, "048 debe ser idempotente (2 corridas = mismo estado)");
    }

    /// 038 F1.0 — las tablas del DAG y las columnas ALTER existen tras 047.
    #[test]
    fn pipeline_dag_schema_present() {
        let conn = open(Path::new(":memory:")).expect("db");
        // Tablas nuevas.
        for table in ["pipeline_runs", "pipeline_edges"] {
            let n: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
                    [table],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(n, 1, "tabla {table} debe existir tras 047");
        }
        // Columnas ALTER sobre orchestration_tasks.
        let cols: Vec<String> = {
            let mut stmt = conn.prepare("PRAGMA table_info(orchestration_tasks)").unwrap();
            stmt.query_map([], |r| r.get::<_, String>(1))
                .unwrap()
                .collect::<std::result::Result<_, _>>()
                .unwrap()
        };
        for c in ["pipeline_run_id", "dag_blocked", "topo_index"] {
            assert!(cols.iter().any(|x| x == c), "columna {c} debe existir");
        }
    }

    /// 038 F1.0 — una tarea LEGACY (insertada como hoy, sin pipeline) trae `dag_blocked=0`
    /// y `pipeline_run_id`/`topo_index` NULL por default → comportamiento single-task idéntico.
    #[test]
    fn legacy_task_defaults_to_unblocked() {
        let conn = open(Path::new(":memory:")).expect("db");
        conn.execute(
            "INSERT INTO orchestration_batches (id, repo_path) VALUES ('b1','/tmp/r')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO orchestration_tasks (id, batch_id, title, repo_path, branch)
             VALUES ('t1','b1','T','/tmp/r','furx/orch/b1/t-t1')",
            [],
        )
        .unwrap();
        let (blocked, run, idx): (i64, Option<String>, Option<i64>) = conn
            .query_row(
                "SELECT dag_blocked, pipeline_run_id, topo_index FROM orchestration_tasks WHERE id='t1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(blocked, 0, "tarea legacy debe arrancar dag_blocked=0 (lanzable)");
        assert!(run.is_none(), "pipeline_run_id default NULL");
        assert!(idx.is_none(), "topo_index default NULL");
    }

    /// 038 F1.0 — FK `ON DELETE CASCADE`: borrar una tarea elimina sus aristas (no quedan
    /// colgantes = dep insatisfecha eterna, red-team riesgo #7). Requiere foreign_keys=ON (ya lo
    /// setea `open`).
    #[test]
    fn deleting_task_cascades_edges() {
        let conn = open(Path::new(":memory:")).expect("db");
        conn.execute_batch(
            "INSERT INTO orchestration_batches (id, repo_path) VALUES ('b1','/tmp/r');
             INSERT INTO orchestration_tasks (id, batch_id, title, repo_path, branch)
               VALUES ('a','b1','A','/tmp/r','furx/orch/b1/a-a');
             INSERT INTO orchestration_tasks (id, batch_id, title, repo_path, branch)
               VALUES ('b','b1','B','/tmp/r','furx/orch/b1/b-b');
             INSERT INTO pipeline_runs (id, batch_id) VALUES ('run1','b1');
             INSERT INTO pipeline_edges (run_id, task_id, depends_on_task_id)
               VALUES ('run1','b','a');",
        )
        .unwrap();
        // Borrar 'a' (la dep) cascadea la arista (b depende de a).
        conn.execute("DELETE FROM orchestration_tasks WHERE id='a'", []).unwrap();
        let edges: i64 = conn
            .query_row("SELECT COUNT(*) FROM pipeline_edges", [], |r| r.get(0))
            .unwrap();
        assert_eq!(edges, 0, "borrar la dep debe cascadear la arista (sin colgantes)");
    }
}
