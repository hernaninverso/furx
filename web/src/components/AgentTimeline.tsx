// 035-ai-visibility-evidence F4 — el TIMELINE EN VIVO de un agente (Golpe 2, primario).
//
// Anima EN TIEMPO REAL mientras el agente trabaja ("está pasando AHORA", estilo Devin en vivo):
//   - base CANÓNICA: `buildTimeline(task, log-history)` (cap 30 del backend) — datos persistidos.
//   - LIVE: se suscribe al eventBus (`useAppEvent('TaskChanged')`) FILTRADO a ESTA tarea; cada
//     transición de estado observada se agrega como paso PROVISIONAL (`liveStepFromTransition`),
//     y al recibir el evento se re-pide el log-history (el snapshot real supersede al provisional).
//   - scrub/replay SECUNDARIO: un slider que mueve el índice (clampScrub, O(1)) sin re-ejecutar nada.
//   - modo "cine" fullscreen: el MISMO componente agrandado para demos (role=dialog, Esc, foco, a11y).
//
// Conservadurismo: un paso vivo NUNCA fabrica contenido (sólo registra la transición + su momento).
// El eventBus ya dedup por seq (un snapshot viejo no pisa uno nuevo). La animación del paso más nuevo
// se desactiva con prefers-reduced-motion. SÓLO el timeline ABIERTO se suscribe (no todas las tareas).

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "../lib/invoke";
import { useAppEvent } from "../lib/eventBus";
import { Modal } from "./Modal"; // focus-trap + Esc + aria-hidden background + scroll-lock (a11y)
import {
  buildTimeline,
  mergeLiveSteps,
  liveStepFromTransition,
  clampScrub,
  type TimelineStep,
} from "../lib/aiVisibility";
import type { OrchTask, OrchLogEntry } from "../types";

const LIVE_CAP = 50;

/** Color/glyph por kind del paso (estética V3, tokens del tema). */
function stepStyle(kind: TimelineStep["kind"]): { color: string; glyph: string } {
  switch (kind) {
    case "spawned": return { color: "var(--ink-dim, #6b6358)", glyph: "◆" };
    case "working": return { color: "var(--accent)", glyph: "▸" };
    case "live": return { color: "var(--accent)", glyph: "▸" };
    case "ready": return { color: "var(--warn, #9a6011)", glyph: "◇" };
    case "manual": return { color: "var(--ink-dim, #6b6358)", glyph: "•" };
    case "terminal": return { color: "var(--ok, #3a6b3f)", glyph: "✦" };
    default: return { color: "var(--ink-dim, #6b6358)", glyph: "•" };
  }
}

/**
 * El cuerpo del timeline (lista de pasos + scrub). Compartido por el inline y el modo cine.
 * `live` indica si la tarea sigue corriendo (muestra el latido "en vivo").
 */
