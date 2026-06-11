-- spec-050 · Ola 8 P2 (FR-003) — Reliability board (métricas de calidad por agente/modelo).
--
-- OBSERVACIONAL. Cada corrida de agente / inferencia que se quiera medir emite UNA fila append-only
-- acá. Read-only board: agrega tasa de éxito / latencia / costo por agente y por modelo. DISTINTO del
-- cost-router (053 cost_router_events = AHORRO $); este es CALIDAD/RELIABILITY. Tablas separadas para
-- no acoplar dos features que evolucionan en paralelo (cost-router F2 toca 053; esto no lo toca).
--
-- Coordinación de migración (spec 050 Notas): 053 = cost-router F1; 054 reservado a cost-router F2
-- (mergea aparte); esta Ola 8 P2 usa 055. rusqlite_migration aplica por POSICIÓN en el vec de db.rs;
-- el número es cosmético pero monótono evita confusión en el merge.
--
-- PRIVACIDAD (mismo invariante que 053): SIN columnas de texto libre — solo agent_kind (enum corto),
-- model/provider (ids), success (0/1), latency_ms (número), cost (número). NUNCA prompts/diffs/paths.
--
-- OPT-IN: el recorder solo persiste si el setting `reliability.board_enabled` está ON (default OFF →
-- cero regresión, la tabla queda vacía hasta que el usuario active el board).

CREATE TABLE IF NOT EXISTS reliability_events (
    event_id        TEXT    PRIMARY KEY NOT NULL,  -- UUID v4 (texto), lo genera el backend
    schema_version  INTEGER NOT NULL DEFAULT 1,
    agent_kind      TEXT    NOT NULL DEFAULT 'unknown', -- claude|codex|gemini|aider|council|generic|...
    model           TEXT,                          -- id del modelo usado (NULL si N/A)
    provider        TEXT,                          -- provider del modelo (NULL si N/A)
    success         INTEGER NOT NULL DEFAULT 0      -- 1 = la corrida terminó OK (verdict Success/done)
        CHECK (success IN (0, 1)),
    latency_ms      INTEGER,                        -- duración medida (NULL si no medible)
    cost_usd        REAL,                           -- costo real (~0 local/free; NULL si desconocido)
    ts              TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
);

CREATE INDEX IF NOT EXISTS idx_rel_ts         ON reliability_events(ts DESC);
CREATE INDEX IF NOT EXISTS idx_rel_agent_ts   ON reliability_events(agent_kind, ts DESC);
CREATE INDEX IF NOT EXISTS idx_rel_model_ts   ON reliability_events(model, ts DESC);

-- Append-only: triggers que abortan toda mutación (mismo patrón que `events`/001 y 053).
CREATE TRIGGER IF NOT EXISTS reliability_events_no_update
BEFORE UPDATE ON reliability_events
BEGIN
    SELECT RAISE(ABORT, 'reliability_events is append-only: UPDATE prohibido');
END;

CREATE TRIGGER IF NOT EXISTS reliability_events_no_delete
BEFORE DELETE ON reliability_events
BEGIN
    SELECT RAISE(ABORT, 'reliability_events is append-only: DELETE prohibido');
END;
