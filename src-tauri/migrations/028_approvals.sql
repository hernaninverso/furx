-- 015 US4 — Capability/Permission + Approval gate.
-- Estado `pending_approval` de PRIMERA CLASE: cuando un comando Destructive/Credential
-- (o con requires_confirmation) pasa por la puerta central (services::capability), en vez
-- de ejecutarse crea un `approval` pending y emite AppEvent::ApprovalRequested (US3 event bus).
-- El comando NO se ejecuta hasta que un humano lo aprueba (approval_resolve).
--
-- BYOK (constitución F-I): esta tabla NUNCA contiene secrets. `args_json` es la metadata
-- del comando solicitado (argumentos NO sensibles); las keys viven SÓLO en el Keychain y
-- las resuelve el backend al ejecutar (ver services::capability::secret provider). Si un
-- comando necesita una credencial, `args_json` lleva el *credential ref* (nombre del entry
-- del Keychain), nunca la key.
--
-- NOTA numeración: 027 la usa otra ventana del reform kernel; 028 es la próxima libre acá.

CREATE TABLE IF NOT EXISTS approvals (
  id           TEXT PRIMARY KEY NOT NULL,                    -- uuid del request de aprobación
  command_id   TEXT NOT NULL,                                -- id del comando del registry (command_registry)
  args_json    TEXT NOT NULL DEFAULT '{}',                   -- args NO sensibles del comando (incl. credential ref)
  status       TEXT NOT NULL DEFAULT 'pending'
                 CHECK (status IN ('pending', 'approved', 'rejected')),
  created_at   TEXT NOT NULL,                                -- ISO-8601 — cuándo se solicitó
  resolved_at  TEXT                                          -- ISO-8601 — cuándo se aprobó/rechazó (NULL si pending)
);

CREATE INDEX IF NOT EXISTS idx_approvals_status ON approvals(status);
CREATE INDEX IF NOT EXISTS idx_approvals_created ON approvals(created_at);
