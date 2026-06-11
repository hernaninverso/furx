// web/src/views/ActivityView.tsx — 057 · "Actividad" como ACTION CENTER (centro de excepciones).
//
// Diseño del consejo (codex 0.97 + gemini 0.95) + rediseño visual "Atelier Terminal" (loop de diseño
// validado en preview). v1 LOCAL-FIRST: hero del ahorro del cost-router (el diferencial) + alertas
// ACCIONABLES (sólo lo que requiere atención) + una franja de vitales glanceable. El probing externo
// de gasto por proveedor (openusage) se difiere a v1.1 opt-in.
//
// Fail-open + NUNCA "sano falso": cada señal se lee best-effort; un fallo (excepción O forma no-array)
// cuenta como probe no verificado. Si TODAS fallan → "no se pudo verificar"; si algunas → "verificación
// parcial". Nunca afirmamos "todo en orden" sin haberlo verificado.
import { useState } from "react";
import { Network, TriangleAlert, ChevronRight, Activity as ActivityIcon } from "lucide-react";
import { invoke } from "../lib/invoke";
import { usePolling } from "../hooks/usePolling";
import { SavingsMeter } from "../components/SavingsMeter";
import type { View } from "../lib/router";
import type { MonitorSnapshot } from "../types";

interface McpServerHealth {
  name: string;
  healthy: boolean;
  // 058 (ultrareview fix) — override de la DB (mcp_health lo anota). Un server que el usuario
  // DESHABILITÓ NO debe contar como "caído" ni levantar alerta roja (era un falso positivo permanente).
  enabled?: boolean;
}
interface McpHealthReport {
  servers: McpServerHealth[];
}
// 058 — resumen de un crash log (crash_log_list → CrashSummary). `iso_ts` es el prefijo del filename.
interface CrashSummary {
  iso_ts?: string;
}

// 058 — refresco del Action Center (antes: fetch único en mount → "actualizado HH:MM" mentía si algo
// caía con la vista abierta). Y ventana de "reciente" para crashes: sólo alertamos por fallos de las
// últimas 24h (los históricos NO deben quedar fijados como alerta permanente — son artefactos pasados).
const REFRESH_MS = 30_000;
const CRASH_RECENT_MS = 24 * 60 * 60 * 1000;

/// Edad en ms de un crash a partir de su `iso_ts`, o null si no parsea. Formatos del writer:
/// normal "YYYY-MM-DDTHHMMSSZ" (el writer le saca los `:`); panic-path "panic-<epoch_secs>".
function crashAgeMs(isoTs: string | undefined): number | null {
  if (!isoTs) return null;
  const m = /^(\d{4}-\d{2}-\d{2})T(\d{2})(\d{2})(\d{2})Z?$/.exec(isoTs);
  if (m) {
    const t = Date.parse(`${m[1]}T${m[2]}:${m[3]}:${m[4]}Z`);
    return Number.isNaN(t) ? null : Date.now() - t;
  }
  const p = /^panic-(\d+)$/.exec(isoTs);
  if (p) return Date.now() - Number(p[1]) * 1000;
  return null;
}

interface Alert {
  id: "mcp" | "crash" | "mon";
  sev: "red" | "amber";
  title: string;
  detail: string;
  view: View;
}

interface Vitals {
  monUp: number | null;
  monTotal: number | null;
  mcpUp: number | null;
  mcpTotal: number | null;
  crashes: number | null;
}

const ALERT_ICON = { mcp: Network, crash: TriangleAlert, mon: ActivityIcon } as const;

