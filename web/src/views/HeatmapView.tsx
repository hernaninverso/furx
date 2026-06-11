import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { HeatmapData } from "../types";

export function HeatmapView() {
  const [data, setData] = useState<HeatmapData | null>(null);
  useEffect(() => {
    let alive = true;
    const refresh = async () => {
      try { const d = await invoke<HeatmapData>("heatmap_data", { days: 30 }); if (alive) setData(d); } catch {}
    };
    refresh();
    const id = setInterval(refresh, 30000);
    return () => { alive = false; clearInterval(id); };
  }, []);
  if (!data) return <div className="page"><div className="page-header"><div className="page-title">Heatmap</div></div><div className="muted" style={{ padding: 14 }}>cargando…</div></div>;
  const today = new Date();
  const dayLabels: string[] = [];
  for (let i = data.days - 1; i >= 0; i--) {
    const d = new Date(today.getTime() - i * 86400_000);
    dayLabels.push(d.toISOString().slice(0, 10));
  }
  const max = Math.max(1, data.max_count);
  const cellW = 14, cellH = 12, gap = 2, marginL = 50, marginT = 22;
  const width = marginL + (cellW + gap) * 24;
  const height = marginT + (cellH + gap) * dayLabels.length;
  const lookup = new Map<string, number>();
  for (const c of data.cells) lookup.set(`${c.day}::${c.hour}`, c.count);
  return (
    <div className="page">
      <div className="page-header">
        <div className="page-title">Activity heatmap</div>
        <div className="page-sub">events últimos {data.days}d · total {data.total} · max/cell {data.max_count}</div>
      </div>
      <div style={{ overflowX: "auto", marginTop: 14 }}>
        <svg width={width} height={height} style={{ background: "var(--panel-2)", border: "1px solid var(--line)", borderRadius: 6 }}>
          {Array.from({ length: 24 }).map((_, h) => (
            <text key={`h${h}`} x={marginL + h * (cellW + gap) + cellW / 2} y={14} fontSize={9} fill="var(--muted)" textAnchor="middle" fontFamily="var(--mono)">{h.toString().padStart(2, "0")}</text>
          ))}
          {dayLabels.map((day, dyi) => (
            <g key={day}>
              <text x={marginL - 6} y={marginT + dyi * (cellH + gap) + cellH * 0.8} fontSize={9} fill="var(--muted)" textAnchor="end" fontFamily="var(--mono)">{day.slice(5)}</text>
              {Array.from({ length: 24 }).map((_, h) => {
                const c = lookup.get(`${day}::${h}`) ?? 0;
                const intensity = c / max;
                const fill = c === 0 ? "var(--wash-1)" : `rgba(255,138,110,${0.15 + 0.75 * intensity})`;
                return <rect key={`${day}-${h}`} x={marginL + h * (cellW + gap)} y={marginT + dyi * (cellH + gap)} width={cellW} height={cellH} fill={fill} rx={2}>
                  <title>{day} {h}:00 · {c} events</title>
                </rect>;
              })}
            </g>
          ))}
        </svg>
      </div>
    </div>
  );
}
