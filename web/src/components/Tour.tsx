// web/src/components/Tour.tsx — 016 US4 (T041/T042/T077/T078) · overlay de tour guiado.
//
// Thin render sobre la FSM PURA (lib/tour.ts). NO es un Modal: es un overlay propio porque resalta
// regiones REALES de la UI de fondo (un Modal las taparía). Por eso implementamos a11y a mano (T078):
//   - focus trap dentro del popover del tour + restaurar el foco al cerrar.
//   - ESC / panic-exit global (cierra el tour desde cualquier foco).
//   - `prefers-reduced-motion`: sin transición de highlight si el usuario lo pide.
// Targets por `data-tour="<id>"` (estables). Si el target falta → la FSM espera con presupuesto
// ACOTADO (no loop) y, agotado, SALTA el paso (fallback). Sincroniza progreso por eventBus
// (CommandExecuted como heartbeat de avance — reusa la unión existente sin agregar variantes).
// Persistencia: al terminar/skipear marca firstRun done (no auto-relanza). Relanzable desde Help.

import { useEffect, useMemo, useReducer, useRef, useState } from "react";
import {
  tourReducer,
  initialState,
  visibleSteps,
  shouldFallbackAdvance,
  markFirstRunDone,
  type TourStep,
} from "../lib/tour";
import { FIRST_RUN_TOUR } from "../data/tours";
import { useT } from "../lib/i18n";
import { trackEvent } from "../lib/telemetry";

export interface TourProps {
  /** Tour corriendo (lo controla el Shell: primer arranque o relanzamiento desde Help). */
  active: boolean;
  /** Cerrar el tour (skip/finish/ESC). El Shell pone active=false. */
  onClose: () => void;
  /** Navegar el deeplink de un paso antes de resaltarlo (router interno del Shell). */
  onNavigate?: (deeplink: string) => void;
  /** Pasos del tour (default = tour de primeros pasos). Inyectable para tests. */
  steps?: TourStep[];
}

function prefersReducedMotion(): boolean {
  try {
    return typeof matchMedia !== "undefined" && matchMedia("(prefers-reduced-motion: reduce)").matches;
  } catch {
    return false;
  }
}

