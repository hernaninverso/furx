-- Seed default Grafana endpoint pointing at the local SSH tunnel installed
-- by ~/Library/LaunchAgents/cloud.furx.desktop.grafana-tunnel.plist (ssh -L
-- 127.0.0.1:13000 -> the dev server 127.0.0.1:3000). User can override in Settings.
INSERT OR IGNORE INTO settings (key, value) VALUES
    ('endpoints.grafana', '"http://127.0.0.1:13000/d/scanner/ops?kiosk"'),
    ('endpoints.telegram_relay', '""');
