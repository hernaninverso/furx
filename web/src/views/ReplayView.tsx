// 2.7 — Session replay scrubber.
import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { homeDir } from "@tauri-apps/api/path";

interface ScrubBucket { ts: string; count: number; kinds: string[]; }
interface ScrubData { buckets: ScrubBucket[]; total: number; first_at: string | null; last_at: string | null; }

// 053 — tipos para replay bundle.
interface ReplayBundleReport {
  path: string;
  size_bytes: number;
  sha256: string;
  events_count: number;
  redacted: boolean;
}

export function ReplayView() {
  const [data, setData] = useState<ScrubData | null>(null);
  const [hours, setHours] = useState(72);
  const [bucketIdx, setBucketIdx] = useState(0);
  const [events, setEvents] = useState<Record<string, unknown>[]>([]);
  // 053 — bundle state.
  const [bundleReport, setBundleReport] = useState<ReplayBundleReport | null>(null);
  const [bundleErr, setBundleErr] = useState<string | null>(null);
  const [bundleLoading, setBundleLoading] = useState(false);

  useEffect(() => { invoke<ScrubData>("replay_buckets", { hours }).then(setData).catch(() => setData(null)); }, [hours]);
  useEffect(() => {
    if (!data || !data.buckets[bucketIdx]) return;
    invoke<Record<string, unknown>[]>("replay_events_at", { bucketTs: data.buckets[bucketIdx].ts }).then(setEvents).catch(() => setEvents([]));
  }, [bucketIdx, data]);

  const handleCreateBundle = async () => {
    if (!data) return;
    const fromTs = data.first_at ?? data.buckets[0]?.ts ?? "";
    const toTs = data.last_at ?? data.buckets[data.buckets.length - 1]?.ts ?? "";
    if (!fromTs || !toTs) {
      setBundleErr("Sin datos en el rango seleccionado.");
      return;
    }
    setBundleLoading(true);
    setBundleErr(null);
    setBundleReport(null);
    try {
      // Build output path under ~/Downloads (or ~/ as fallback).
      let outDir: string;
      try {
        const home = await homeDir();
        outDir = `${home}/Downloads`;
      } catch {
        outDir = "~";
      }
      const ts = new Date().toISOString().replace(/[:.]/g, "-").slice(0, 19);
      // El backend genera un tar.zst (.furxreplay), NO un zip — la extensión engañosa haría que un
      // descompresor ZIP lo trate como corrupto (audit-3 Codex).
      const outPath = `${outDir}/furx-replay-${ts}.furxreplay`;
      const report = await invoke<ReplayBundleReport>("replay_bundle_create", {
        projectDir: null,
        spanStart: fromTs,
        spanEnd: toTs,
        outPath,
      });
      setBundleReport(report);
    } catch (e: unknown) {
      setBundleErr(e instanceof Error ? e.message : String(e));
    } finally {
      setBundleLoading(false);
    }
  };

  if (!data) return <div className="page"><div className="page-header"><div className="page-title">Replay scrubber</div></div><div className="muted" style={{ padding: 14 }}>loading…</div></div>;
  const cur = data.buckets[bucketIdx];
  return (
    <div className="page">
      <div className="page-header">
        <div className="page-title">Session replay</div>
        <div className="page-sub">{data.total} events · {data.first_at?.slice(0, 16)} → {data.last_at?.slice(0, 16)}</div>
      </div>
      <div className="form-row">
        <label>Window (hours)</label>
        <div className="form-input">
          <input type="number" value={hours} onChange={(e) => setHours(parseInt(e.target.value || "72"))} min={1} max={720} />
        </div>
      </div>
      <input type="range" min={0} max={Math.max(0, data.buckets.length - 1)} value={bucketIdx} onChange={(e) => setBucketIdx(parseInt(e.target.value))} style={{ width: "100%" }} />
      {cur && (
        <div className="card-block info" style={{ marginTop: 12 }}>
          <strong>{cur.ts}</strong> · {cur.count} events · kinds: <code>{cur.kinds.join(", ")}</code>
        </div>
      )}
      <pre style={{ background: "var(--bg2)", padding: 10, marginTop: 10, fontSize: 11, maxHeight: 420, overflow: "auto", whiteSpace: "pre-wrap" }}>
        {events.map((e) => JSON.stringify(e, null, 2)).join("\n---\n")}
      </pre>

      {/* 053 — Crear bundle del rango visible */}
      <div className="mon" style={{ marginTop: 16, padding: "12px 14px" }}>
        <div className="mon-head">
          <strong>Bundle de sesión</strong>
          <button
            className="btn btn-secondary"
            style={{ fontSize: 11, padding: "2px 9px" }}
            onClick={handleCreateBundle}
            disabled={bundleLoading || !data.first_at}
          >
            {bundleLoading ? "Creando…" : "Crear bundle"}
          </button>
        </div>
        <div className="muted" style={{ fontSize: 11, marginTop: 4 }}>
          Empaqueta todos los eventos del rango visible ({data.first_at?.slice(0, 16)} → {data.last_at?.slice(0, 16)}) en un archivo .furxreplay (tar.zst) en ~/Downloads.
        </div>
        {bundleErr && (
          <div style={{ color: "var(--red, #c0392b)", fontSize: 11, marginTop: 6 }}>{bundleErr}</div>
        )}
        {bundleReport && (
          <div
            style={{
              marginTop: 10,
              padding: "8px 10px",
              background: "var(--bg2)",
              borderRadius: 6,
              fontSize: 11,
              fontFamily: "var(--mono)",
              display: "flex",
              flexDirection: "column",
              gap: 3,
            }}
          >
            <div style={{ color: "var(--green)" }}>Bundle creado</div>
            <div>{bundleReport.path}</div>
            <div className="muted">
              {bundleReport.events_count} eventos · {(bundleReport.size_bytes / 1024).toFixed(1)} KB
              {bundleReport.redacted && " · redactado"}
            </div>
            <div className="muted" style={{ fontSize: 10 }}>sha256: {bundleReport.sha256.slice(0, 16)}…</div>
          </div>
        )}
      </div>
    </div>
  );
}
