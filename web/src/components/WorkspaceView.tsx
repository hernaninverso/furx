// web/src/components/WorkspaceView.tsx — 018 Fase 2 US1 (T010/T011/T012/T014)
//
// Renderiza el Workspace desde `LayoutConfigV1` (árbol Split/Tabs/Leaf) vía dockview,
// reemplazando la grilla 2×2 legacy cuando el flag `newWorkspace` está ON. Cada Leaf
// monta su pane REAL por panel_id (vía la prop `renderLeaf`, que Shell cablea con el
// componente Pane existente → reusa procesos vivos, no los respawnea).
//
// SSOT: `LayoutConfigV1` (Rust/DB). dockview es SÓLO el motor de render (council 5/5).
// Flujo:
//   - Al montar / cuando llega `LayoutChanged` (respetando seq del event bus) →
//     `loadLayoutConfig()` (que MIGRA legacy server-side, T014) → `toDockview(plan)` →
//     se aplica imperativamente a la DockviewApi (re-hidratación).
//   - Cada Leaf, al montarse en dockview, hace `pty_lease_attach` (T060) y al
//     desmontarse `pty_lease_detach` (T061) — el binding UI↔PTY queda único por panel_id.
//   - NUNCA se persiste el JSON interno de dockview (no split-brain).

