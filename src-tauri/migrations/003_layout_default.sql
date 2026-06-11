-- Seed default layout (sobrescribible desde la app).
INSERT OR IGNORE INTO layouts (id, name, panes, grid_cols, grid_rows) VALUES (
    'default',
    'Default 2×2',
    '[{"id":"p1","mode":"claude-A","title":"Pane 1"},{"id":"p2","mode":"claude-B","title":"Pane 2"},{"id":"p3","mode":"codex","title":"Pane 3"},{"id":"p4","mode":"zsh","title":"Pane 4"}]',
    '1fr 1fr',
    '1fr 1fr'
);
