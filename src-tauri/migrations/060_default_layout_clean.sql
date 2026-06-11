-- 060 — first-run UX: el seed legacy de 003 ('Default 2×2' con claude-A/claude-B/codex)
-- auto-spawneaba CLIs de agentes en un install NUEVO sin cuentas configuradas → la primera
-- impresión eran 4 panes tirando errores de Keychain/cuentas (y consumiendo quota si algún
-- CLI sí estaba logueado). El default pasa a un único pane zsh; el EmptyShellState guía la
-- creación de panes de agentes. Solo pisa el default INTACTO de 003 — si el usuario ya
-- customizó su layout 'default', no se toca.
UPDATE layouts SET
  name = 'Single pane',
  panes = '[{"id":"p1","mode":"zsh","title":"Pane 1"}]',
  grid_cols = '1fr',
  grid_rows = '1fr'
WHERE id = 'default'
  AND panes = '[{"id":"p1","mode":"claude-A","title":"Pane 1"},{"id":"p2","mode":"claude-B","title":"Pane 2"},{"id":"p3","mode":"codex","title":"Pane 3"},{"id":"p4","mode":"zsh","title":"Pane 4"}]'
  AND name = 'Default 2×2'
  AND grid_cols = '1fr 1fr'
  AND grid_rows = '1fr 1fr';