import { DockviewReact, type DockviewReadyEvent, type IDockviewPanelProps } from "dockview";
import { useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { loadLayoutConfig, type LayoutConfigV1, type PanelLayoutNode, MAIN_WINDOW_KEY } from "../lib/layoutConfig.ts";
import { windowFor } from "../lib/windowManager.ts";
import { toDockview } from "../lib/dockviewAdapter.ts";
import { useAppEvent } from "../lib/eventBus.ts";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import "dockview/dist/styles/dockview.css";
import "./dockview-furx.css";

/** El label de ESTA webview. En la ola 1 siempre "main"; US2 lo lee del ?window_key. */
export const THIS_WINDOW_LABEL = MAIN_WINDOW_KEY;

/** Genera un mount_instance_id único por montaje del componente (T060). */
function newMountId(): string {
  return `mnt-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 8)}`;
}

/** Props del Workspace. `renderLeaf` mapea un panel_id (+ params) al pane real (Shell lo
 *  cablea con su componente Pane → reusa el proceso vivo de ese panel_id). */
export interface WorkspaceViewProps {
  /** workspace a renderizar (default). */
  workspaceId?: string;
  /** mapea un Leaf a su pane real. Recibe panel_id, panel_type, params y el binding del
   *  lease (windowLabel + mountInstanceId del attach de ESTE montaje) para que el pane los
   *  pase a `pty_write` (HIGH-1 audit: cierra el fail-open universal). */
  renderLeaf: (args: {
    panelId: string;
    panelType: string;
    params: unknown;
    leaseWindowLabel: string;
    leaseMountInstanceId: string;
  }) => ReactNode;
  /** label de la ventana (ola 1 = main). */
  windowLabel?: string;
}


export function WorkspaceView({ workspaceId, renderLeaf, windowLabel = THIS_WINDOW_LABEL }: WorkspaceViewProps) {
  const [cfg, setCfg] = useState<LayoutConfigV1 | null>(null);
  const [error, setError] = useState<string | null>(null);
  const apiRef = useRef<DockviewReadyEvent["api"] | null>(null);
  // guarda el renderLeaf más reciente sin re-crear los componentes de dockview.
  const renderLeafRef = useRef(renderLeaf);
  renderLeafRef.current = renderLeaf;

  // (T014) Carga inicial: loadLayoutConfig migra legacy server-side (sin perder panes/procesos).
  useEffect(() => {
    let alive = true;
    loadLayoutConfig(workspaceId)
      .then((c) => {
        if (alive) setCfg(c);
      })
      .catch((e) => {
        if (alive) setError(String(e));
      });
    return () => {
      alive = false;
    };
  }, [workspaceId]);

  // (T012) Re-hidratación por LayoutChanged respetando seq (el eventBus ya descarta seq viejos).
  useAppEvent("LayoutChanged", () => {
    loadLayoutConfig(workspaceId)
      .then(setCfg)
      .catch((e) => setError(String(e)));
  });

  // Componente único de dockview que delega al renderLeaf de Shell + maneja el lease.
  const components = useMemo(
    () => ({
      furxPane: (props: IDockviewPanelProps<{ panelType: string; rawParams: unknown }>) => {
        const panelId = props.api.id;
        const params = props.params ?? { panelType: "terminal", rawParams: null };
        // mountId ESTABLE por montaje (HIGH-1 audit): el MISMO id que se usa para el attach
        // se pasa al pane para que su `pty_write` declare el binding vigente. Si se regenerase
        // por render, el write traería un mount_instance ≠ al del lease → fail-closed lo
        // descartaría. useRef lo fija al primer render del montaje.
        const mountIdRef = useRef<string>("");
        if (!mountIdRef.current) mountIdRef.current = newMountId();
        const mountId = mountIdRef.current;
        // (should-fix audit ola-1) `lostBinding`: esta vista perdió el binding del pane (otra
        // ventana lo reclamó vía `furx:lease-lost`). El input ya está fail-closed server-side
        // (is_current=false), pero lo reflejamos en la UI (read-only) para no teclear al vacío.
        const [lostBinding, setLostBinding] = useState(false);
        // T060/T061 — lease attach al montar, detach al desmontar. NUNCA toca el proceso.
        // `windowLabel` está en las deps (should-fix): si una vista re-resuelve su ventana, el
        // lease se re-attacha con el label nuevo en vez de quedar stale.
        useEffect(() => {
          setLostBinding(false);
          // El attach devuelve la window_label DESPLAZADA (señal `displaced`, should-fix). Si es
          // OTRA ventana, ESTA vista ganó el binding (tomó el pane de allí); la ventana vieja
          // recibe `furx:lease-lost` y se pone read-only. Lo logueamos (no se descarta en silencio).
          invoke<string | null>("pty_lease_attach", { panelId, mountInstanceId: mountId })
            .then((displaced) => {
              if (displaced && displaced !== windowLabel) {
                console.debug(`[workspace] panel ${panelId} re-bound from window ${displaced} → ${windowLabel}`);
              }
            })
            .catch(() => {
              /* fuera de Tauri / no fatal: el render igual procede */
            });
          // Escuchar si OTRA ventana nos quita el binding de ESTE panel → read-only.
          let off: (() => void) | undefined;
          listen<{ panel_id: string }>("furx:lease-lost", (ev) => {
            if (ev.payload?.panel_id === panelId) setLostBinding(true);
          })
            .then((u) => {
              off = u;
            })
            .catch(() => {});
          return () => {
            off?.();
            invoke("pty_lease_detach", { panelId, mountInstanceId: mountId }).catch(() => {});
          };
        }, [panelId, windowLabel]);
        return (
          <div
            className="dv-react-part"
            style={{ height: "100%", width: "100%", position: "relative", pointerEvents: lostBinding ? "none" : undefined, opacity: lostBinding ? 0.55 : 1 }}
            aria-disabled={lostBinding}
          >
            {lostBinding && (
              <div
                style={{ position: "absolute", inset: 0, zIndex: 5, display: "flex", alignItems: "center", justifyContent: "center", background: "var(--bg-1, rgba(0,0,0,0.35))", color: "var(--ink-2)", fontSize: 13, padding: 12, textAlign: "center" }}
              >
                Este panel se movió a otra ventana. El proceso sigue vivo allí.
              </div>
            )}
            {renderLeafRef.current({
              panelId,
              panelType: params.panelType,
              params: params.rawParams,
              leaseWindowLabel: windowLabel,
              leaseMountInstanceId: mountId,
            })}
          </div>
        );
      },
    }),
    [windowLabel],
  );

  // Aplica el árbol vigente a la DockviewApi (re-hidratación determinista).
  function hydrate(api: DockviewReadyEvent["api"], config: LayoutConfigV1) {
    const root = windowFor(config, windowLabel);
    // Limpiar la disposición previa (sin tocar procesos: dockview sólo destruye UI).
    api.clear();
    if (!root) return;
    const plan = toDockview(root);
    for (const op of plan) {
      const position = op.position
        ? op.position.type === "split"
          ? { referencePanel: op.position.referencePanelId, direction: op.position.direction }
          : { referenceGroup: groupOf(api, op.position.referenceGroupOf) ?? undefined }
        : undefined;
      const panel = api.addPanel({
        id: op.panelId,
        component: "furxPane",
        params: { panelType: op.panelType, rawParams: op.params },
        position: position as never,
      });
      if (op.active) panel.api.setActive();
    }
  }

  function groupOf(api: DockviewReadyEvent["api"], panelId: string) {
    const p = api.getPanel(panelId);
    return p ? p.group : null;
  }

  const onReady = (event: DockviewReadyEvent) => {
    apiRef.current = event.api;
    if (cfg) hydrate(event.api, cfg);
  };

  // Re-hidratar cuando cambia el cfg (LayoutChanged / carga inicial) y la api existe.
  useEffect(() => {
    if (apiRef.current && cfg) hydrate(apiRef.current, cfg);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [cfg]);

  if (error) {
    return (
      <div className="furx-base" style={{ padding: 16, color: "var(--ink-2)" }}>
        <p className="lbl">Workspace</p>
        <p>No se pudo cargar el layout: {error}. Se mantienen los procesos en background.</p>
      </div>
    );
  }

  return (
    <div className="dockview-theme-furx" style={{ height: "100%", width: "100%" }}>
      <DockviewReact components={components} onReady={onReady} />
    </div>
  );
}
