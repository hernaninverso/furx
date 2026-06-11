-- spec-052 · Cost-Router Classifier v2 (bandit-ready) — schema.
--
-- Evoluciona el clasificador de 049 (router activo, OFF por flag) al diseño canónico del council:
-- score ponderado + bandit ε-greedy + circuit breaker + canary. TODO OFF detrás de
-- `FURX_COST_ROUTER_MODE` (default off ⇒ no-op). Estas tablas son ADITIVAS → cero regresión.
--
-- PRIVACIDAD (igual que 053): NINGUNA columna de texto libre. Solo ids, números, scores y outcomes
-- categóricos. NUNCA prompts/diffs/paths/secrets.
--
-- DIVERGENCIA vs council (NO bug, alcance): Furx-cliente es SQLite (rusqlite), no PostgreSQL. El
-- `bandit_state` es MUTABLE por diseño (EWMA se actualiza); NO es append-only (a diferencia de
-- `cost_router_events`/053 que sí lo es, porque ese es el audit de ahorro). `cost_router_decisions` y
-- `cost_router_outcomes` son estado operativo de routing, tampoco append-only.

-- ── Estado de instalación: installation_id estable (NO derivado de hardware ni boot_ts — council C3) ──
-- UUID v4 puro persistido. Sobrevive reboots (a diferencia de un hash con boot_ts que cambiaría cada
-- arranque y perdería el historial del circuit breaker / bandit). Si se corrompe ⇒ se genera uno nuevo
-- (pérdida de historial aceptable en V1, mejor que inestabilidad por reboot).
-- SINGLETON de verdad (audit-3 052): `singleton INTEGER PK CHECK(singleton=1)` ⇒ la tabla tiene a lo
-- sumo UNA fila. Dos `INSERT OR IGNORE (1, ...)` concurrentes ⇒ solo uno persiste (colisión de PK=1) ⇒
-- el installation_id queda ESTABLE aun bajo concurrencia (no dos filas con ids distintos).
CREATE TABLE IF NOT EXISTS cost_router_state (
    singleton       INTEGER PRIMARY KEY NOT NULL CHECK (singleton = 1),
    installation_id TEXT NOT NULL,                        -- UUID v4 (texto)
    created_at      TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
);

-- ── Estado del bandit ε-greedy por (installation, modelo) — council C5/C9 ──────────────────────────
-- EWMA de éxito y latencia con prior optimista (0.85 / 1500ms) para cold-start seguro: si las primeras
-- requests son a un free caído, el prior evita que success_ema→0 mate el bandit. MUTABLE.
CREATE TABLE IF NOT EXISTS bandit_state (
    installation_id          TEXT NOT NULL,
    model_id                 TEXT NOT NULL,
    real_success_ema         REAL NOT NULL DEFAULT 0.85,   -- prior optimista
    real_latency_ema         REAL NOT NULL DEFAULT 1500.0, -- ms
    exploration_success_ema  REAL NOT NULL DEFAULT 0.85,
    exploration_latency_ema  REAL NOT NULL DEFAULT 1500.0,
    n_real                   INTEGER NOT NULL DEFAULT 0,
    n_exploration            INTEGER NOT NULL DEFAULT 0,
    p99_window_ms            INTEGER NOT NULL DEFAULT 1500, -- cap de latencia p99 observada
    updated_at_ms            INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (installation_id, model_id)
);

-- ── Decisiones del router (decision_id generado por F2 — council C7) ───────────────────────────────
-- F2 genera `decision_id` ANTES de despachar; vincula el outcome sin depender de que F1 lo refleje.
-- `route` ∈ premium/local/free. `exploration`/`shadow` marcan las decisiones del canary (no alimentan
-- el bandit real). Estado operativo, no append-only (puede limpiarse por retención > 30 días).
CREATE TABLE IF NOT EXISTS cost_router_decisions (
    decision_id        TEXT PRIMARY KEY NOT NULL,         -- UUID v4 (texto), lo genera F2
    task_id            TEXT NOT NULL,
    classifier_version INTEGER NOT NULL DEFAULT 1,
    route              TEXT NOT NULL CHECK (route IN ('premium','local','free')),
    score              REAL,                              -- score continuo (NULL si hard gate)
    reason             TEXT,                              -- razón corta (gate o heurística), sin texto libre
    exploration        INTEGER NOT NULL DEFAULT 0,        -- 1 = decisión de exploración canary
    shadow             INTEGER NOT NULL DEFAULT 0,        -- 1 = shadow (ejecutó free, descartó resultado)
    ts_utc_ms          INTEGER NOT NULL
);

-- ── Outcomes (cierra el blocker del contrato F1→F2 — council C8/open#1) ────────────────────────────
-- El reward del bandit necesita success + latency, que F1 (053) NO emite. Acá se persiste el outcome
-- inferido (bool→F1Outcome por SLA) vinculado por decision_id. Gate: NUNCA outcome con decision_id NULL.
CREATE TABLE IF NOT EXISTS cost_router_outcomes (
    decision_id        TEXT PRIMARY KEY NOT NULL REFERENCES cost_router_decisions(decision_id),
    outcome            TEXT NOT NULL CHECK (outcome IN ('success','semantic_failure','system_failure','degraded')),
    latency_ms         INTEGER NOT NULL,
    is_inferred_outcome INTEGER NOT NULL DEFAULT 1,       -- 1 = derivado del puente bool→F1Outcome
    model_id           TEXT NOT NULL,
    ts_utc_ms          INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_crd_task     ON cost_router_decisions(task_id);
CREATE INDEX IF NOT EXISTS idx_crd_ts       ON cost_router_decisions(ts_utc_ms DESC);
CREATE INDEX IF NOT EXISTS idx_cro_ts       ON cost_router_outcomes(ts_utc_ms DESC);
CREATE INDEX IF NOT EXISTS idx_cro_model    ON cost_router_outcomes(model_id);
