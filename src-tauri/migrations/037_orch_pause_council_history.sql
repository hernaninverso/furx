-- 019 F3 (T030/T031) — corazones a producción.
-- T030 orchestration: pause/resume transaccional de un attempt (flag persistido, el PTY se
--   SIGSTOP/SIGCONT aparte; el flag es el SSOT de "está pausado", reversible).
-- T031 council: history (persistir cada run) + custom-voices (voces pinneadas del user —
--   config, NUNCA un paywall: el council es free para todos los tiers, constitución F-II).
-- SQLite no soporta ADD COLUMN IF NOT EXISTS; rusqlite_migration garantiza idempotencia por
-- user_version (esta migración NO se re-corre).

-- ── T030 pause/resume por attempt ────────────────────────────────────────────
-- paused_at NULL = corriendo normal; NON-NULL = pausado (el PTY recibió SIGSTOP). El kill y el
-- merge siguen funcionando sobre una tarea pausada (resume implícito al matar). El poller (012)
-- y el auto-confirm respetan el flag (no auto-presionan Enter sobre un proceso detenido).
ALTER TABLE orchestration_tasks ADD COLUMN paused_at TEXT;

-- ── T031 council history ─────────────────────────────────────────────────────
-- Cada council run queda registrado (consulta + UI). prompt/synth pueden ser grandes; se
-- guardan tal cual (ya redactados por el redactor de secrets antes de persistir — F-I BYOK).
-- voices_json = resumen NO-secreto de las voces ({provider, model, ok, latency_ms} por voz).
CREATE TABLE IF NOT EXISTS council_runs (
    id                TEXT PRIMARY KEY,
    ran_at            TEXT NOT NULL DEFAULT (datetime('now')),
    preset            TEXT NOT NULL DEFAULT 'mix',
    template          TEXT,                          -- council template aplicado (NULL = ninguno)
    prompt            TEXT NOT NULL DEFAULT '',       -- redactado antes de persistir
    synth             TEXT NOT NULL DEFAULT '',       -- síntesis redactada
    voices_attempted  INTEGER NOT NULL DEFAULT 0,
    voices_succeeded  INTEGER NOT NULL DEFAULT 0,
    elapsed_ms        INTEGER NOT NULL DEFAULT 0,
    voices_json       TEXT NOT NULL DEFAULT '[]'      -- [{provider, model, ok, latency_ms}]
);
CREATE INDEX IF NOT EXISTS idx_council_runs_ran_at ON council_runs(ran_at);

-- ── T031 council custom-voices ───────────────────────────────────────────────
-- Voces pinneadas por el user: SIEMPRE participan del council, por encima del preset/template
-- (no son un tier-gate — F-II council es free para todos). provider_alias referencia una
-- credencial ya conectada (Furx Connect); NUNCA guarda la key (vive en Keychain, BYOK). model
-- opcional (NULL = el default_ping_model del provider). enabled = on/off sin borrar.
CREATE TABLE IF NOT EXISTS council_custom_voices (
    id              TEXT PRIMARY KEY,
    provider_alias  TEXT NOT NULL,                    -- alias de una ProviderCredential conectada
    model           TEXT,                             -- modelo concreto (NULL = default del provider)
    enabled         INTEGER NOT NULL DEFAULT 1,        -- 0/1
    created_at      TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_council_custom_voice_uniq
    ON council_custom_voices(provider_alias, COALESCE(model, ''));
