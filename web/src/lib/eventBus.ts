// web/src/lib/eventBus.ts — 015-frontend-reform-kernel · US3 (front del state sync layer).
//
// Subscriber TIPADO del bus de eventos Rust→ventana. Escucha el canal único `furx:event`
// (espejo de services/event_bus.rs::BUS_CHANNEL), descarta envelopes con `seq` <= al último
// visto (un snapshot viejo NUNCA pisa uno nuevo — clave para 2+ viewports / IPC reordenado), y
// expone:
//   - `subscribeAppEvents(handler)` — suscripción cruda tipada.
//   - `useAppEvent(tag, handler)` — hook React mínimo, rehidratable, filtrado por tag.
//   - `lastSeenSeq()` — cursor monotónico observado (debug / tests).
//
// Los tipos son ESPEJO de `AppEvent` (serde adjacently-tagged: { tag, data }) + el envelope
// { seq, ts, ...payload } (serde flatten). Si cambia el enum Rust, actualizar acá.

import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { useEffect, useRef } from "react";

/** Canal Tauri único — DEBE coincidir con event_bus.rs::BUS_CHANNEL. */
export const BUS_CHANNEL = "furx:event";

/** Espejo tipado de `AppEvent` (Rust). Discriminado por `tag`. */
export type AppEvent =
  | { tag: "TaskChanged"; data: { id: string; state: string } }
  | { tag: "AgentStateChanged"; data: { id: string; state: string } }
  | { tag: "LayoutChanged"; data: { window_id: string } }
  | { tag: "CommandExecuted"; data: { command_id: string } }
  | { tag: "ApprovalRequested"; data: { request_id: string; command_id: string } };

export type AppEventTag = AppEvent["tag"];

/**
 * Envelope que llega por el IPC: el `EventEnvelope` de Rust serializa `seq` + `ts` + el payload
 * aplanado (`#[serde(flatten)]`), por eso `tag`/`data` viven al lado de `seq`/`ts`.
 */
export type EventEnvelope = AppEvent & { seq: number; ts: number };

type Handler = (ev: AppEvent, env: EventEnvelope) => void;

// --- Estado del subscriber (singleton por webview window) -------------------------------------

let lastSeq = 0;
const handlers = new Set<Handler>();
let unlisten: UnlistenFn | null = null;
let starting: Promise<void> | null = null;

/** Cursor monotónico del último seq APLICADO. Expuesto para debug/tests. */
export function lastSeenSeq(): number {
  return lastSeq;
}

/** Reset (sólo para tests — limpia el cursor y los handlers). */
export function __resetForTest(): void {
  lastSeq = 0;
  handlers.clear();
}

/**
 * Regla SSOT de orden: aplicar un envelope SÓLO si su `seq` es estrictamente mayor al último
 * visto. Idéntica al contrato fijado en el test Rust `stale_seq_is_ignored_contract`. Devuelve
 * true si se aplicó (y avanza el cursor), false si se descartó por viejo/duplicado.
 *
 * Audit US3: esto NO pierde eventos semánticos. El bus de Rust ya NO coalesce eventos semánticos
 * (coalescible()==false), así que cada ocurrencia real recibe un `seq` ÚNICO y CRECIENTE; el canal
 * `furx:event` es FIFO de un solo stream, por lo que llegan en orden. El `seq <= lastSeq` sólo
 * descarta una RE-ENTREGA exacta del mismo seq (replay), nunca un evento distinto.
 */
export function applyEnvelope(env: EventEnvelope): boolean {
  if (env.seq <= lastSeq) return false; // replay exacto → no re-aplicar
  lastSeq = env.seq;
  const ev = { tag: env.tag, data: env.data } as AppEvent;
  for (const h of handlers) {
    try {
      h(ev, env);
    } catch (e) {
      // Un handler que tira NO debe matar al resto.
      console.warn("[eventBus] handler error", e);
    }
  }
  return true;
}

/** Arranca el listener Tauri una sola vez (idempotente). */
async function ensureStarted(): Promise<void> {
  if (unlisten || starting) return starting ?? Promise.resolve();
  starting = (async () => {
    try {
      unlisten = await listen<EventEnvelope>(BUS_CHANNEL, (e) => {
        applyEnvelope(e.payload);
      });
    } catch (e) {
      // Fuera de Tauri (web companion / SSR) `listen` no existe — degradar en silencio.
      console.warn("[eventBus] listen unavailable (non-Tauri context?)", e);
    } finally {
      starting = null;
    }
  })();
  return starting;
}

/**
 * Suscripción cruda tipada. Devuelve un unsubscribe. El primer subscriber arranca el listener;
 * el listener Tauri queda vivo (es barato) aunque no haya subscribers — los envelopes igual
 * avanzan `lastSeq` para no perder el orden si alguien se re-suscribe.
 */
export function subscribeAppEvents(handler: Handler): () => void {
  handlers.add(handler);
  void ensureStarted();
  return () => {
    handlers.delete(handler);
  };
}

/**
 * Hook React mínimo y rehidratable: corre `handler` por cada evento del `tag` pedido. Usa un ref
 * para el handler → no re-suscribe si la closure cambia entre renders.
 */
export function useAppEvent<T extends AppEventTag>(
  tag: T,
  handler: (data: Extract<AppEvent, { tag: T }>["data"], env: EventEnvelope) => void,
): void {
  const ref = useRef(handler);
  ref.current = handler;
  useEffect(() => {
    // Adaptador no-genérico: trabaja sobre la unión completa y delega al handler. El compilador
    // no puede estrechar el genérico abierto `T` por el guard `ev.tag === tag`, así que tratamos
    // el ref como un consumidor de la unión (seguro en runtime: el guard garantiza la variante).
    // CRÍTICO (audit codex 035#2): leer `ref.current` DENTRO del callback, no al suscribir — así
    // el subscriber NO se re-suscribe entre renders PERO usa SIEMPRE la última closure (no la 1ra).
    return subscribeAppEvents((ev, env) => {
      if (ev.tag === tag) (ref.current as (data: unknown, env: EventEnvelope) => void)(ev.data, env);
    });
  }, [tag]);
}
