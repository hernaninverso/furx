-- spec-049 · Cost-Router Fase 2 (Router ACTIVO) — P4 schema (quota por tenant).
--
-- El router activo (OFF detrás del flag `FURX_COST_ROUTER_MODE`) controla un presupuesto de tokens
-- FREE por tenant/mes. Si el tenant superó su budget free ⇒ el router cae a premium. El consumo usa
-- un lock NO-BLOQUEANTE (council §F1.1 "SKIP LOCKED"): en SQLite se traduce a una transacción
-- IMMEDIATE con busy_timeout=0 → si la DB está ocupada (SQLITE_BUSY), retorna inmediatamente y el
-- router cae a premium en <1ms (nunca encola 500ms). El mecanismo está en `cost_router.rs`.
--
-- DIVERGENCIA vs council v6 (NO bug, alcance): el council asume PostgreSQL con
-- `SELECT ... FOR UPDATE SKIP LOCKED`. Furx-cliente es SQLite (un proceso, Mutex<Connection>). El
-- equivalente semántico es el try-lock no-bloqueante (documentado en specs/049/analysis).
--
-- PRIVACIDAD: esta tabla NO guarda texto libre. Solo tenant_id (id opaco), budget/usado (números) y
-- el período. Sin superficie de PII.

CREATE TABLE IF NOT EXISTS router_quotas (
    tenant_id                TEXT    PRIMARY KEY NOT NULL, -- id opaco del tenant (Team/Enterprise)
    -- Presupuesto de tokens FREE por período (mes). 0 = sin budget free ⇒ siempre premium.
    batch_budget_tokens      INTEGER NOT NULL DEFAULT 0,
    -- Tokens free ya consumidos en el período actual.
    used_batch_tokens        INTEGER NOT NULL DEFAULT 0
        CHECK (used_batch_tokens >= 0),
    -- Período (mes) al que aplica `used_batch_tokens`, formato 'YYYY-MM'. Cuando cambia el mes, el
    -- consumo se resetea (lo maneja `try_consume_quota` comparando el período actual).
    period                   TEXT    NOT NULL DEFAULT (strftime('%Y-%m','now')),
    updated_at               TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
);

CREATE INDEX IF NOT EXISTS idx_router_quotas_period ON router_quotas(period);

-- ── Enriquecer las trazas de Fase 1 con columnas de Fase 2 ───────────────────────────────────────
-- La tabla `cost_router_events` (migr 053, Fase 1) reservó `acceptance_proxy`. La Fase 2 agrega tres
-- columnas que el router activo puebla (y que el KPI gate agrega): cuántas veces se rerouteó la tarea
-- (invariante ≤1), si se redactó algún secreto antes de un tier free, y si la sesión era interactiva.
-- Con default / NULLable → ADITIVAS, no rompen las filas de Fase 1 ni el append-only (no son UPDATE,
-- son columnas nuevas). SQLite ADD COLUMN es seguro sobre una tabla con triggers BEFORE UPDATE/DELETE.
ALTER TABLE cost_router_events ADD COLUMN reroute_count INTEGER NOT NULL DEFAULT 0;
ALTER TABLE cost_router_events ADD COLUMN security_redacted INTEGER NOT NULL DEFAULT 0;
ALTER TABLE cost_router_events ADD COLUMN session_interactive INTEGER;
