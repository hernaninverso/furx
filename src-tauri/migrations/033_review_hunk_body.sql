-- 019 F0 (audit codex #1) — cuerpo del hunk en la review projection.
-- review_apply DEBE aplicar EXACTAMENTE lo que el usuario aprobó. Re-derivar el patch de los
-- worktrees vivos en apply-time es inseguro: si una variante cambia el cuerpo del mismo hunk (mismo
-- {file}:{old_start},{old_count} = mismo id content-based, distinto cuerpo) tras la revisión, se
-- aplicaría un cuerpo distinto al aprobado. Fix: snapshotear el CUERPO del hunk al abrir la review
-- y construir el patch desde ese snapshot (no desde el worktree vivo).
ALTER TABLE review_hunks ADD COLUMN body TEXT NOT NULL DEFAULT '';