function TimelineBody({
  steps, scrubIndex, setScrub, live, newestIndex, big,
}: {
  steps: TimelineStep[];
  scrubIndex: number;
  setScrub: (i: number) => void;
  live: boolean;
  newestIndex: number;
  big?: boolean;
}) {
  const cur = steps[clampScrub(scrubIndex, steps.length)] ?? null;
  return (
    <div>
      <div style={{ display: "flex", alignItems: "center", gap: 8, marginBottom: big ? 14 : 8 }}>
        <div style={{ fontFamily: "var(--mono)", fontSize: big ? 12 : 10, letterSpacing: ".06em", textTransform: "uppercase", color: "var(--accent)", display: "flex", alignItems: "center", gap: 6 }}>
          {live && <span className="furx-tl-livedot" aria-hidden />}
          {live ? "Timeline · en vivo" : "Timeline"} · {steps.length} paso(s)
        </div>
      </div>

      {/* Lista de pasos: el más nuevo aparece animado (CSS, salvo reduced-motion). */}
      <div style={{ display: "flex", flexDirection: "column", gap: big ? 8 : 5, maxHeight: big ? "52vh" : 220, overflowY: "auto" }}>
        {steps.length === 0 ? (
          <div style={{ fontSize: 12, color: "var(--ink-dim, #6b6358)" }}>Todavía no hay pasos para esta tarea.</div>
        ) : steps.map((s) => {
          const st = stepStyle(s.kind);
          const isSelected = s.index === clampScrub(scrubIndex, steps.length);
          return (
            <button
              key={`${s.index}-${s.at}`}
              type="button"
              onClick={() => setScrub(s.index)}
              className={`furx-tl-step${s.index === newestIndex ? " is-new" : ""}`}
              style={{
                display: "flex", alignItems: "baseline", gap: 8, textAlign: "left", cursor: "pointer",
                border: isSelected ? `1px solid ${st.color}` : "1px solid transparent",
                background: isSelected ? "color-mix(in srgb, var(--accent) 6%, transparent)" : "transparent",
                borderRadius: 6, padding: big ? "7px 10px" : "4px 7px",
              }}
            >
              <span aria-hidden style={{ color: st.color, fontSize: big ? 14 : 12 }}>{st.glyph}</span>
              <span style={{ minWidth: 0, flex: 1 }}>
                <span style={{ fontSize: big ? 14 : 12.5, color: "var(--ink, #1c1814)" }}>{s.label}</span>
                {s.provisional && (
                  <span title="paso observado en vivo (provisional); el snapshot real lo confirma" style={{ marginLeft: 6, fontSize: 10, fontFamily: "var(--mono)", color: "var(--ink-dim, #6b6358)", border: "1px dashed var(--line, rgba(0,0,0,.3))", borderRadius: 4, padding: "0 5px" }}>en vivo</span>
                )}
                <span style={{ display: "block", fontFamily: "var(--mono)", fontSize: big ? 11 : 10, color: "var(--ink-dim, #6b6358)" }}>{s.at}</span>
              </span>
            </button>
          );
        })}
      </div>

      {/* Scrub (secundario): mover el índice sobre los pasos ya cargados — O(1), read-only. */}
      {steps.length > 1 && (
        <div style={{ marginTop: big ? 16 : 8 }}>
          <input
            type="range" min={0} max={steps.length - 1} value={clampScrub(scrubIndex, steps.length)}
            onChange={(e) => setScrub(Number(e.target.value))}
            aria-label="Rebobinar el timeline del agente"
            style={{ width: "100%" }}
          />
          {cur && cur.content && (
            <pre style={{ background: "var(--card, #fbf9f4)", border: "1px solid var(--line)", borderRadius: 6, padding: big ? 12 : 8, fontSize: big ? 12 : 11, fontFamily: "var(--mono)", overflowX: "auto", margin: "6px 0 0", maxHeight: big ? "24vh" : 160 }}>{cur.content}</pre>
          )}
        </div>
      )}
    </div>
  );
}

