-- 015 T015 — enforcement UNIVERSAL del gate US4 (approve→execute, "consume, sin replay").
--
-- El interceptor del dispatch (invoke_handler wrapper, lib.rs) corta TODO comando
-- Destructive/Credential/requires_confirmation venga de donde venga (palette, botón, plugin,
-- móvil, deeplink). El 1er invoke crea un `approval` pending y rechaza; tras la aprobación
-- humana, el front RE-invoca el MISMO comando y el interceptor CONSUME el approval aprobado
-- (single-use) antes de delegar al comando real.
--
-- Dos columnas nuevas para ese protocolo:
--   args_hash    : hash canónico (sha256 hex) de los args del comando. El approval queda atado
--                  a (command_id, args_hash) → no se puede aprobar con args benignos y ejecutar
--                  con args peligrosos (bait-and-switch). NULL en filas legacy (pre-030).
--   consumed_at  : ISO-8601 de cuándo el approval aprobado fue CONSUMIDO por una ejecución real.
--                  NON-NULL = ya usado → no se puede re-consumir (sin replay). El consumo es
--                  atómico (UPDATE ... WHERE consumed_at IS NULL) → exactamente un ganador.
--
-- Un approval es CONSUMIBLE si: status='approved' AND consumed_at IS NULL AND no expiró
-- (TTL desde created_at, ver services::capability::APPROVAL_TTL_SECS). El TTL evita que un
-- approval viejo autorice una ejecución horas después.
ALTER TABLE approvals ADD COLUMN args_hash TEXT;
ALTER TABLE approvals ADD COLUMN consumed_at TEXT;

CREATE INDEX IF NOT EXISTS idx_approvals_consumable
  ON approvals(command_id, args_hash, status, consumed_at);
