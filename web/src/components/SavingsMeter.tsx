// 048 Cost-Router Fase 1 (Savings Meter) — vista de SOLO-MEDIDO del ahorro del routing que Furx YA
// hace (local Ollama / free AIE / premium BYOK). NO desvía routing: solo muestra lo MEDIDO.
//
// NON-NEGOTIABLE:
//   - Muestra ÚNICAMENTE cifras reales medidas. NUNCA proyecta ni extrapola.
//   - Pitch sin overpromise: el ahorro real es una fracción de la factura, NO "$20 como $200".
//   - Sin la palabra "honest"/"honesto" en ningún copy (constitución F-III).
//   - Free / kill-switch OFF → estado "off" (sin cifras).
//   - Tokens del design system V3 ("atelier"), dark+light vía var(--…).
import { useEffect, useState } from "react";
import { invoke } from "../lib/invoke";
import { usePolling } from "../hooks/usePolling";

// 053 — tipos del clasificador v2 (read-only).
interface CostRouterV2Status {
  config_valid: boolean;
  classifier_version: number;
  phase: string; // "off" | "log_only" | "active"
  canary_gate_passed: boolean;
}

interface RouterV2SectionState {
  status: CostRouterV2Status | null;
  loading: boolean;
  reloading: boolean;
  error: string | null;
}

type MeterStatus = "off" | "warming_up" | "ready";

interface SavingsSummary {
  status: MeterStatus;
  spent_real_usd: number;
  baseline_premium_usd: number;
  saved_usd: number;
  saved_pct: number;
  events_counted: number;
  events_excluded_no_baseline: number;
  window_days: number;
  days_observed: number;
  eta_days: number | null;
}

interface SavingsBucket {
  bucket_start: string;
  spent_real_usd: number;
  saved_usd: number;
  events: number;
}

function usd(n: number): string {
  return `$${n.toFixed(2)}`;
}

// 057 — sparkline decorativo del hero (alturas fijas, look de serie de ahorro). Sólo atmósfera.
const HERO_SPARK = [38, 52, 30, 64, 44, 58, 36, 70, 48, 62, 34, 56, 42, 68, 50, 60, 32, 72, 46, 54, 40, 66, 38, 58, 44, 74, 48, 56, 36, 62, 50, 68, 42, 60];

export function SavingsMeter() {
  const [summary, setSummary] = useState<SavingsSummary | null>(null);
  const [series, setSeries] = useState<SavingsBucket[]>([]);
  const [loading, setLoading] = useState(true);

  // 058 — refresco lento: si el medidor cruza warming_up→ready con la vista abierta, la serie se trae
  // sin remount. El status cambia con granularidad de días → 120s alcanza. `usePolling` serializa
  // (`inFlight`, sin solapes) y para los ticks al desmontar; React 19 hace no-op el setState post-unmount.
  usePolling(async (isCancelled) => {
      try {
        const s = await invoke<SavingsSummary>("savings_summary");
        if (isCancelled()) return; // desmontado durante el await → no hagas el 2º invoke (audit codex)
        setSummary(s);
        if (s.status === "ready") {
          // `ser` (no `series`) — evita sombrear el state `series`.
          const ser = await invoke<SavingsBucket[]>("savings_series", { bucket: "day" });
          if (isCancelled()) return;
          setSeries(ser);
        }
      } catch {
        // Fail-soft: sin datos, el panel queda en estado vacío (no rompe la UI).
      } finally {
        setLoading(false);
      }
  }, { intervalMs: 120_000 });

  if (loading) {
    return (
      <div className="act-hero">
        <div className="eyebrow">Ahorro del routing</div>
        <p className="hsub">Cargando…</p>
      </div>
    );
  }

  if (!summary || summary.status === "off") {
    // 057 — gate premium (no texto pelado). El cost-router ya ahorra; el MEDIDOR acumulado es Pro/Team.
    return (
      <div className="act-hero">
        <div className="spark">{HERO_SPARK.map((h, i) => <i key={i} style={{ height: `${h}%` }} />)}</div>
        <span className="pill">Pro · Team</span>
        <h3>Medí cuánto te ahorra el routing</h3>
        <p className="hsub">
          El cost-router ya enruta a modelos free y locales cuando alcanzan. El medidor de ahorro acumulado vive en los planes Pro y Team.
        </p>
        <button className="cta" type="button">Ver planes →</button>
      </div>
    );
  }

  if (summary.status === "warming_up") {
    return (
      <div className="act-hero">
        <div className="spark">{HERO_SPARK.map((h, i) => <i key={i} style={{ height: `${h}%` }} />)}</div>
        <div className="eyebrow">Ahorro del routing</div>
        <h3>Midiendo tu patrón de uso</h3>
        <p className="hsub">
          Furx mide cuánto te ahorra el routing sobre tu uso real. El ahorro acumulado aparece a los ~30 días.
          {summary.eta_days != null && ` Faltan ~${summary.eta_days} días (${summary.days_observed} observados).`}
        </p>
      </div>
    );
  }

  // ready — solo cifras medidas. NUNCA proyecta.
  return (
    <div className="fx-card" style={{ padding: 16 }}>
      <div className="lbl">Ahorro verificable medido · últimos {summary.window_days} días</div>

      <div style={{ display: "flex", gap: 24, marginTop: 12, flexWrap: "wrap" }}>
        <Stat label="Gastado real" value={usd(summary.spent_real_usd)} />
        <Stat label="Sin router habrías gastado" value={usd(summary.baseline_premium_usd)} />
        <Stat
          label="Ahorro medido"
          value={`${usd(summary.saved_usd)} · ${summary.saved_pct.toFixed(0)}%`}
          accent
        />
      </div>

      <hr className="fx-rule" style={{ margin: "14px 0" }} />

      <div style={{ color: "var(--ink-3)", fontFamily: "var(--font-mono)", fontSize: 11 }}>
        {summary.events_counted} decisiones medidas
        {summary.events_excluded_no_baseline > 0 &&
          ` · ${summary.events_excluded_no_baseline} sin baseline (excluidas del cálculo)`}
      </div>

      {series.length > 0 && <SavingsSparkline series={series} />}

      <div style={{ color: "var(--ink-3)", fontFamily: "var(--font-sans)", fontSize: 11, marginTop: 10 }}>
        Cifras medidas sobre tu uso real. No incluye proyecciones.
      </div>
    </div>
  );
}

