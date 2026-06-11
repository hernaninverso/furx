// web/src/components/DetachedWindow.tsx — 018 Fase 2 US2 (T023)
//
// Entry de una ventana DETACHED. Una webview secundaria (`index.html?window_key=detached-N`)
// renderiza SÓLO el subárbol de SU ventana (no el chrome de la app Main). Reusa el mismo
// `WorkspaceView` + adapter dockview del SSOT: lee `LayoutConfigV1`, encuentra su `WindowLayout`
// por `window_key`, y monta cada Leaf como un `Terminal` ligado al MISMO proceso PTV vivo
// (por panel_id). El lease (`pty_lease_attach` con SU label, derivado server-side) garantiza que
// el panel_id se monta en UNA sola webview: al detachar, el force-detach versionado desplaza el
// binding de la ventana Main hacia ésta — SIN tocar el proceso (constitución VI).
//
// Tema V3 dark+light + anti-FOUC: la ventana carga la MISMA `index.html`, cuyo script inline ya
// aplica la clase de tema antes del primer paint (no hay flash). No se duplica nada acá.
//
// BYOK (F-I): esta webview NUNCA recibe keys. Los comandos `Risk::Credential` se DENIEGAN
// off-Main por el gate central (`window_byok::check_window_command` en lib.rs). El Terminal sólo
// usa pty_write/pty_capture_history (Safe) sobre su binding vigente.

import { useEffect, useState, type ReactNode } from "react";
import { WorkspaceView } from "./WorkspaceView.tsx";
import { DetachButton } from "./DetachButton.tsx";
import { Terminal } from "../Terminal.tsx";
import { resolveWindowLabel } from "../lib/windowManager.ts";

/** Deriva el `mode` y `cwd` de un Leaf detached desde su PanelDescriptor (panelType + params). */
function deriveTerminalProps(panelType: string, params: unknown): { mode: string; cwd?: string } {
  const p = (params && typeof params === "object" ? (params as Record<string, unknown>) : {}) ?? {};
  const cwd = typeof p.cwd === "string" ? p.cwd : undefined;
  return { mode: panelType || "zsh", cwd };
}

export function DetachedWindow() {
  // Resolver el window_key de ESTA webview desde la URL (espejo del label que Rust deriva
  // server-side). Estable por montaje de la ventana.
  const [label] = useState<string>(() =>
    resolveWindowLabel(typeof window !== "undefined" ? window.location.search : ""),
  );

  // Re-fit del terminal al redimensionar la ventana (el ResizeObserver del Terminal ya cubre
  // el contenedor; este efecto sólo asegura un layout inicial estable).
  useEffect(() => {
    document.title = `Furx — ${label}`;
  }, [label]);

  return (
    <div className="furx-base detached-window" style={{ height: "100vh", width: "100vw", display: "flex", flexDirection: "column" }}>
      <div style={{ flex: 1, minHeight: 0 }}>
        <WorkspaceView
          windowLabel={label}
          renderLeaf={({ panelId, panelType, params, leaseWindowLabel, leaseMountInstanceId }): ReactNode => {
            const { mode, cwd } = deriveTerminalProps(panelType, params);
            return (
              <div style={{ height: "100%", width: "100%", display: "flex", flexDirection: "column" }}>
                <div className="pane-header" style={{ display: "flex", alignItems: "center", gap: 6, padding: "2px 6px" }}>
                  <span className="lbl" style={{ flex: 1, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
                    {panelId} · {mode}
                  </span>
                  {/* Re-attach: reata a Main y cierra esta ventana (sin matar el proceso). */}
                  <DetachButton panelId={panelId} windowLabel={label} />
                </div>
                <div style={{ flex: 1, minHeight: 0 }}>
                  <Terminal
                    paneId={panelId}
                    mode={mode}
                    cwd={cwd}
                    leaseWindowLabel={leaseWindowLabel}
                    leaseMountInstanceId={leaseMountInstanceId}
                  />
                </div>
              </div>
            );
          }}
        />
      </div>
    </div>
  );
}
