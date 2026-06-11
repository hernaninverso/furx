-- 006 ext — presets de rol + motor (engine_kind). Council 2026-05-29:
-- preset/rol = AgentProfile built-in (sin capa nueva); engine_kind separa el motor.
-- MVP: engine_kind='cli' (panes = CLIs). engine_kind='aie' (REPL HTTP) = spec aparte.

ALTER TABLE agent_profiles ADD COLUMN engine_kind TEXT NOT NULL DEFAULT 'cli'; -- 'cli' | 'aie' (aie diferido)
ALTER TABLE agent_profiles ADD COLUMN category TEXT;                            -- 'soporte' | 'ventas' | 'qa' | ... (agrupar presets)
