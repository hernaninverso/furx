// web/src/components/DetachButton.tsx — 018 Fase 2 US2 (T023)
//
// Botón por-pane para SACAR el pane a una ventana propia (detach) o, si el pane YA vive en una
// ventana detached, REATARLO a Main (close de su ventana). NUNCA mata el proceso: delega en los
// comandos `window_open_detached` / `window_close` (toda la mutación del árbol es server-side,
// unidireccional). El proceso PTY sigue vivo; su binding migra vía el lease.
//
// - En la ventana Main → acción "detach" (↗). Llama `detachPane(panelId)` → Rust mueve el Leaf a
//   una WindowLayout{Detached} + abre la webview; el LayoutChanged hace que Main deje de mostrarlo.
// - En una ventana detached → acción "re-attach" (↙). Llama `closeWindow(thisWindowLabel)` → reata
//   el subárbol a Main y cierra esta ventana (mismo camino que la X del SO).

import { detachPane, closeWindow, isMainWindow } from "../lib/windowManager.ts";

export interface DetachButtonProps {
  /** panel_id del pane (== el id del Leaf en el SSOT). */
  panelId: string;
  /** label de la ventana donde vive este pane (main / detached-N). */
  windowLabel: string;
  /** workspace (default si se omite). */
  workspaceId?: string;
}

export function DetachButton({ panelId, windowLabel, workspaceId }: DetachButtonProps) {
  const onMain = isMainWindow(windowLabel);
  const onClick = (e: React.MouseEvent) => {
    e.stopPropagation();
    if (onMain) {
      // Detach: el proceso NO se toca; sólo migra de ventana.
      void detachPane(panelId, workspaceId).catch((err) => console.warn("detachPane failed", err));
    } else {
      // Re-attach: reata a Main y cierra esta ventana (sin matar el proceso).
      void closeWindow(windowLabel, workspaceId).catch((err) => console.warn("closeWindow failed", err));
    }
  };
  return (
    <button
      className="pane-detach"
      onClick={onClick}
      title={onMain ? "Sacar panel a su propia ventana" : "Reatar panel a la ventana principal"}
      aria-label={onMain ? `Sacar ${panelId} a una ventana propia` : `Reatar ${panelId} a la ventana principal`}
      style={{ marginRight: 4 }}
    >
      {onMain ? "↗" : "↙"}
    </button>
  );
}
