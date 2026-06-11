-- F2 + F21 — cards pueden referenciar un bundle de contexto guardado en ~/.furx/contexts/.
-- ALTER TABLE separado para que falle aislado si la columna ya existe (downgrade-friendly).

ALTER TABLE cards ADD COLUMN context_bundle_path TEXT;