export function AgentTimeline({ task }: { task: OrchTask }) {
  const [logs, setLogs] = useState<OrchLogEntry[]>([]);
  // Pasos vivos acumulados (provisionales) por transición observada. Se mergean sobre la base canónica.
  const [liveSteps, setLiveSteps] = useState<TimelineStep[]>([]);
  const [scrubIndex, setScrub] = useState<number>(Number.MAX_SAFE_INTEGER); // arranca pegado al final (vivo)
  const [pinned, setPinned] = useState(false); // si el usuario hizo scrub, NO auto-saltamos al final
  const [cinema, setCinema] = useState(false);
  const lastState = useRef<OrchTask["state"]>(task.state);
  // Generation-guard (audit codex 035#1): cada recarga toma un nº monotónico; sólo la MÁS RECIENTE
  // puede escribir `logs`. Evita que una respuesta vieja (que resolvió tarde) pise un snapshot nuevo.
  const reqGen = useRef(0);

  // Carga (o recarga) el log-history persistido (cap 30 del backend). Best-effort: captura primero.
  const reloadLogs = useCallback(async () => {
    const myGen = ++reqGen.current;
    try {
      await invoke("orchestration_capture_log", { taskId: task.id }).catch(() => {});
      const entries = await invoke<OrchLogEntry[]>("orchestration_log_history", { taskId: task.id, limit: 30 });
      if (myGen === reqGen.current) setLogs(entries ?? []); // sólo la recarga más reciente escribe
    } catch { /* la tarea pudo salir; el próximo evento/refresh lo refleja */ }
  }, [task.id]);

  // Al cambiar de tarea, invalidar respuestas en vuelo de la tarea anterior y resetear el estado vivo.
  useEffect(() => {
    reqGen.current++; // invalida cualquier respuesta vieja en vuelo
    lastState.current = task.state;
    setLiveSteps([]);
    reloadLogs();
  }, [task.id, reloadLogs]); // eslint-disable-line react-hooks/exhaustive-deps

  // LIVE: SÓLO este timeline (abierto) se suscribe, FILTRADO a esta tarea (finding council ALTA).
  // Cada evento TaskChanged de esta tarea: (1) registra la transición observada como paso provisional,
  // (2) re-pide el log-history (el snapshot canónico supersede al provisional vía mergeLiveSteps).
  useAppEvent("TaskChanged", (data) => {
    if (data.id !== task.id) return;
    const next = data.state as OrchTask["state"];
    const prev = lastState.current;
    const step = liveStepFromTransition(prev, next, new Date().toISOString().replace("T", " ").slice(0, 19), task.cli_kind);
    lastState.current = next;
    if (step) {
      setLiveSteps((prevSteps) => {
        const merged = [...prevSteps, step];
        return merged.length > LIVE_CAP ? merged.slice(merged.length - LIVE_CAP) : merged;
      });
    }
    void reloadLogs();
  });

  // El timeline final: base canónica + pasos vivos, de-duplicado/acotado/re-indexado (puro, testeado).
  const steps = useMemo(
    () => mergeLiveSteps(buildTimeline(task, logs), liveSteps, LIVE_CAP),
    [task, logs, liveSteps],
  );

  const isLive = task.state === "running" || task.state === "pending" || task.state === "awaiting_review";
  const newestIndex = steps.length - 1;
  // Si el usuario NO está rebobinando, el scrub sigue pegado al paso más nuevo (sensación "en vivo").
  const effectiveScrub = pinned ? scrubIndex : newestIndex;
  const onScrub = (i: number) => { setPinned(i < newestIndex); setScrub(i); };

  // Modo cine: usa el <Modal> del proyecto → focus-trap + Esc + aria-hidden del fondo + scroll-lock
  // + restauración de foco (a11y, audit deepseek 035#3). No re-implementamos el trap a mano.

  return (
    <div style={{ marginTop: 6 }}>
      <div style={{ display: "flex", justifyContent: "flex-end", marginBottom: 4 }}>
        <button
          type="button"
          onClick={() => setCinema(true)}
          title="Ver el timeline en pantalla completa (modo cine)"
          style={{ cursor: "pointer", padding: "3px 9px", fontSize: 12, borderRadius: 6, border: "1px solid var(--line, rgba(0,0,0,.15))", background: "var(--bg, #faf7f0)", color: "var(--ink, #1c1814)", fontFamily: "var(--body)" }}
        >
          ⤢ Cine
        </button>
      </div>

      <TimelineBody steps={steps} scrubIndex={effectiveScrub} setScrub={onScrub} live={isLive} newestIndex={newestIndex} />

      {cinema && (
        <Modal
          title={task.title}
          subtitle={`${task.cli_kind ?? task.mode ?? "?"} · ${task.branch}`}
          ariaLabel={`Timeline en vivo · ${task.title}`}
          onClose={() => setCinema(false)}
          maxWidth="min(1100px,94vw)"
        >
          <TimelineBody steps={steps} scrubIndex={effectiveScrub} setScrub={onScrub} live={isLive} newestIndex={newestIndex} big />
        </Modal>
      )}
    </div>
  );
}
