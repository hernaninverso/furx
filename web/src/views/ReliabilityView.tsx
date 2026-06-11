// 050 Ola 8 P2 (FR-003) — Reliability board.
//
// Dashboard read-only de CALIDAD: tasa de éxito / latencia / costo por AGENTE y por MODELO, leído de
// `reliability_summary` (agrega la tabla append-only `reliability_events`). DISTINTO del panel de
// ahorro $ del cost-router. OPT-IN: si el board está OFF (default), el backend devuelve filas vacías
// y mostramos un panel que explica cómo activarlo (toggle del setting). Solo-medido: ningún número
// es proyección — son los runs observados en la ventana.

import { useEffect, useState, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Button } from "../components/Button";

interface ReliabilityRow {
  label: string;
  runs: number;
  successes: number;
  success_pct: number;
  avg_latency_ms: number | null;
  p95_latency_ms: number | null;
  total_cost_usd: number;
}

interface ReliabilitySummary {
  enabled: boolean;
  window_days: number;
  total_runs: number;
  by_agent: ReliabilityRow[];
  by_model: ReliabilityRow[];
}

const ENABLED_SETTING = "reliability.board_enabled";

function fmtLat(ms: number | null): string {
  if (ms == null) return "—";
  return ms >= 1000 ? `${(ms / 1000).toFixed(1)}s` : `${Math.round(ms)}ms`;
}

function fmtCost(usd: number): string {
  if (usd <= 0) return "$0";
  if (usd < 0.01) return "<$0.01";
  return `$${usd.toFixed(2)}`;
}

function RelTable({ title, rows }: { title: string; rows: ReliabilityRow[] }) {
  return (
    <section style={{ marginBottom: 22 }}>
      <h3 className="section-title">{title}</h3>
      {rows.length === 0 ? (
        <div className="body muted">Sin runs registrados en esta ventana.</div>
      ) : (
        <table className="data-table" style={{ width: "100%" }}>
          <thead>
            <tr>
              <th style={{ textAlign: "left" }}>{title.includes("modelo") ? "Modelo" : "Agente"}</th>
              <th style={{ textAlign: "right" }}>Runs</th>
              <th style={{ textAlign: "right" }}>Éxito</th>
              <th style={{ textAlign: "right" }}>Latencia (avg)</th>
              <th style={{ textAlign: "right" }}>p95</th>
              <th style={{ textAlign: "right" }}>Costo</th>
            </tr>
          </thead>
          <tbody>
            {rows.map((r) => (
              <tr key={r.label}>
                <td style={{ fontFamily: "var(--mono)" }}>{r.label}</td>
                <td style={{ textAlign: "right" }}>{r.runs}</td>
                <td style={{ textAlign: "right" }}>
                  {r.success_pct.toFixed(0)}%
                  <span className="muted" style={{ marginLeft: 6, fontSize: 11 }}>
                    ({r.successes}/{r.runs})
                  </span>
                </td>
                <td style={{ textAlign: "right" }}>{fmtLat(r.avg_latency_ms)}</td>
                <td style={{ textAlign: "right" }}>{fmtLat(r.p95_latency_ms)}</td>
                <td style={{ textAlign: "right" }}>{fmtCost(r.total_cost_usd)}</td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </section>
  );
}

export function ReliabilityView() {
  const [summary, setSummary] = useState<ReliabilitySummary | null>(null);
  const [busy, setBusy] = useState(false);

  const refresh = useCallback(async () => {
    try {
      setSummary(await invoke<ReliabilitySummary>("reliability_summary", { windowDays: 30 }));
    } catch (e) {
      console.error(e);
    }
  }, []);

  const enableBoard = async () => {
    setBusy(true);
    try {
      await invoke("settings_set", { key: ENABLED_SETTING, value: true });
      await refresh();
    } catch (e) {
      console.error(e);
    } finally {
      setBusy(false);
    }
  };

  useEffect(() => {
    refresh();
    const id = setInterval(refresh, 60000);
    return () => clearInterval(id);
  }, [refresh]);

  const off = summary != null && !summary.enabled;

  return (
    <div className="page">
      <div className="page-header">
        <div className="page-title">Reliability</div>
        <div className="page-sub">
          calidad por agente y modelo · solo-medido ·{" "}
          {summary ? `${summary.total_runs} runs · ${summary.window_days}d` : "…"}
        </div>
      </div>

      {off ? (
        <div className="empty">
          <div className="head">Board desactivado</div>
          <div className="body muted">
            El reliability board es opt-in. Al activarlo, Furx registra (solo-medido) la tasa de éxito,
            la latencia y el costo de cada corrida de agente — sin guardar prompts ni diffs, solo
            metadata (agente, modelo, éxito, números).
          </div>
          <div className="actions-row" style={{ marginTop: 12 }}>
            <Button variant="primary" onClick={enableBoard} disabled={busy}>
              {busy ? "activando…" : "Activar board"}
            </Button>
          </div>
        </div>
      ) : (
        <>
          <div className="actions-row" style={{ marginBottom: 14 }}>
            <Button variant="ghost" onClick={refresh}>
              Refresh
            </Button>
          </div>
          {summary && summary.total_runs === 0 ? (
            <div className="empty">
              <div className="head">Sin datos todavía</div>
              <div className="body muted">
                El board está activo pero aún no hay runs en los últimos {summary.window_days} días.
                Las métricas aparecen a medida que corren agentes (council, etc.).
              </div>
            </div>
          ) : (
            <>
              <RelTable title="Por agente" rows={summary?.by_agent ?? []} />
              <RelTable title="Por modelo" rows={summary?.by_model ?? []} />
            </>
          )}
        </>
      )}
    </div>
  );
}
