// web/src/lib/approvalBus.ts — 015 T015 · cola GLOBAL de pedidos de aprobación.
//
// El gate universal del backend (interceptor del dispatch) corta cualquier comando Destructive/
// Credential sin approval consumible y rechaza el invoke con `pending_approval`. El wrapper
// `lib/invoke.ts` captura ese rechazo y ENCOLA un pedido acá; el `GlobalApprovalModal` (montado una
// vez en el Shell) lo muestra y, cuando el humano decide, resuelve la promesa del wrapper. Así
// CUALQUIER superficie (palette/botón/plugin/móvil) hereda la UI de aprobación sin lógica propia.

export interface ApprovalRequest {
  /** id local (seq) — clave del pedido en la cola. */
  id: number;
  /** command id del registry (lo que se va a ejecutar). */
  commandId: string;
  /** id del approval pending en el backend (para approval_resolve). */
  requestId: string;
  /** risk del comando ("destructive"|"credential"|...) que reportó el gate. */
  risk: string;
  /** interno: resuelve la promesa del wrapper con la decisión humana. */
  resolve: (approved: boolean) => void;
}

type Listener = (reqs: ApprovalRequest[]) => void;

let queue: ApprovalRequest[] = [];
let listeners: Listener[] = [];
let seq = 0;

function emit(): void {
  const snapshot = [...queue];
  for (const l of listeners) l(snapshot);
}

/** Suscribe el modal global a la cola. Devuelve el unsubscribe. Llama al listener con el estado actual. */
export function subscribeApprovals(l: Listener): () => void {
  listeners.push(l);
  l([...queue]);
  return () => {
    listeners = listeners.filter((x) => x !== l);
  };
}

/**
 * Encola un pedido de aprobación y devuelve una promesa que resuelve `true` (aprobar) / `false`
 * (rechazar) cuando el usuario decide en el modal global. La usa `lib/invoke.ts`.
 */
export function requestApproval(info: {
  commandId: string;
  requestId: string;
  risk: string;
}): Promise<boolean> {
  return new Promise<boolean>((resolve) => {
    queue.push({ id: ++seq, ...info, resolve });
    emit();
  });
}

/** El modal llama esto al decidir: saca el pedido de la cola y resuelve su promesa. */
export function decideApproval(id: number, approved: boolean): void {
  const req = queue.find((r) => r.id === id);
  if (!req) return;
  queue = queue.filter((r) => r.id !== id);
  emit();
  req.resolve(approved);
}
