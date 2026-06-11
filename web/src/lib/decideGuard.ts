// web/src/lib/decideGuard.ts — 044 FR-003 — guard anti doble-respuesta para las acciones de card.
//
// Las acciones de card (approve/reject/snooze/…) son DECISIONES HUMANAS: nada se auto-decide. El
// guard sólo evita una condición de carrera de UI (una respuesta tardía post-timeout que mutaría
// estado obsoleto, o un doble-clic que dejaría el botón habilitado durante el 2º invoke).
//
// Diseño (council P0.1, corregido por audit-3 a seq POR-CARD): cada card tiene su PROPIO contador de
// seq monotónico → dos cards distintas NO se invalidan entre sí (la global rompía la concurrencia).
// Cada acción captura su `thisSeq` al arrancar; cualquier resolución (éxito/error/timeout) SÓLO aplica
// si `thisSeq` sigue siendo el ÚLTIMO seq de ESA card. El TIMEOUT consume el seq (lo incrementa) para
// que una resolución posterior del MISMO invoke quede stale (evita el doble-efecto error→éxito).
//
// La parte PURA vive acá para ser testeable sin React/timers; el hook `useDecideGuard` le agrega el
// estado React + el timer real.

/** Estado del guard: el último seq emitido por cada cardId. */
export interface DecideSeqState {
  /** cardId → último seq emitido para esa card. */
  seqByCard: Map<string, number>;
}

/** Crea el estado inicial (sin cards en vuelo). */
export function createSeqState(): DecideSeqState {
  return { seqByCard: new Map() };
}

/**
 * Arranca (o invalida) la acción en curso de una card: incrementa SU seq y devuelve el nuevo valor.
 * El caller guarda `thisSeq` y lo compara contra el seq actual de la card en cada resolución.
 * También se usa para CONSUMIR el seq desde un timeout (bumpea → la resolución vieja queda stale).
 */
export function beginInvoke(state: DecideSeqState, cardId: string): number {
  const next = (state.seqByCard.get(cardId) ?? 0) + 1;
  state.seqByCard.set(cardId, next);
  return next;
}

/**
 * ¿Esta resolución (éxito/error/timeout) debe aplicar su efecto sobre el estado?
 * Sólo si `thisSeq` sigue siendo el ÚLTIMO seq emitido para ESA card (no arrancó otra acción ni
 * venció el timeout que consume el seq).
 */
export function shouldApply(state: DecideSeqState, cardId: string, thisSeq: number): boolean {
  return (state.seqByCard.get(cardId) ?? 0) === thisSeq;
}

/** Timeout (ms) tras el cual una acción sin respuesta re-habilita el botón con un error temporal. */
export const DECIDE_TIMEOUT_MS = 15000;
