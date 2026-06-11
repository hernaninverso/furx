// 2.10 / W4 — Cost-aware router cascade visualizer.
import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

interface CascadeProvider { provider: string; model: string; blocked_until: string | null; bucket_used: number; bucket_limit: number; dimension: string; }
interface CascadeSnapshot { enabled: boolean; shadow_mode: boolean; fetched_at: string; providers: CascadeProvider[]; }

// 053 — resilience snapshot types (ProviderHealthSnapshot from Rust).
interface ProviderHealthSnapshot {
  provider: string;
  model: string;
  credential_alias: string;
  rate_limit_blocked_until: string | null;
  quota_blocked_until: string | null;
  circuit_blocked_until: string | null;
  consecutive_failures: number;
}

export function RouterView() {
  const [snap, setSnap] = useState<CascadeSnapshot | null>(null);
  const [err, setErr] = useState<string | null>(null);
  // 053 — resilience snapshot state.
  const [resSnap, setResSnap] = useState<ProviderHealthSnapshot[] | null>(null);
  const [resErr, setResErr] = useState<string | null>(null);
  const [resLoading, setResLoading] = useState(false);

  const refresh = async () => {
    try { setSnap(await invoke<CascadeSnapshot>("router_snapshot")); setErr(null); }
    catch (e) { setErr(String(e)); }
  };
  useEffect(() => { refresh(); const id = setInterval(refresh, 5000); return () => clearInterval(id); }, []);

  const loadResilienceSnapshot = async () => {
    setResLoading(true);
    setResErr(null);
    try {
      const data = await invoke<ProviderHealthSnapshot[]>("resilience_snapshot");
      setResSnap(data);
    } catch (e: unknown) {
      setResErr(e instanceof Error ? e.message : String(e));
    } finally {
      setResLoading(false);
    }
  };

  if (!snap) return <div className="page"><div className="page-header"><div className="page-title">AIE Router</div></div><div className="muted" style={{ padding: 14 }}>{err ?? "loading…"}</div></div>;
  const grouped = new Map<string, CascadeProvider[]>();
  for (const p of snap.providers) {
    const arr = grouped.get(p.provider) ?? [];
    arr.push(p); grouped.set(p.provider, arr);
  }
  return (
    <div className="page">
      <div className="page-header">
        <div className="page-title">AIE Router cascade</div>
        <div className="page-sub">enabled={String(snap.enabled)} · shadow={String(snap.shadow_mode)} · fetched {snap.fetched_at.slice(11, 19)} · {snap.providers.length} providers</div>
      </div>
      {err && <div className="card-block info" style={{ borderLeftColor: "var(--amber)" }}>{err}</div>}
      <div className="mon-grid" style={{ marginTop: 14 }}>
        {Array.from(grouped.entries()).map(([prov, items]) => (
          <div key={prov} className="mon">
            <div className="mon-head">
              <strong>{prov}</strong>
              <span className="mon-addr muted">{items.length} models</span>
            </div>
            {items.map((p, i) => {
              const blocked = !!p.blocked_until;
              const pct = p.bucket_limit > 0 ? Math.round((p.bucket_used / p.bucket_limit) * 100) : 0;
              return (
                <div key={i} style={{ marginTop: 6, fontSize: 11, fontFamily: "var(--mono)" }}>
                  <span style={{ color: blocked ? "var(--red)" : pct > 80 ? "var(--amber)" : "var(--green)" }}>●</span>{" "}
                  {p.model} · {p.dimension} · <strong>{pct}%</strong> ({p.bucket_used}/{p.bucket_limit})
                  {blocked && <span className="muted"> · blocked until {p.blocked_until?.slice(11, 19)}</span>}
                </div>
              );
            })}
          </div>
        ))}
      </div>

      {/* 053 — Resilience snapshot */}
      <div className="mon" style={{ marginTop: 18, padding: "12px 14px" }}>
        <div className="mon-head">
          <strong>Resilience snapshot</strong>
          <button
            className="btn btn-secondary"
            style={{ fontSize: 11, padding: "2px 9px" }}
            onClick={loadResilienceSnapshot}
            disabled={resLoading}
          >
            {resLoading ? "Cargando…" : "Resilience snapshot"}
          </button>
        </div>
        {resErr && (
          <div style={{ color: "var(--red, #c0392b)", fontSize: 11, marginTop: 6 }}>{resErr}</div>
        )}
        {resSnap !== null && resSnap.length === 0 && (
          <div className="muted" style={{ fontSize: 11, marginTop: 8 }}>Sin datos de resilience aún.</div>
        )}
        {resSnap !== null && resSnap.length > 0 && (
          <div style={{ display: "flex", flexDirection: "column", gap: 4, marginTop: 10 }}>
            {resSnap.map((h, i) => {
              const anyBlocked = h.rate_limit_blocked_until || h.quota_blocked_until || h.circuit_blocked_until;
              return (
                <div
                  key={i}
                  style={{
                    fontSize: 11,
                    fontFamily: "var(--mono)",
                    padding: "5px 8px",
                    background: "var(--bg2)",
                    borderRadius: 5,
                    display: "flex",
                    gap: 8,
                    flexWrap: "wrap",
                    alignItems: "baseline",
                  }}
                >
                  <span style={{ color: anyBlocked ? "var(--red)" : "var(--green)" }}>●</span>
                  <span>{h.provider} / {h.model}</span>
                  {h.credential_alias && <span className="muted">({h.credential_alias})</span>}
                  {h.consecutive_failures > 0 && (
                    <span style={{ color: "var(--amber)" }}>fails: {h.consecutive_failures}</span>
                  )}
                  {h.rate_limit_blocked_until && (
                    <span className="muted">rate_limit until {h.rate_limit_blocked_until.slice(11, 19)}</span>
                  )}
                  {h.quota_blocked_until && (
                    <span className="muted">quota until {h.quota_blocked_until.slice(11, 19)}</span>
                  )}
                  {h.circuit_blocked_until && (
                    <span style={{ color: "var(--red)" }}>circuit open until {h.circuit_blocked_until.slice(11, 19)}</span>
                  )}
                </div>
              );
            })}
          </div>
        )}
      </div>
    </div>
  );
}
