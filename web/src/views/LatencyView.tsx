// 1.8 — Latency heatmap LLM providers view.

import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Button } from "../components/Button";

interface LatencyCell { provider: string; day: string; hour: number; avg_rtt_ms: number; blocked_ratio: number; samples: number; }

export function LatencyView() {
  const [cells, setCells] = useState<LatencyCell[]>([]);
  const [busy, setBusy] = useState(false);
  const refresh = async () => {
    try { setCells(await invoke<LatencyCell[]>("latency_heatmap", { days: 7 })); } catch {}
  };
  const pollNow = async () => {
    setBusy(true);
    try { await invoke<number>("latency_poll_once"); await refresh(); }
    catch (e) { console.error(e); }
    finally { setBusy(false); }
  };
  useEffect(() => { refresh(); const id = setInterval(refresh, 60000); return () => clearInterval(id); }, []);

  const providers = Array.from(new Set(cells.map((c) => c.provider))).sort();
  const today = new Date();
  const days: string[] = [];
  for (let i = 6; i >= 0; i--) days.push(new Date(today.getTime() - i * 86400_000).toISOString().slice(0, 10));
  const maxRtt = Math.max(1, ...cells.map((c) => c.avg_rtt_ms));

  return (
    <div className="page">
      <div className="page-header">
        <div className="page-title">LLM provider latency</div>
        <div className="page-sub">poll AIE /v1/resilience/state · {cells.length} samples · {providers.length} providers</div>
      </div>
      <div className="actions-row" style={{ marginBottom: 14 }}>
        <Button variant="ghost" onClick={pollNow} disabled={busy}>{busy ? "polleando…" : "Poll now"}</Button>
        <Button variant="ghost" onClick={refresh}>Refresh</Button>
      </div>
      {providers.length === 0
        ? <div className="empty"><div className="head">Sin datos</div><div className="body muted">Apretá "Poll now" para empezar a recolectar.</div></div>
        : providers.map((p) => {
            const lookup = new Map<string, LatencyCell>();
            for (const c of cells.filter((x) => x.provider === p)) lookup.set(`${c.day}::${c.hour}`, c);
            return (
              <section key={p} style={{ marginBottom: 22 }}>
                <h3 className="section-title">{p}</h3>
                <svg width={Math.min(900, 50 + (12 + 2) * 24)} height={22 + (10 + 2) * 7} style={{ background: "var(--panel-2)", border: "1px solid var(--line)", borderRadius: 6 }}>
                  {Array.from({ length: 24 }).map((_, h) => (
                    <text key={`h${h}`} x={50 + h * 14 + 6} y={12} fontSize={9} fill="var(--muted)" textAnchor="middle" fontFamily="var(--mono)">{h.toString().padStart(2, "0")}</text>
                  ))}
                  {days.map((day, dyi) => (
                    <g key={day}>
                      <text x={44} y={22 + dyi * 12 + 8} fontSize={9} fill="var(--muted)" textAnchor="end" fontFamily="var(--mono)">{day.slice(5)}</text>
                      {Array.from({ length: 24 }).map((_, h) => {
                        const c = lookup.get(`${day}::${h}`);
                        if (!c) return <rect key={h} x={50 + h * 14} y={22 + dyi * 12} width={12} height={10} fill="var(--wash-1)" rx={1} />;
                        const intensity = Math.min(1, c.avg_rtt_ms / maxRtt);
                        const blocked = c.blocked_ratio;
                        const fill = blocked > 0.3
                          ? `rgba(255,107,107,${0.3 + 0.6 * blocked})`
                          : `rgba(255,138,110,${0.15 + 0.7 * intensity})`;
                        return <rect key={h} x={50 + h * 14} y={22 + dyi * 12} width={12} height={10} fill={fill} rx={1}>
                          <title>{day} {h}:00 · {Math.round(c.avg_rtt_ms)}ms avg · {c.samples} samples · {Math.round(c.blocked_ratio*100)}% blocked</title>
                        </rect>;
                      })}
                    </g>
                  ))}
                </svg>
              </section>
            );
          })}
    </div>
  );
}
