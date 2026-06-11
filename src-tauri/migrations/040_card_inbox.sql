-- spec-022 P1 · US6 — incidents inbox accionable.
--
-- Extiende `cards` para un inbox tipo Linear/GitHub: snooze con duración explícita,
-- mark-read, dismiss, y auto-unsnooze ante nueva actividad de la fuente. Todo
-- non-destructivo: ALTERs aislados (cada uno falla solo si la columna ya existe) y
-- defaults que preservan el comportamiento existente de las filas legacy.
--
-- Modelo del inbox (derivado, NO toca el CHECK status IN ('open','closed')):
--   - `snooze_until`     : timestamp ISO UTC hasta el que la card queda oculta. NULL = no snoozeada.
--   - `read_at`          : marcada leída (mark-read). NULL = no leída.
--   - `dismissed_at`     : descartada del inbox sin decisión (dismiss). NULL = no descartada.
--   - `last_activity_at` : última actividad conocida de la FUENTE (para auto-unsnooze). Default =
--                          created_at en filas legacy; el backend lo refresca al re-observar la causa.
--   - `reopened`         : 1 si la card fue auto-reabierta por nueva actividad mientras estaba
--                          snoozeada (badge "Reabierto" en la UI). 0 = normal.
--
-- Una card es "accionable en el inbox" si: status='open' AND dismissed_at IS NULL AND
-- (snooze_until IS NULL OR snooze_until <= now OR reopened=1).

ALTER TABLE cards ADD COLUMN snooze_until TEXT;
ALTER TABLE cards ADD COLUMN read_at TEXT;
ALTER TABLE cards ADD COLUMN dismissed_at TEXT;
ALTER TABLE cards ADD COLUMN last_activity_at TEXT;
ALTER TABLE cards ADD COLUMN reopened INTEGER NOT NULL DEFAULT 0;

-- Backfill last_activity_at = created_at para que el auto-unsnooze tenga una línea base
-- (sin esto, una card snoozeada legacy nunca podría detectar "nueva actividad").
UPDATE cards SET last_activity_at = created_at WHERE last_activity_at IS NULL;

-- Índice para el barrido de inbox (open + no descartadas, ordenadas por recencia).
CREATE INDEX IF NOT EXISTS idx_cards_inbox
    ON cards(status, dismissed_at, snooze_until, created_at DESC);
