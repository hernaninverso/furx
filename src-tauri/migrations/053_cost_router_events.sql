-- spec-048 · Cost-Router Fase 1 (Savings Meter) — P4 schema.
--
-- MIDE el ahorro del routing que Furx YA hace (local Ollama / free AIE / premium BYOK). NO desvía
-- ninguna decisión (eso es Fase 2). Cada decisión de routing emite UNA fila append-only acá.
--
-- DIVERGENCIA vs council v6 (NO bug, alcance): el council asume PostgreSQL (ai_engine_db) con
-- `REVOKE UPDATE,DELETE,TRUNCATE`. Furx-cliente es SQLite (rusqlite) → append-only se hace con
-- triggers BEFORE UPDATE/DELETE → RAISE(ABORT), idéntico patrón a `events` (001) / `policy_rule_changes`
-- (044). SQLite no tiene roles ni TRUNCATE; el test CI (Rust) verifica el rechazo real de UPDATE/DELETE.
--
-- PRIVACIDAD (P3): esta tabla NO tiene columnas de texto libre. Solo guarda el tier, el id de modelo,
-- tokens (números) y costos (números). NUNCA prompts/diffs/paths/secrets → no hay superficie de PII.

-- ── Tabla de precios versionada (mutable: se actualizan tarifas, se versiona) ──────────────────
-- El baseline premium = tokens × precio del modelo premium. Versionado para que un cambio de tarifa
-- no reescriba ahorros pasados (la fila del evento guarda el `price_table_version` que usó).
CREATE TABLE IF NOT EXISTS price_table (
    provider             TEXT    NOT NULL,           -- ej "anthropic", "openai"
    model                TEXT    NOT NULL,           -- ej "claude-sonnet-4", "gpt-5"
    price_in_per_mtok    REAL    NOT NULL,           -- USD por millón de tokens de input
    price_out_per_mtok   REAL    NOT NULL,           -- USD por millón de tokens de output
    price_table_version  TEXT    NOT NULL,           -- ej "2026-06"
    is_default           INTEGER NOT NULL DEFAULT 0, -- 1 = fila default usada cuando no hay premium BYOK
    created_at           TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
    PRIMARY KEY (provider, model, price_table_version)
);

CREATE INDEX IF NOT EXISTS idx_price_default ON price_table(is_default) WHERE is_default = 1;

-- Seed: un default premium DOCUMENTADO (modelo premium común) para arrancar antes de que el user
-- configure su BYOK premium. Precios de referencia 2026-06 (USD/Mtok). Marcado is_default=1.
-- Si el user configura un premium, el baseline usa ESE; si no, este default + baseline_is_default=1.
INSERT OR IGNORE INTO price_table
    (provider, model, price_in_per_mtok, price_out_per_mtok, price_table_version, is_default)
VALUES
    ('anthropic', 'claude-sonnet-4', 3.0, 15.0, '2026-06', 1);

-- ── Eventos de ahorro (APPEND-ONLY) ────────────────────────────────────────────────────────────
CREATE TABLE IF NOT EXISTS cost_router_events (
    event_id                  TEXT    PRIMARY KEY NOT NULL, -- UUID v4 (texto), lo genera el backend
    schema_version            INTEGER NOT NULL DEFAULT 1,
    decision                  TEXT    NOT NULL
        CHECK (decision IN ('local','free','premium','blocked')),
    model_id                  TEXT,                          -- modelo realmente usado (NULL si N/A)
    provider                  TEXT,                          -- provider realmente usado
    tokens_in                 INTEGER,
    tokens_out                INTEGER,
    cost_real_usd             REAL,                          -- lo que costó de verdad (~0 para local/free)
    cost_baseline_premium_usd REAL,                          -- lo que habría costado todo en premium
    price_table_version       TEXT,                          -- versión usada para el baseline
    baseline_is_default       INTEGER NOT NULL DEFAULT 0,    -- 1 = baseline con precio default (sin BYOK premium)
    acceptance_proxy          INTEGER,                       -- NULLable, RESERVADO Fase 2 (NO se puebla en Fase 1)
    ts                        TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
);

CREATE INDEX IF NOT EXISTS idx_cre_ts       ON cost_router_events(ts DESC);
CREATE INDEX IF NOT EXISTS idx_cre_decision ON cost_router_events(decision);

-- Append-only: triggers que abortan toda mutación (mismo patrón que `events`/001 y 044).
CREATE TRIGGER IF NOT EXISTS cost_router_events_no_update
BEFORE UPDATE ON cost_router_events
BEGIN
    SELECT RAISE(ABORT, 'cost_router_events is append-only: UPDATE prohibido');
END;

CREATE TRIGGER IF NOT EXISTS cost_router_events_no_delete
BEFORE DELETE ON cost_router_events
BEGIN
    SELECT RAISE(ABORT, 'cost_router_events is append-only: DELETE prohibido');
END;
