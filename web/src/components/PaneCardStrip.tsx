// 047 FR-002 — PaneCard: cabecera contextual de ~28px sobre el PTY. Muestra modo · tokens · tiempo
// (uptime) · estado (running / idle / awaiting). Cuando el pane RECLAMA decisión humana (priority
// `needs_input` de la cola de atención 030-034) muestra un overlay "Aprobar".
//
// FOCO HUMANO (030-034) — NON-NEGOTIABLE: el botón "Aprobar" es una acción HUMANA explícita; sólo
// ENFOCA el pane (lleva al humano a decidir ahí), NUNCA aprueba/auto-dispara nada por su cuenta.
// El estado deriva de datos del backend; el strip no muta nada.
import { useEffect, useState } from "react";
import { usePaneAttention } from "../hooks/useAttention";

export type PaneRunState = "running" | "idle" | "awaiting";

function fmtUptime(ms: number): string {
  if (ms < 0) ms = 0;
  const s = Math.floor(ms / 1000);
  if (s < 60) return `${s}s`;
  const m = Math.floor(s / 60);
  if (m < 60) return `${m}m ${s % 60}s`;
  const h = Math.floor(m / 60);
  return `${h}h ${m % 60}m`;
}

const STATE_META: Record<PaneRunState, { label: string; color: string; glyph: string }> = {
  running: { label: "activo", color: "var(--ok, #2ec98a)", glyph: "●" },
  idle: { label: "en reposo", color: "var(--muted, #6c7b91)", glyph: "○" },
  awaiting: { label: "espera tu decisión", color: "var(--amber, #e0a548)", glyph: "▲" },
};

export function PaneCardStrip({
  paneId,
  modeLabel,
  modeColor,
  tokens,
  bornAt,
  hasLiveProcess,
  onApprove,
}: {
  paneId: string;
  /** Label del modo. `null` = no mostrarlo en el strip (ya lo muestra el dropdown del header — 066). */
  modeLabel: string | null;
  modeColor: string;
  /** tokens formateados (ej "12.4k") o null si no aplica. */
  tokens: string | null;
  /** epoch ms de nacimiento del pane (0 = desconocido → no mostramos uptime). */
  bornAt: number;
  /** Señal INFORMATIVA de "proceso vivo" (no autoritativa): hoy el front no expone un estado
   *  de PTY vivo/muerto por-pane, así que un pane de terminal se trata como vivo hasta cerrarse.
   *  La detección real de "terminó" vive en la cola de atención (has_result) / done_detection. */
  hasLiveProcess: boolean;
  /** acción HUMANA: llevar el foco visual a este pane para decidir. NUNCA aprueba por su cuenta. */
  onApprove: () => void;
}) {
  const attention = usePaneAttention(paneId);
  const awaiting = attention === "needs_input";

  // Uptime: tick local de 1s (sólo cuando hay nacimiento conocido y proceso vivo).
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (bornAt === 0 || !hasLiveProcess) return;
    const id = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(id);
  }, [bornAt, hasLiveProcess]);

  const state: PaneRunState = awaiting ? "awaiting" : hasLiveProcess ? "running" : "idle";
  const sm = STATE_META[state];
  const uptime = bornAt > 0 && hasLiveProcess ? fmtUptime(now - bornAt) : null;

  return (
    <div
      className={`pane-card-strip ${awaiting ? "awaiting" : ""}`}
      role="group"
      aria-label={`Estado del panel: ${sm.label}`}
    >
      {modeLabel && (
        <span className="pcs-mode" style={{ color: modeColor }} title="Modo del agente">
          <span className="pcs-dot" aria-hidden="true" style={{ background: modeColor }} />
          {modeLabel}
        </span>
      )}
      {tokens && (
        <span className="pcs-tokens" title="Tokens de la sesión" style={{ fontVariantNumeric: "tabular-nums" }}>
          {tokens} tok
        </span>
      )}
      {uptime && (
        <span className="pcs-uptime" title="Tiempo activo" style={{ fontVariantNumeric: "tabular-nums" }}>
          {uptime}
        </span>
      )}
      <span className="pcs-state" style={{ color: sm.color }} title={sm.label}>
        <span aria-hidden="true">{sm.glyph}</span> {sm.label}
      </span>
      {awaiting && (
        <button
          type="button"
          className="pcs-approve"
          onClick={(e) => {
            e.stopPropagation();
            onApprove();
          }}
          title="Te lleva a este panel para revisar y decidir — vos aprobás, no se aprueba solo"
        >
          Ir a aprobar…
        </button>
      )}
    </div>
  );
}
