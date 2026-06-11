-- 015-frontend-reform-kernel · US6 — Layout config versionada + multi-window-ready.
--
-- Tabla NUEVA y aditiva: NO toca `panes`/`layouts` (el `get_layout`/`save_layout`
-- legacy sigue funcionando en paralelo). Una fila por workspace guarda el árbol de
-- layout versionado (LayoutConfigV1) serializado en `json`. La noción de
-- window_key/monitor vive DENTRO del json (display HINTS, no monitor-IDs absolutos),
-- así un solo registro describe N ventanas aunque la UI use una sola.
CREATE TABLE IF NOT EXISTS layout_config (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    workspace_id TEXT NOT NULL UNIQUE,
    version INTEGER NOT NULL,
    json TEXT NOT NULL,
    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_layout_config_workspace ON layout_config(workspace_id);
