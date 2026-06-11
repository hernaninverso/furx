-- 019 (audit codex ronda 2) — FENCING TOKEN para el kill-switch transaccional.
--
-- Problema (sin token): el kill usa un estado intermedio `killing` + stale-reclaim a 300s, pero la
-- fila no identificaba al DUEÑO del claim. Consecuencias:
--   - Tras 300s, un 2º killer podía re-clamar (`killing` viejo → nuevo `killing`) mientras el 1º
--     seguía corriendo git → 2 git concurrentes sobre el mismo worktree.
--   - `mark_killed`/`release_killing` actualizaban por (task_id, status='killing') SIN verificar
--     "soy el dueño del claim actual" → un killer viejo podía marcar `killed` durante el git del
--     nuevo, o liberar a `open` el claim del nuevo.
--
-- Fix: una columna `kill_token` que se genera ÚNICA en cada transición a `killing` (incluido el
-- stale-reclaim, que sobrescribe el token viejo). `mark_killed`/`release_killing` exigen
-- `kill_token = <token-del-claim>` en su WHERE; si afectan 0 filas, el claim ya no es nuestro (otro
-- killer lo reclamó tras el stale) y NO reportamos éxito.
--
-- Idempotente: `ADD COLUMN` con default NULL. Funciona haya o no quedado aplicada la 035 en algún
-- dev DB; los checkpoints `open`/`killed` preexistentes tienen `kill_token = NULL` (no afecta su
-- semántica — el token sólo es relevante mientras hay un `killing` in-flight).

ALTER TABLE attempt_checkpoints ADD COLUMN kill_token TEXT;