export function Tour({ active, onClose, onNavigate, steps = FIRST_RUN_TOUR }: TourProps) {
  const t = useT();
  const visible = useMemo(() => visibleSteps(steps), [steps]);
  const [state, dispatch] = useReducer(tourReducer, undefined, initialState);
  const [rect, setRect] = useState<DOMRect | null>(null);
  const popoverRef = useRef<HTMLDivElement>(null);
  const prevFocus = useRef<HTMLElement | null>(null);
  const reduced = prefersReducedMotion();

  const total = visible.length;
  const step: TourStep | undefined = visible[state.index];

  // Arrancar/parar la FSM según `active`.
  useEffect(() => {
    if (active) dispatch({ type: "start" });
  }, [active]);

  // Al terminar (completed/skipped) → persistir firstRun + telemetry (allowlisted) + cerrar (no auto-relanza).
  useEffect(() => {
    if (state.status === "completed") {
      markFirstRunDone();
      trackEvent("tour_completed", {});
      onClose();
    } else if (state.status === "skipped") {
      markFirstRunDone();
      // SÓLO el índice del paso (número, no contenido) — allowlisted.
      trackEvent("tour_skipped", { step: state.index });
      onClose();
    }
  }, [state.status, state.index, onClose]);

  // Guardar/restaurar el foco (T078). Al activarse, recordamos el foco previo y enfocamos el popover.
  useEffect(() => {
    if (!active) return;
    prevFocus.current = (document.activeElement as HTMLElement) ?? null;
    const id = window.setTimeout(() => popoverRef.current?.focus(), 0);
    return () => {
      window.clearTimeout(id);
      prevFocus.current?.focus?.();
    };
  }, [active]);

  // Navegar el deeplink del paso (si lo tiene) antes de resaltar.
  useEffect(() => {
    if (!active || !step?.deeplink) return;
    onNavigate?.(step.deeplink);
  }, [active, step?.deeplink, onNavigate]);

  // Resolver el target del paso con presupuesto ACOTADO (no loop): tickeamos buscando el
  // `[data-tour]`; si aparece → targetFound (resalta); si se agota el presupuesto → fallback advance.
  useEffect(() => {
    if (!active || !step || state.status === "completed" || state.status === "skipped") return;
    let stopped = false;
    let interval = 0;
    const tick = () => {
      if (stopped) return;
      const el = document.querySelector<HTMLElement>(`[data-tour="${step.targetId}"]`);
      if (el) {
        setRect(el.getBoundingClientRect());
        dispatch({ type: "targetFound" });
        // S1 (audit): target presente → parar de tickear ESTE paso. El status sigue
        // "running", así que el effect no se re-ejecuta; sin esto el setInterval seguiría
        // re-querying cada 120ms todo el paso. Cortamos el interval acá.
        stopped = true;
        if (interval) window.clearInterval(interval);
        return;
      }
      dispatch({ type: "targetMissing" });
    };
    tick();
    if (!stopped) interval = window.setInterval(tick, 120);
    return () => { stopped = true; window.clearInterval(interval); };
  }, [active, step?.targetId, state.index, state.status]);

  // Presupuesto agotado esperando el target → avanzar (fallback) para no colgar el tour. FR-016.
  useEffect(() => {
    if (active && shouldFallbackAdvance(state)) {
      dispatch({ type: "next", total });
    }
  }, [active, state, total]);

  // ESC / panic-exit global (T077): cerrar el tour desde cualquier foco.
  useEffect(() => {
    if (!active) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") { e.preventDefault(); dispatch({ type: "skip" }); }
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [active]);

  // Focus trap dentro del popover (T078): Tab/Shift+Tab ciclan dentro de los botones del tour.
  function onPopoverKeyDown(e: React.KeyboardEvent) {
    if (e.key !== "Tab") return;
    const focusables = popoverRef.current?.querySelectorAll<HTMLElement>("button:not([disabled])");
    if (!focusables || focusables.length === 0) return;
    const first = focusables[0];
    const last = focusables[focusables.length - 1];
    const act = document.activeElement;
    if (e.shiftKey && act === first) { e.preventDefault(); last.focus(); }
    else if (!e.shiftKey && act === last) { e.preventDefault(); first.focus(); }
  }

  if (!active || !step || total === 0) return null;
  if (state.status !== "running") return null; // sólo renderizamos en running (waiting/terminal = nada)

  // Posición del popover: cerca del target si lo conocemos; centrado abajo si no.
  const highlightStyle: React.CSSProperties = rect
    ? {
        top: rect.top - 6,
        left: rect.left - 6,
        width: rect.width + 12,
        height: rect.height + 12,
        transition: reduced ? "none" : "all 160ms ease",
      }
    : { display: "none" };
  const popoverStyle: React.CSSProperties = rect
    ? { top: Math.min(rect.bottom + 12, window.innerHeight - 200), left: Math.min(rect.left, window.innerWidth - 360) }
    : { bottom: 24, left: "50%", transform: "translateX(-50%)" };

  const atFirst = state.index === 0;
  const atLast = state.index === total - 1;

  return (
    <div className="fxc-tour" data-reduced={reduced ? "1" : "0"}>
      {/* scrim translúcido que NO bloquea el highlight; clic fuera = panic-exit. */}
      <div className="fxc-tour__scrim" onClick={() => dispatch({ type: "skip" })} aria-hidden="true" />
      <div className="fxc-tour__highlight" style={highlightStyle} aria-hidden="true" />
      <div
        ref={popoverRef}
        className="fxc-tour__popover"
        style={popoverStyle}
        role="dialog"
        aria-modal="false"
        aria-label={t(step.titleKey as never)}
        tabIndex={-1}
        onKeyDown={onPopoverKeyDown}
      >
        <div className="fxc-tour__progress">{t("tour.progress", { current: state.index + 1, total })}</div>
        <h3 className="fxc-tour__title">{t(step.titleKey as never)}</h3>
        <p className="fxc-tour__body">{t(step.bodyKey as never)}</p>
        <div className="fxc-tour__actions">
          <button type="button" className="fxc-btn" onClick={() => dispatch({ type: "skip" })}>
            {t("tour.skip")}
          </button>
          <span style={{ flex: 1 }} />
          {!atFirst && (
            <button type="button" className="fxc-btn" onClick={() => dispatch({ type: "back" })}>
              {t("tour.back")}
            </button>
          )}
          {atLast ? (
            <button type="button" className="fxc-btn fxc-btn--primary" onClick={() => dispatch({ type: "finish" })}>
              {t("tour.finish")}
            </button>
          ) : (
            <button type="button" className="fxc-btn fxc-btn--primary" onClick={() => dispatch({ type: "next", total })}>
              {t("tour.next")}
            </button>
          )}
        </div>
      </div>
    </div>
  );
}
