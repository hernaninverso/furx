-- 045 FR-002 (Ola 5 P1) — overrides de MCP servers por el usuario.
--
-- Hoy los MCP servers se editan a mano en ~/.claude.json. Esta tabla guarda el toggle
-- enabled/disabled del usuario SIN tocar el JSON: la DB es la fuente de verdad en RUNTIME.
-- Bootstrap: leer ~/.claude.json (lista canónica de servers) → aplicar override de la DB.
-- Un server presente en ~/.claude.json sin fila en esta tabla = habilitado por default.
--
-- `mcp_set_enabled(name, enabled)` valida que `name` exista en ~/.claude.json antes de
-- insertar (no se aceptan nombres inventados). Migración ADITIVA (sin seed).
CREATE TABLE IF NOT EXISTS mcp_user_overrides (
    name       TEXT PRIMARY KEY,
    enabled    INTEGER NOT NULL DEFAULT 1 CHECK(enabled IN (0,1)),
    source     TEXT NOT NULL DEFAULT 'user' CHECK(source IN ('user','discovery')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);
