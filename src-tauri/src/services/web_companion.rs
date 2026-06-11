// 2.40 — Web companion read-only mobile view.
// Local HTTP server :7799 (bind 127.0.0.1 ONLY) que sirve UI mínima
// con cards open + last 100 events. NO auth porque solo loopback —
// access via SSH port forward desde móvil.
// Council V1+V4: bind 127.0.0.1 obligado, read-only, body limit, RAII shutdown.

use anyhow::Result;
use axum::{
    extract::{DefaultBodyLimit, State},
    response::{Html, IntoResponse},
    routing::get,
    Json, Router,
};
use parking_lot::Mutex;
use rusqlite::Connection;
use serde::Serialize;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::oneshot;

#[derive(Clone)]
struct AppCtx {
    db: Arc<Mutex<Connection>>,
}

pub struct WebCompanion {
    shutdown_tx: Option<oneshot::Sender<()>>,
}

impl WebCompanion {
    pub fn start(db: Arc<Mutex<Connection>>) -> Result<Self> {
        let (tx, rx) = oneshot::channel::<()>();
        let ctx = AppCtx { db };
        let app = Router::new()
            .route("/", get(handle_root))
            .route("/api/cards", get(handle_cards))
            .route("/api/events", get(handle_events))
            .layer(DefaultBodyLimit::max(4096))
            // Security headers on EVERY response (incl. errors / 404 / API), not just root.
            .layer(axum::middleware::from_fn(security_headers))
            .with_state(ctx);
        let addr: SocketAddr = "127.0.0.1:7799".parse()?;
        tauri::async_runtime::spawn(async move {
            let listener = match tokio::net::TcpListener::bind(addr).await {
                Ok(l) => l,
                Err(e) => {
                    tracing::error!("web_companion bind: {}", e);
                    return;
                }
            };
            tokio::select! {
                res = axum::serve(listener, app) => {
                    if let Err(e) = res { tracing::warn!("web_companion serve: {}", e); }
                }
                _ = rx => { tracing::info!("web_companion shutdown"); }
            }
        });
        Ok(Self {
            shutdown_tx: Some(tx),
        })
    }
    pub fn shutdown(&mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
    }
}

impl Drop for WebCompanion {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Security headers applied to every response (CSP + nosniff) — see the `from_fn` layer.
async fn security_headers(
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let mut resp = next.run(req).await;
    let h = resp.headers_mut();
    h.insert(
        "content-security-policy",
        axum::http::HeaderValue::from_static(
            "default-src 'none'; style-src 'unsafe-inline'; script-src 'unsafe-inline'; connect-src 'self'",
        ),
    );
    h.insert(
        "x-content-type-options",
        axum::http::HeaderValue::from_static("nosniff"),
    );
    resp
}

async fn handle_root() -> impl IntoResponse {
    // SECURITY: this read-only page renders values that originate in the DB (card titles,
    // project, event kind/actor). All interpolation is HTML-escaped client-side via esc();
    // `severity` flows into a CSS class so it is reduced to [A-Za-z0-9-] via cls(). The
    // CSP/nosniff headers are added by the security_headers layer (covers every response).
    Html(
        r##"<!doctype html><html><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1">
<title>Furx mobile</title>
<style>
body { background: #06090d; color: #d8e2ee; font: 14px/1.5 -apple-system, system-ui; margin: 0; padding: 12px; }
h1 { font-size: 18px; margin: 4px 0 10px; color: #ff8a6e; }
.card { background: #11161f; border: 1px solid #1d2837; border-left: 3px solid #ff8a6e; padding: 8px 12px; margin: 6px 0; border-radius: 6px; }
.muted { color: #6c7b91; font-size: 11px; font-family: monospace; }
.sev-warning { border-left-color: #f4b860; }
.sev-critical { border-left-color: #ff6b6b; }
.event { padding: 4px 8px; border-bottom: 1px solid #1d2837; font-family: monospace; font-size: 11px; }
</style></head>
<body>
<h1>◆ (Furx) mobile · read-only</h1>
<div id="cards"></div>
<h1 style="margin-top:14px">Last events</h1>
<div id="events"></div>
<script>
const esc = s => String(s ?? '').replace(/[&<>"']/g, c => ({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[c]));
const cls = s => String(s ?? '').replace(/[^A-Za-z0-9-]/g, '');
async function load() {
  const cards = await fetch('/api/cards').then(r => r.json()).catch(() => []);
  document.getElementById('cards').innerHTML = cards.map(c =>
    `<div class="card sev-${cls(c.severity)}"><strong>${esc(c.title)}</strong><div class="muted">${esc(c.project)} · ${esc(c.created_at)}</div></div>`
  ).join('') || '<div class="muted">no cards open</div>';
  const events = await fetch('/api/events').then(r => r.json()).catch(() => []);
  document.getElementById('events').innerHTML = events.map(e =>
    `<div class="event"><span class="muted">${esc(String(e.at).slice(11,19))}</span> ${esc(e.kind)} <span class="muted">(${esc(e.actor)})</span></div>`
  ).join('') || '<div class="muted">no events</div>';
}
load(); setInterval(load, 5000);
</script></body></html>"##,
    )
}

#[derive(Serialize)]
struct CardOut {
    id: String,
    project: String,
    title: String,
    severity: String,
    created_at: String,
}

async fn handle_cards(State(ctx): State<AppCtx>) -> Json<Vec<CardOut>> {
    let conn = ctx.db.lock();
    let rows: Vec<CardOut> = match conn.prepare(
        "SELECT id, project, title, severity, created_at FROM cards WHERE status='open' ORDER BY created_at DESC LIMIT 50"
    ) {
        Ok(mut stmt) => {
            let it = stmt.query_map([], |r| Ok(CardOut {
                id: r.get(0)?, project: r.get(1)?, title: r.get(2)?,
                severity: r.get(3)?, created_at: r.get(4)?,
            })).map(|i| i.filter_map(|x| x.ok()).collect()).unwrap_or_default();
            it
        }
        _ => vec![],
    };
    Json(rows)
}

#[derive(Serialize)]
struct EventOut {
    id: String,
    at: String,
    kind: String,
    actor: String,
}

async fn handle_events(State(ctx): State<AppCtx>) -> Json<Vec<EventOut>> {
    let conn = ctx.db.lock();
    let rows: Vec<EventOut> =
        match conn.prepare("SELECT id, at, kind, actor FROM events ORDER BY at DESC LIMIT 100") {
            Ok(mut stmt) => {
                let it = stmt
                    .query_map([], |r| {
                        Ok(EventOut {
                            id: r.get(0)?,
                            at: r.get(1)?,
                            kind: r.get(2)?,
                            actor: r.get(3)?,
                        })
                    })
                    .map(|i| i.filter_map(|x| x.ok()).collect())
                    .unwrap_or_default();
                it
            }
            _ => vec![],
        };
    Json(rows)
}
