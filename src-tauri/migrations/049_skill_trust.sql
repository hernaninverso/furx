-- spec-kit 043 · Ola 4 F2 — Skills híbrido: registry de confianza local.
--
-- Coordinación de migración: la Ola 1 (041 multi-usuario) reserva el 048
-- (`04X_defaults_patch.sql`). Esta Ola 4 usa el 049. rusqlite_migration aplica por
-- POSICIÓN en el vec de db.rs, no por número de archivo — el número es cosmético, pero
-- mantenerlo monótono evita confusión en el merge. Al mergear ambas olas, el orquestador
-- intercala 048 (Ola 1) antes de 049 (esta) en el vec.
--
-- ADITIVA sobre la tabla `plugins` (creada en 010, UNIQUE(name) en 039). NO toca filas
-- existentes salvo backfill de defaults. Las columnas nuevas son NULL/default para todo
-- plugin legacy → cero regresión (un plugin sin estos campos sigue funcionando igual).

-- trust_level: estado del trust gate (043 §3). NULL para plugins legacy (los maneja la
-- ruta `plugins.rs::list` por firma Ed25519 del SignedManifest, sin cambios). Valores:
--   'verified' | 'promoted' | 'sandboxed' | 'rejected'
ALTER TABLE plugins ADD COLUMN trust_level TEXT;

-- inert: 1 = los scripts NO ejecutan (sandboxed/rejected, o pendiente). Default 0 para
-- no romper el comportamiento legacy (enabled/verified ya gobierna el resto).
ALTER TABLE plugins ADD COLUMN inert INTEGER NOT NULL DEFAULT 0;

-- pending_verification: 1 = la instalación está a mitad de camino (post-INSERT, pre-rename
-- o pre-UPDATE final). El recovery al startup re-verifica estas filas. Default 0.
ALTER TABLE plugins ADD COLUMN pending_verification INTEGER NOT NULL DEFAULT 0;

-- staging_path: nombre del dir `.tmp_<uuid>` dentro de plugins_base mientras la
-- instalación está pendiente. NULL una vez publicado (rename completado). El recovery lo
-- usa para reanudar/limpiar instalaciones interrumpidas por crash.
ALTER TABLE plugins ADD COLUMN staging_path TEXT;

-- last_verified_at: RFC3339 del último re-verify exitoso del tree_hash. Base del
-- fast-path P1 (en P0 se rehashea siempre). NULL = nunca verificado.
ALTER TABLE plugins ADD COLUMN last_verified_at TEXT;

-- status_message: razón legible del último cambio de estado (ej "tree_hash mismatch:
-- expected X got Y", "recovery_failed"). Se muestra en la UI inline. NULL = sin nota.
ALTER TABLE plugins ADD COLUMN status_message TEXT;

-- tree_hash: el SHA-256 canónico (NFC) del contenido firmado, copiado del manifest al
-- instalar. El recovery/re-verify compara el hash de disco contra este. NULL = legacy.
ALTER TABLE plugins ADD COLUMN tree_hash TEXT;

-- Índice para el recovery: barre filas pending al startup sin full-scan.
CREATE INDEX IF NOT EXISTS idx_plugins_pending ON plugins(pending_verification)
  WHERE pending_verification = 1;
