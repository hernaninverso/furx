// 015 US4 — Capability / approval gate (espejo TS de `services/capability.rs`).
//
// El front NO decide nada de seguridad: el backend Rust es el dueño del gate. Este
// módulo sólo provee tipos + loaders sobre los 3 comandos Tauri (`capability_check`,
// `approval_list`, `approval_resolve`).
//
// BYOK (constitución F-I): el front NUNCA recibe ni maneja keys. Cuando un comando
// necesita una credencial, despacha un *credential ref* (un string id = nombre del
// entry del Keychain); el backend resuelve la key SÓLO al ejecutar. Acá no hay, ni
// debe haber, ningún tipo que lleve una key.

import { invoke } from "@tauri-apps/api/core";

/** Resultado de consultar el gate para un comando, SIN ejecutar nada. */
export interface CapabilityCheck {
  /** Si el comando debe pasar por aprobación humana antes de ejecutarse. */
  requires_approval: boolean;
  /** Risk del comando según el registry, o "unknown" si no está en el registry. */
  risk: "safe" | "destructive" | "credential" | "external" | "unknown";
  /** True si el command_id no está en el registry (fail-closed → requires_approval). */
  unknown: boolean;
}

/** Estado de una solicitud de aprobación. Espejo de `ApprovalStatus` (Rust). */
export type ApprovalStatus = "pending" | "approved" | "rejected";

/**
 * Solicitud de aprobación persistida (estado de primera clase). NUNCA contiene
 * secrets: `args_json` lleva args NO sensibles (incl. un credential ref), jamás la key.
 */
export interface Approval {
  id: string;
  command_id: string;
  /** JSON string con args NO sensibles del comando (incl. credential ref). */
  args_json: string;
  status: ApprovalStatus;
  created_at: string;
  resolved_at: string | null;
  /** 015 T015 — ISO-8601 de cuándo se consumió el approval (ejecución real). NON-NULL = usado. */
  consumed_at: string | null;
}

/**
 * 015 T015 — payload con que el interceptor del dispatch RECHAZA un comando gateado sin
 * aprobación. El backend (`lib.rs`) hace `resolver.reject({kind:"pending_approval", ...})`, así
 * que el `invoke()` promise rechaza con este objeto. El front lo reconoce para mostrar la UI de
 * aprobación y, tras aprobar, RE-invocar el mismo comando (el interceptor consume el approval).
 */
export interface PendingApprovalRejection {
  kind: "pending_approval";
  request_id: string;
  risk: string;
}

/** Type guard: ¿este error de `invoke()` es un pedido de aprobación del gate universal? */
export function isPendingApproval(err: unknown): err is PendingApprovalRejection {
  return (
    typeof err === "object" &&
    err !== null &&
    (err as { kind?: unknown }).kind === "pending_approval" &&
    typeof (err as { request_id?: unknown }).request_id === "string"
  );
}

/**
 * Pregunta al backend si `commandId` requiere aprobación (consulta pura, no crea
 * approvals). Devuelve el risk del registry. Lanza fuera del runtime Tauri.
 */
export async function capabilityCheck(
  commandId: string,
): Promise<CapabilityCheck> {
  return invoke<CapabilityCheck>("capability_check", { commandId });
}

/** Lista los approvals (pendientes primero). */
export async function approvalList(): Promise<Approval[]> {
  return invoke<Approval[]>("approval_list");
}

/**
 * Resuelve un approval pendiente. `approved=true` → aprobado (señal para que el
 * backend ejecute el comando real); `false` → rechazado. Devuelve el approval
 * actualizado. Esta llamada ES la decisión humana del gate.
 */
export async function approvalResolve(
  id: string,
  approved: boolean,
): Promise<Approval> {
  return invoke<Approval>("approval_resolve", { id, approved });
}

/** Sólo los approvals que siguen pendientes. Helper de conveniencia para un panel. */
export function pendingApprovals(all: Approval[]): Approval[] {
  return all.filter((a) => a.status === "pending");
}
