-- 010-furx-signals — eventos + notificaciones multi-canal + control remoto.
-- Council 2026-05-29: dispatcher local persistente (worker tokio) con N sinks opt-in,
-- reusando mobile_bridge / telegram / tauri-plugin-notification / allowlist / audit.
-- BYOK: tokens SÓLO en Keychain; acá sólo config no-secreta (toggles, allowlist, filtros).

-- Eventos emitidos por los productores (008 transiciones, agent input, council).
-- Inmutables salvo `dispatched_at` (marca que el router ya generó sus deliveries).
CREATE TABLE IF NOT EXISTS signal_events (
    id            TEXT PRIMARY KEY,
    project_key   TEXT,                            -- 007 ownership (NULL = global)
    task_id       TEXT,                            -- orchestration_tasks.id si aplica
    agent_id      TEXT,
    type          TEXT NOT NULL,                   -- task.done | task.failed | task.awaiting_review | agent.input_requested | council.ready
    severity      TEXT NOT NULL DEFAULT 'info'
                    CHECK (severity IN ('info','warning','critical')),
    title         TEXT NOT NULL DEFAULT '',
    body          TEXT NOT NULL DEFAULT '',
    payload       TEXT,                            -- JSON extra (priority/tags/actions estilo ntfy)
    created_at    TEXT NOT NULL DEFAULT (datetime('now')),
    expires_at    TEXT,
    dispatched_at TEXT                             -- NULL = el router aún no lo procesó
);

CREATE INDEX IF NOT EXISTS idx_signal_events_undispatched
    ON signal_events(created_at) WHERE dispatched_at IS NULL;
CREATE INDEX IF NOT EXISTS idx_signal_events_type ON signal_events(type);

-- Una fila por (evento, canal). Idempotencia por la PK compuesta.
CREATE TABLE IF NOT EXISTS signal_deliveries (
    event_id      TEXT NOT NULL,
    channel       TEXT NOT NULL,                   -- desktop | mobile | telegram | webhook
    status        TEXT NOT NULL DEFAULT 'pending'
                    CHECK (status IN ('pending','sent','failed','skipped')),
    attempts      INTEGER NOT NULL DEFAULT 0,
    last_error    TEXT,
    next_retry_at TEXT,                            -- backoff: no reintentar antes de esto
    created_at    TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at    TEXT NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (event_id, channel),
    FOREIGN KEY (event_id) REFERENCES signal_events(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_signal_deliveries_status ON signal_deliveries(status);

-- Filtros opt-in por (tipo de evento, canal). Si no hay fila => default del canal.
CREATE TABLE IF NOT EXISTS signal_subscriptions (
    event_type    TEXT NOT NULL,                   -- '*' = cualquier tipo
    channel       TEXT NOT NULL,
    enabled       INTEGER NOT NULL DEFAULT 1,
    min_severity  TEXT NOT NULL DEFAULT 'info'
                    CHECK (min_severity IN ('info','warning','critical')),
    created_at    TEXT NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (event_type, channel)
);

-- Allowlist de chat_ids para control remoto por Telegram. NO secreta (el token va al Keychain).
-- paired_via: 'pair' (challenge /pair) | 'manual' (Settings).
CREATE TABLE IF NOT EXISTS signal_remote_allowlist (
    chat_id    TEXT PRIMARY KEY,
    label      TEXT,
    paired_via TEXT NOT NULL DEFAULT 'pair',
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

-- Códigos de pairing de un solo uso (challenge local). Se consumen en /pair <code>.
CREATE TABLE IF NOT EXISTS signal_pair_codes (
    code       TEXT PRIMARY KEY,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    expires_at TEXT NOT NULL,
    used_at    TEXT,                               -- NULL = aún válido
    used_by    TEXT                                -- chat_id que lo consumió
);
