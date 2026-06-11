-- 045 FR-001 (Ola 5 P1) — monitores configurables por el usuario.
--
-- Antes los monitores estaban HARDCODEADOS en `monitors::default_targets()` (hosts de infra del
-- autor, sin UI para agregar/quitar). Esta tabla los hace configurables: el daemon poller LEE de
-- acá (no del hardcode). Seed inicial = SOLO localhost/mac-local (cualquier host de infra se SACA
-- del default; el usuario los agrega por UI). Migración ADITIVA: ON CONFLICT DO NOTHING (idempotente).
--
-- Caps por tier (Free 5 / Pro 50 / Team ∞) se aplican en `monitor_add` (backend), no en SQL.
-- `tier_min` permite marcar un target como exclusivo de un tier (default 'free' = visible a todos).
CREATE TABLE IF NOT EXISTS monitor_targets (
    id           TEXT PRIMARY KEY,
    label        TEXT NOT NULL,
    kind         TEXT NOT NULL CHECK(kind IN ('tcp','http')),
    addr         TEXT NOT NULL,
    interval_s   INTEGER NOT NULL DEFAULT 30 CHECK(interval_s >= 5),
    tier_min     TEXT NOT NULL DEFAULT 'free' CHECK(tier_min IN ('free','pro','team','enterprise')),
    enabled      INTEGER NOT NULL DEFAULT 1 CHECK(enabled IN (0,1)),
    created_at   TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE(kind, addr)
);

-- Seed default: SOLO la Mac local (127.0.0.1). Ningún host de infra del autor entra al seed —
-- el usuario agrega los suyos por UI. Idempotente.
INSERT INTO monitor_targets (id, label, kind, addr, interval_s, tier_min, enabled)
VALUES ('mac-local', 'Mac (local)', 'tcp', '127.0.0.1:22', 30, 'free', 1)
ON CONFLICT(kind, addr) DO NOTHING;
