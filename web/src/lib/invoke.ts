// web/src/lib/invoke.ts — 015 T015 · `invoke` envuelto con el flujo de aprobación universal.
//
// Drop-in de `@tauri-apps/api/core`'s invoke. Si el comando es Destructive/Credential, el gate del
// backend (interceptor del dispatch en lib.rs) rechaza con `pending_approval`; acá lo capturamos,
// pedimos la aprobación humana (modal global vía approvalBus) y, si se aprueba, resolvemos el
// approval y RE-invocamos el MISMO comando con los MISMOS args (el interceptor consume el approval
// — single-use — y ejecuta). Así toda superficie que importe este `invoke` queda gateada sin código
// extra. Safe/External pasan directo (el backend no los intercepta).

import { invoke as rawInvoke } from "@tauri-apps/api/core";
import { isPendingApproval, approvalResolve } from "./capability";
import { requestApproval } from "./approvalBus";

export async function invoke<T = unknown>(
  cmd: string,
  args?: Record<string, unknown>,
): Promise<T> {
  try {
    return await rawInvoke<T>(cmd, args);
  } catch (e) {
    if (!isPendingApproval(e)) throw e;
    // Gate universal: el comando quedó pending. Pedimos la decisión humana (modal global).
    const approved = await requestApproval({
      commandId: cmd,
      requestId: e.request_id,
      risk: e.risk,
    });
    if (!approved) {
      await approvalResolve(e.request_id, false).catch(() => {
        /* best-effort: si ya estaba resuelto, da igual */
      });
      throw new Error("aprobación rechazada");
    }
    await approvalResolve(e.request_id, true);
    // Re-invoke con los MISMOS args → el interceptor encuentra el approval aprobado, lo consume y
    // delega al comando real. Un 2do `pending_approval` (raro: TTL/args) se propaga como error.
    return await rawInvoke<T>(cmd, args);
  }
}

// Re-export del invoke crudo para los pocos call-sites que necesiten saltear el flujo (ej el
// propio approval_resolve, que el backend ya bypassa, o llamadas internas del wrapper).
export { rawInvoke };