function Stat({ label, value, accent }: { label: string; value: string; accent?: boolean }) {
  return (
    <div>
      <div className="lbl">{label}</div>
      <div
        style={{
          fontFamily: "var(--font-display)",
          fontSize: 22,
          marginTop: 4,
          color: accent ? "var(--accent)" : "var(--ink)",
        }}
      >
        {value}
      </div>
    </div>
  );
}

// Sparkline minimalista del ahorro por día (solo lo medido). Sin librerías externas.
function SavingsSparkline({ series }: { series: SavingsBucket[] }) {
  const max = Math.max(1e-9, ...series.map((b) => b.saved_usd));
  return (
    <div style={{ display: "flex", alignItems: "flex-end", gap: 2, height: 40, marginTop: 12 }}>
      {series.map((b) => (
        <div
          key={b.bucket_start}
          title={`${b.bucket_start}: ${usd(b.saved_usd)} (${b.events} decisiones)`}
          style={{
            flex: 1,
            minWidth: 2,
            height: `${Math.max(2, (b.saved_usd / max) * 100)}%`,
            background: "var(--accent-dim)",
            borderRadius: 1,
          }}
        />
      ))}
    </div>
  );
}

// 053 — sección Router v2: estado read-only del clasificador + botón de recarga de policy.
export function RouterV2Section() {
  const [state, setState] = useState<RouterV2SectionState>({
    status: null,
    loading: true,
    reloading: false,
    error: null,
  });

  const loadStatus = async () => {
    setState((s) => ({ ...s, loading: true, error: null }));
    try {
      // cost_router_status returns CostRouterStatus; we only need the v2 field.
      const full = await invoke<{ v2: CostRouterV2Status }>("cost_router_status");
      setState({ status: full.v2, loading: false, reloading: false, error: null });
    } catch (e: unknown) {
      setState((s) => ({
        ...s,
        loading: false,
        error: e instanceof Error ? e.message : String(e),
      }));
    }
  };

  const reloadPolicy = async () => {
    setState((s) => ({ ...s, reloading: true, error: null }));
    try {
      await invoke("cost_router_policy_reload");
      await loadStatus();
    } catch (e: unknown) {
      setState((s) => ({
        ...s,
        reloading: false,
        error: e instanceof Error ? e.message : String(e),
      }));
    }
  };

  useEffect(() => { void loadStatus(); }, []);

  const phaseColor = (phase: string) => {
    if (phase === "active") return "var(--green)";
    if (phase === "log_only") return "var(--amber)";
    return "var(--ink-3)";
  };

  return (
    <div className="fx-card" style={{ padding: 16, marginTop: 12 }}>
      <div style={{ display: "flex", alignItems: "center", gap: 10, marginBottom: 10 }}>
        <div className="lbl">Router v2</div>
        {state.status && (
          <span
            style={{
              fontSize: 11,
              fontFamily: "var(--font-mono)",
              padding: "2px 7px",
              borderRadius: 4,
              background: "var(--surface2, var(--bg2))",
              color: phaseColor(state.status.phase),
            }}
          >
            {state.status.phase}
          </span>
        )}
      </div>

      {state.loading && (
        <div style={{ color: "var(--ink-3)", fontFamily: "var(--font-sans)", fontSize: 12 }}>
          Cargando…
        </div>
      )}

      {state.error && (
        <div style={{ color: "var(--red, #c0392b)", fontSize: 12, marginBottom: 8 }}>
          {state.error}
        </div>
      )}

      {state.status && !state.loading && (
        <div
          style={{
            display: "flex",
            gap: 16,
            flexWrap: "wrap",
            fontFamily: "var(--font-mono)",
            fontSize: 11,
            color: "var(--ink-2)",
          }}
        >
          <span>
            config:{" "}
            <strong style={{ color: state.status.config_valid ? "var(--green)" : "var(--red, #c0392b)" }}>
              {state.status.config_valid ? "válido" : "inválido"}
            </strong>
          </span>
          <span>v{state.status.classifier_version}</span>
          <span>
            canary gate:{" "}
            <strong style={{ color: state.status.canary_gate_passed ? "var(--green)" : "var(--ink-3)" }}>
              {state.status.canary_gate_passed ? "pasado" : "pendiente"}
            </strong>
          </span>
        </div>
      )}

      <div style={{ marginTop: 12 }}>
        <button
          className="btn btn-secondary"
          style={{ fontSize: 12 }}
          onClick={reloadPolicy}
          disabled={state.reloading || state.loading}
        >
          {state.reloading ? "Recargando…" : "Recargar policy"}
        </button>
      </div>
    </div>
  );
}
