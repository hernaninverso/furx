-- 033 U3 — cola de atención persistente: descartar = "no molestar hasta nueva actividad".
-- Al `attention_ack(seq)` se registra (pane_id, dismissed_at). El poller, al re-evaluar un pane,
-- saltea encolarlo si `dismissed_at >= task.updated_at` (no hubo actividad nueva desde el descarte);
-- si el task tuvo actividad nueva (`updated_at > dismissed_at`), reaparece y se borra el descarte.
-- `dismissed_at` se escribe en RFC3339 UTC; la comparación con `updated_at` se hace PARSEANDO ambos a
-- instantes (offset-aware, NO lexicográfico) en `attention::is_dismissed`.
CREATE TABLE IF NOT EXISTS attention_dismissed (
    pane_id      TEXT PRIMARY KEY,
    dismissed_at TEXT NOT NULL
);
