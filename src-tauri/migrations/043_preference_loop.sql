-- 026 F0 · T004 — preference loop sobre best-of-N (auto-aprendizaje #2).
--
-- Cierra el loop: la elección humana auditada en `review_audit_links` (034) + el estado final de
-- `review_hunks` (032) se derivan en un REGISTRO DE PREFERENCIA estructurado (qué variante/hunks
-- ganaron, con las features objetivas de cada variante y el contexto repo/tarea). Ese registro
-- alimenta un PRIOR estadístico local explicable (`context_priors`) que enriquece el ranking
-- advisory de 020 — SIEMPRE advisory, SIEMPRE explicable, reseteable.
--
-- INVARIANTES:
--   - `preference_records` + `variant_features` son APPEND-ONLY (triggers BEFORE UPDATE/DELETE →
--     RAISE, igual que `034_review_audit_link.sql`): la señal es inmutable; correcciones = nuevos
--     registros. NO duplica el audit — lo DERIVA.
--   - `context_priors` SÍ es MUTABLE: el prior evoluciona (update bayesiano + decay por contexto).
--   - CERO código crudo de diffs en estas tablas — solo metadata/features + contexto scrubbeado
--     (FR-005/SC-008). Lo garantiza el scrubber de `preference_signal.rs`, no el schema.

-- ── La señal de preferencia (append-only) ──────────────────────────────────────
CREATE TABLE IF NOT EXISTS preference_records (
    id                     TEXT PRIMARY KEY,                 -- uuid del registro
    group_id               TEXT NOT NULL,                    -- == orch_task_groups.id / review_groups.group_id
    repo_key               TEXT NOT NULL,                    -- hash/relativo (NO ruta absoluta) — scrubbeado
    task_type              TEXT NOT NULL DEFAULT 'unknown',  -- bugfix|feature|refactor|… o 'unknown'
    outcome_kind           TEXT NOT NULL,                    -- single|mixed|none
    feature_schema_version INTEGER NOT NULL,                 -- versión del set de features (interpretabilidad)
    revision               INTEGER,                          -- revisión de la review al momento (NULL si N/A)
    created_at             TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_pref_records_group ON preference_records(group_id);
CREATE INDEX IF NOT EXISTS idx_pref_records_ctx   ON preference_records(repo_key, task_type);

-- ── Features objetivas por variante dentro de un record (append-only) ──────────
-- Un registro de preferencia tiene N filas (una por variante × feature). `chosen` marca si la
-- variante fue elegida (1) o rechazada (0); `measured` distingue "no medido" (0) de "0 issues" (1).
CREATE TABLE IF NOT EXISTS variant_features (
    record_id        TEXT NOT NULL,                          -- FK lógico a preference_records.id
    task_id          TEXT NOT NULL,                          -- == orchestration_tasks.id (la variante)
    chosen           INTEGER NOT NULL DEFAULT 0,             -- 1 si la variante fue elegida (parcial o total)
    agent_profile_id TEXT,                                   -- agente/modelo que la generó (NULL si N/A)
    feature_key      TEXT NOT NULL,                          -- diff_added|diff_removed|files_touched|risky_paths|qg_errors|qg_warnings|…
    value            REAL NOT NULL DEFAULT 0,                -- valor normalizado/crudo del feature
    measured         INTEGER NOT NULL DEFAULT 1,             -- 1 medido, 0 AUSENTE (≠ 0 — contrato fail-safe 024)
    PRIMARY KEY (record_id, task_id, feature_key)
);

CREATE INDEX IF NOT EXISTS idx_variant_features_record ON variant_features(record_id);

-- ── El prior aprendido por contexto (MUTABLE — evoluciona) ─────────────────────
-- Modelo Beta por feature: (alpha, beta) acumulan evidencia elegida/rechazada con decay. El peso
-- = 2*(alpha/(alpha+beta) - 0.5). `sample_count` por contexto gobierna el cold-start (≥15).
CREATE TABLE IF NOT EXISTS context_priors (
    repo_key     TEXT NOT NULL,
    task_type    TEXT NOT NULL,
    feature_key  TEXT NOT NULL,
    alpha        REAL NOT NULL DEFAULT 1.0,                  -- Beta(1,1) = uniforme (neutro)
    beta         REAL NOT NULL DEFAULT 1.0,
    distinct_obs INTEGER NOT NULL DEFAULT 0,                 -- nº de valores distintos observados (diversidad)
    updated_at   TEXT NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (repo_key, task_type, feature_key)
);

-- `sample_count` por contexto (independiente del nº de features). Tabla separada para no repetir.
CREATE TABLE IF NOT EXISTS context_prior_meta (
    repo_key     TEXT NOT NULL,
    task_type    TEXT NOT NULL,
    sample_count INTEGER NOT NULL DEFAULT 0,
    updated_at   TEXT NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (repo_key, task_type)
);

-- ── Append-only en la señal (la inmutabilidad la pide FR-003/SC-001, patrón 034) ──
CREATE TRIGGER IF NOT EXISTS preference_records_no_update
BEFORE UPDATE ON preference_records
BEGIN
    SELECT RAISE(ABORT, 'preference_records is append-only');
END;

CREATE TRIGGER IF NOT EXISTS preference_records_no_delete
BEFORE DELETE ON preference_records
BEGIN
    SELECT RAISE(ABORT, 'preference_records is append-only');
END;

CREATE TRIGGER IF NOT EXISTS variant_features_no_update
BEFORE UPDATE ON variant_features
BEGIN
    SELECT RAISE(ABORT, 'variant_features is append-only');
END;

CREATE TRIGGER IF NOT EXISTS variant_features_no_delete
BEFORE DELETE ON variant_features
BEGIN
    SELECT RAISE(ABORT, 'variant_features is append-only');
END;
