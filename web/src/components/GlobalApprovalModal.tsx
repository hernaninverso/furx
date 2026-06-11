// web/src/components/GlobalApprovalModal.tsx — 015 T015 · modal GLOBAL de aprobación.
//
// Montado UNA vez en el Shell. Escucha la cola de approvalBus (que llena `lib/invoke.ts` cuando el
// gate universal del backend rechaza un comando con `pending_approval`) y muestra el pedido. Al
// decidir, resuelve la promesa del wrapper (Aprobar → approval_resolve(true) + re-invoke + consume;
// Cancelar → approval_resolve(false)). Así CUALQUIER superficie hereda la UI sin lógica propia.

import { useEffect, useState } from "react";
import { ModalFrame, DangerZone } from "./canonical";
import { ApprovalRequest, subscribeApprovals, decideApproval } from "../lib/approvalBus";
import { CommandDef, loadCommandRegistry } from "../lib/commandRegistry";

export function GlobalApprovalModal() {
  const [queue, setQueue] = useState<ApprovalRequest[]>([]);
  const [registry, setRegistry] = useState<Map<string, CommandDef>>(new Map());

  useEffect(() => subscribeApprovals(setQueue), []);
  useEffect(() => {
    let alive = true;
    loadCommandRegistry()
      .then((cmds) => {
        if (alive) setRegistry(new Map(cmds.map((c) => [c.id, c])));
      })
      .catch(() => {
        /* sin registry mostramos command_id + risk igual */
      });
    return () => {
      alive = false;
    };
  }, []);

  const req = queue[0];
  if (!req) return null;
  const def = registry.get(req.commandId);

  const reasons: string[] = [];
  if (def?.requires_confirmation) reasons.push("requiere confirmación");
  if (req.risk === "destructive") reasons.push("acción destructiva");
  if (req.risk === "credential") reasons.push("toca credenciales");

  return (
    <ModalFrame
      title="Aprobación requerida"
      subtitle={req.commandId}
      onClose={() => decideApproval(req.id, false)}
      danger
      maxWidth={560}
      footer={
        <>
          <button
            type="button"
            className="fxc-btn"
            onClick={() => decideApproval(req.id, false)}
          >
            Cancelar
          </button>
          <button
            type="button"
            className="fxc-btn fxc-btn--danger"
            onClick={() => decideApproval(req.id, true)}
          >
            Aprobar y ejecutar
          </button>
        </>
      }
    >
      <DangerZone
        title={def?.label || req.commandId}
        label="APROBACIÓN REQUERIDA"
        description={
          <>
            {def?.description || "Esta acción requiere tu aprobación antes de ejecutarse."}
            {reasons.length > 0 && (
              <span className="fxc-cp015__reasons"> — {reasons.join(" · ")}.</span>
            )}
            {def && !def.reversible && <span className="fxc-cp015__reasons"> Irreversible.</span>}
          </>
        }
      />
    </ModalFrame>
  );
}
