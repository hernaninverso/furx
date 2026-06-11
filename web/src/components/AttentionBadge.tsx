// 032 U1 + 033 U1 — Badge de la cola de atención (visible) + popover INTERACTIVO. La cola la puebla el
// poller backend; el badge la hace visible (conteo urgente/informativo) y, al clic, abre un popover
// con los panes en cola y dos acciones por pane: "ir" (enfoca VISUALMENTE, NUNCA el foco del mic) y
// "descartar" (attention_ack). Accesible: ícono+texto+aria (no sólo color). Reactivo: poll de
// `attention_list` (~2s) + refresh ante AppEvents; fail-safe a "vacío" si el backend falla.
import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "../lib/invoke";
import { usePolling } from "../hooks/usePolling";
import { useAppEvent } from "../lib/eventBus";

type Priority = "needs_input" | "has_result";
interface AttentionEntry {
  seq: number;
  pane_id: string;
  priority: Priority;
  attended: boolean;
}

const cap = (n: number) => (n > 9 ? "9+" : String(n));

export function AttentionBadge({
  labelOf,
  onFocus,
}: {
  labelOf?: (pid: string) => string;
  onFocus?: (paneId: string) => void;
}) {
  const [entries, setEntries] = useState<AttentionEntry[]>([]);
  const [open, setOpen] = useState(false);
  const rootRef = useRef<HTMLSpanElement>(null);

  const refresh = useCallback(async () => {
    try {
      const list = await invoke<AttentionEntry[]>("attention_list", {});
      setEntries(Array.isArray(list) ? list : []);
    } catch {
      setEntries([]); // fail-safe: nunca mostrar datos obsoletos si el backend falla
    }
  }, []);

  usePolling(refresh, { intervalMs: 2000, runOnMount: true });
  useAppEvent("TaskChanged", refresh);
  useAppEvent("AgentStateChanged", refresh);
  useAppEvent("CommandExecuted", refresh);

  // Si la cola se vacía, cerrar el popover (sin setState-en-render; audit codex). El badge además se
  // oculta abajo cuando no hay entradas.
  useEffect(() => {
    if (entries.length === 0) setOpen(false);
  }, [entries.length]);

  // Cerrar el popover con clic afuera / Escape (FR-1.4).
  useEffect(() => {
    if (!open) return;
    const onDocClick = (e: MouseEvent) => {
      if (rootRef.current && !rootRef.current.contains(e.target as Node)) setOpen(false);
    };
    const onKey = (e: KeyboardEvent) => { if (e.key === "Escape") setOpen(false); };
    document.addEventListener("mousedown", onDocClick);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("mousedown", onDocClick);
      document.removeEventListener("keydown", onKey);
    };
  }, [open]);

  const urgent = entries.filter((e) => e.priority === "needs_input").length;
  const info = entries.filter((e) => e.priority === "has_result").length;
  if (urgent + info === 0) return null; // cola vacía → badge oculto (el cierre del popover va en el effect)

  const label = (pid: string) => (labelOf ? labelOf(pid) : pid);

  const goTo = (pid: string) => {
    // "ir": foco VISUAL al pane (NO mueve el foco del mic). El popover se cierra.
    onFocus?.(pid);
    setOpen(false);
  };
  const dismiss = async (seq: number) => {
    // "Descartar" NO cierra el popover a propósito: permite triagear varios seguidos. El popover
    // se cierra solo cuando la cola se vacía (effect) o por clic-afuera/Escape/"Ir".
    try {
      await invoke<boolean>("attention_ack", { seq });
    } catch {
      // si el ack falla, el pane permanece en la cola (la lista se refresca igual)
    }
    refresh();
  };

  return (
    <span className="attention-badge" ref={rootRef}>
      <button
        type="button"
        className="attention-badge__btn"
        aria-haspopup="true"
        aria-expanded={open}
        aria-label={`Cola de atención: ${urgent} urgente(s), ${info} informativo(s). Abrir lista.`}
        title="Panes que te reclaman — clic para ver y actuar"
        onClick={() => setOpen((v) => !v)}
      >
        {urgent > 0 && <span className="attention-badge__urgent">⚠ {cap(urgent)}</span>}
        {urgent > 0 && info > 0 && <span className="attention-badge__sep"> · </span>}
        {info > 0 && <span className="attention-badge__info">ℹ {cap(info)}</span>}
      </button>
      {open && (
        <div className="attention-popover" role="list" aria-label="Panes en cola de atención">
          {entries.map((e) => (
            <div key={e.seq} className="attention-popover__row" role="listitem">
              <span
                className={
                  e.priority === "needs_input"
                    ? "attention-badge__urgent"
                    : "attention-badge__info"
                }
                aria-hidden="true"
              >
                {e.priority === "needs_input" ? "⚠" : "ℹ"}
              </span>
              <span className="attention-popover__name">{label(e.pane_id)}</span>
              <button type="button" className="attention-popover__act" onClick={() => goTo(e.pane_id)}>
                Ir
              </button>
              <button
                type="button"
                className="attention-popover__act"
                onClick={() => dismiss(e.seq)}
              >
                Descartar
              </button>
            </div>
          ))}
        </div>
      )}
    </span>
  );
}