export function ActivityView({ onNavigate }: { onNavigate: (v: View) => void }) {
  const [alerts, setAlerts] = useState<Alert[] | null>(null);
  const [vitals, setVitals] = useState<Vitals>({ monUp: null, monTotal: null, mcpUp: null, mcpTotal: null, crashes: null });
  const [freshAt, setFreshAt] = useState<number | null>(null);
  const [verify, setVerify] = useState<{ probes: number; failures: number } | null>(null);

  // 058 — refresco periódico: el "actualizado HH:MM" refleja un estado VIVO, no uno congelado del
  // mount (un monitor/MCP que cae con la vista abierta aparece sin reabrir). `usePolling` serializa las
  // rondas (`inFlight`): nunca hay dos solapadas → no hace falta el guard de generación de antes, y una
  // ronda en vuelo al desmontar no puede pisar datos (React 19: setState post-unmount = no-op).
  usePolling(async () => {
      const out: Alert[] = [];
      const v: Vitals = { monUp: null, monTotal: null, mcpUp: null, mcpTotal: null, crashes: null };
      let probes = 0;
      let failures = 0;

      // 058 (ultrareview fix) — las 3 señales son independientes → en PARALELO (antes secuencial: el
      // tiempo de carga era la SUMA de las 3). allSettled: un fallo de una NO tumba a las otras.
      const [mcpRes, crashRes, monRes] = await Promise.allSettled([
        invoke<McpHealthReport>("mcp_health"),
        invoke<CrashSummary[]>("crash_log_list"),
        invoke<MonitorSnapshot[]>("list_monitors"),
      ]);

      // MCP — un server DESHABILITADO no es alerta ni cuenta en los vitales (058).
      probes++;
      if (mcpRes.status === "fulfilled" && Array.isArray(mcpRes.value?.servers)) {
        const enabled = mcpRes.value.servers.filter((s) => s?.enabled !== false);
        v.mcpTotal = enabled.length;
        v.mcpUp = enabled.filter((s) => s?.healthy === true).length;
        const down = enabled.filter((s) => s?.healthy === false);
        if (down.length > 0) {
          out.push({ id: "mcp", sev: "red", title: `${down.length} servidor${down.length > 1 ? "es" : ""} MCP sin responder`, detail: down.map((s) => s?.name ?? "?").slice(0, 4).join(", "), view: "health" });
        }
      } else {
        failures++;
      }

      // Crash logs — SÓLO los de las últimas 24h alertan; los históricos no quedan fijados (058).
      probes++;
      if (crashRes.status === "fulfilled" && Array.isArray(crashRes.value)) {
        const recent = crashRes.value.filter((c) => {
          const age = crashAgeMs(c?.iso_ts);
          // 058 (ultrareview audit fix): `age >= 0` — un ts futuro (clock skew) daba edad negativa que
          // pasaba `< 24h` y se contaba como reciente para siempre.
          return age !== null && age >= 0 && age < CRASH_RECENT_MS;
        });
        v.crashes = recent.length;
        if (recent.length > 0) {
          out.push({ id: "crash", sev: "amber", title: `${recent.length} fallo${recent.length > 1 ? "s" : ""} en las últimas 24h`, detail: "Revisá los crash logs", view: "crashlog" });
        }
      } else {
        failures++;
      }

      // Monitores.
      probes++;
      if (monRes.status === "fulfilled" && Array.isArray(monRes.value)) {
        v.monTotal = monRes.value.length;
        v.monUp = monRes.value.filter((m) => m?.last?.up === true).length;
        const down = monRes.value.filter((m) => m?.last?.up === false);
        if (down.length > 0) {
          out.push({ id: "mon", sev: "red", title: `${down.length} monitor${down.length > 1 ? "es" : ""} caído${down.length > 1 ? "s" : ""}`, detail: down.map((m) => m.target?.label ?? "?").slice(0, 4).join(", "), view: "monitors" });
        }
      } else {
        failures++;
      }

      setAlerts(out);
      setVitals(v);
      setVerify({ probes, failures });
      setFreshAt(Date.now());
  }, { intervalMs: REFRESH_MS });

  const fresh = freshAt ? new Date(freshAt).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" }) : null;
  const fmt = (n: number | null) => (n === null ? "—" : String(n));

  return (
    <div className="activity-view">
      <div className="view-head">
        <h1>Actividad</h1>
        {fresh && <span className="fresh">actualizado <b>{fresh}</b></span>}
      </div>

      {/* Hero: el AHORRO del cost-router (el diferencial). El SavingsMeter maneja gated/real. */}
      <SavingsMeter />

      {/* Centro de excepciones: SÓLO lo accionable. */}
      <div className="sec-label">Requiere tu atención</div>
      <section className="activity-alerts">
        {alerts === null || verify === null ? (
          <div className="activity-ok">Revisando…</div>
        ) : alerts.length > 0 ? (
          <>
            {alerts.map((a) => {
              const Icon = ALERT_ICON[a.id];
              return (
                <button key={a.id} type="button" className={`activity-alert ${a.sev}`} onClick={() => onNavigate(a.view)}>
                  <span className="sev" />
                  <span className="ico"><Icon /></span>
                  <span className="body">
                    <span className="aa-title">{a.title}</span>
                    <span className="aa-detail">{a.detail}</span>
                  </span>
                  <span className="aa-go">Ver detalle <ChevronRight /></span>
                </button>
              );
            })}
            {verify.failures > 0 && (
              <div className="activity-ok">{verify.failures} de {verify.probes} señales no respondieron — verificación parcial.</div>
            )}
          </>
        ) : verify.failures >= verify.probes ? (
          <div className="activity-ok">No se pudo verificar el estado (sin datos). Reabrí la vista para reintentar.</div>
        ) : verify.failures > 0 ? (
          <div className="activity-ok">Sin alertas, pero {verify.failures} de {verify.probes} señales no respondieron — verificación parcial.</div>
        ) : (
          <div className="activity-ok">✓ Todo en orden — sin problemas que requieran tu atención.</div>
        )}
      </section>

      {/* Vitales glanceable. */}
      <div className="sec-label">Vitales</div>
      <div className="vitals">
        <div className="vtile">
          <div className={`vn ${vitals.monTotal !== null && vitals.monUp === vitals.monTotal ? "ok" : ""}`}>{fmt(vitals.monUp)}<small>/{fmt(vitals.monTotal)}</small></div>
          <div className="vl">Monitores arriba</div>
        </div>
        <div className="vtile">
          <div className={`vn ${vitals.mcpTotal !== null && vitals.mcpUp === vitals.mcpTotal ? "ok" : ""}`}>{fmt(vitals.mcpUp)}<small>/{fmt(vitals.mcpTotal)}</small></div>
          <div className="vl">Servidores MCP</div>
        </div>
        <div className="vtile">
          <div className={`vn ${vitals.crashes === 0 ? "ok" : ""}`}>{fmt(vitals.crashes)}</div>
          <div className="vl">Fallos (24h)</div>
        </div>
      </div>
    </div>
  );
}
