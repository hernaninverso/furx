-- 015-frontend-reform-kernel · US5 — Headless Process / Task Lifecycle Manager.
--
-- Registro CENTRAL de procesos/jobs que viven en el BACKEND y SOBREVIVEN a
-- unmount/reload/cierre de ventana de la UI. El proceso es PROPIEDAD del backend;
-- la UI es un viewport que lo observa/controla. Cerrar una ventana / desmontar un
-- pane NO mata el proceso: sólo cierra la vista (la fila queda en `running`).
--
-- Tabla NUEVA y aditiva: NO toca `panes`/`orchestration_tasks`/`bg_jobs`. Coexiste
-- con orchestration (008/014, nivel TAREA) — este es el nivel PROCESO/ownership.
-- El PtyManager existente (src/pty.rs) sigue siendo el dueño del PTY real; este
-- registro es la capa de OWNERSHIP/LIFECYCLE por encima (registra qué proceso vive,
-- de quién es el contexto, y su estado persistido para reattach tras un reload).
--
--   kind          : 'pty' | 'job' | 'agent'  — clase de proceso.
--   owner_context : contexto de origen (window_id/pane_id/task_id) SÓLO informativo.
--                   La muerte de ese contexto (cerrar ventana) NO cancela el proceso.
--   status        : 'running' | 'done' | 'failed' | 'canceled'.
--   progress      : 0.0..1.0 (REAL), opcional.
--   external_ref  : pane_id del PtyManager / task_id de orchestration / job_id, para
--                   reconciliar el registro con el recurso real al cancelar/reatachar.
CREATE TABLE IF NOT EXISTS process_registry (
    process_id    TEXT PRIMARY KEY,
    kind          TEXT NOT NULL,
    owner_context TEXT,
    external_ref  TEXT,
    status        TEXT NOT NULL DEFAULT 'running',
    progress      REAL,
    label         TEXT,
    started_at    TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at    TEXT NOT NULL DEFAULT (datetime('now'))
);

-- Reattach/list por estado y por contexto de origen.
CREATE INDEX IF NOT EXISTS idx_process_registry_status ON process_registry(status);
CREATE INDEX IF NOT EXISTS idx_process_registry_owner ON process_registry(owner_context);
