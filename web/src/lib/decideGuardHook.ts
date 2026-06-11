// web/src/lib/decideGuardHook.ts — 044 FR-003 — hook React del guard anti doble-respuesta.
//
// Envuelve la lógica pura de `decideGuard.ts` (seq POR-CARD) con el estado React (`decidingCardId`,
// `cardErrors`) y el timer real de 15s. Expone `run(cardId, action, onApplied?)` que ejecuta el
// invoke real con el guard y dispara el efecto post-éxito (`onApplied`, p.ej. refreshAll) SÓLO si la
// resolución sigue siendo la vigente para esa card.

import { useCallback, useEffect, useRef, useState } from "react";
import {
  createSeqState,
  beginInvoke,
  shouldApply,
  DECIDE_TIMEOUT_MS,
} from "./decideGuard.ts";

export interface DecideGuard {
  /** error inline por card (la card NO desaparece; muestra el error). */
  cardErrors: Record<string, string>;
  /**
   * Ejecuta una acción de decisión sobre `cardId` con el guard. `action` es el invoke real (puede
   * tirar) y NO debe tener efectos colaterales de mutación de vista (esos van en `onApplied`).
   * `onApplied` (opcional) corre SÓLO si la resolución exitosa sigue siendo la vigente de esa card
   * (p.ej. refreshAll) — así una respuesta tardía post-timeout no refresca/remueve la card.
   * Si ya hay una acción en vuelo para ESA card, no arranca otra (evita el doble-clic durante el
   * invoke). Una respuesta tardía post-timeout (seq viejo) NO muta. A los 15s sin respuesta,
   * re-habilita el botón con un error temporal Y consume el seq (la resolución posterior queda stale).
   */
  run: (cardId: string, action: () => Promise<void>, onApplied?: () => void) => void;
  /** ¿la card tiene una decisión en vuelo (botones deshabilitados)? */
  isDeciding: (cardId: string) => boolean;
  /** limpia el error inline de una card. */
  clearError: (cardId: string) => void;
}

export function useDecideGuard(timeoutMs: number = DECIDE_TIMEOUT_MS): DecideGuard {
  const seqRef = useRef(createSeqState());
  // cards en vuelo, en ESTADO React (no sólo ref) para que el cambio dispare re-render y los botones
  // de TODAS las cards concurrentes reflejen su disabled. Mapa cardId→true (sólo presentes las en
  // vuelo). El audit-3 reveló que un único `decidingCardId` no re-renderizaba al 2º invoke concurrente.
  const [decidingMap, setDecidingMap] = useState<Record<string, true>>({});
  const [cardErrors, setCardErrors] = useState<Record<string, string>>({});
  // timers vivos por (cardId+seq), para limpiarlos al desmontar (evita setState tras unmount).
  const timersRef = useRef<Map<string, ReturnType<typeof setTimeout>>>(new Map());
  const aliveRef = useRef(true);
  // set de cards en vuelo: guard SÍNCRONO anti doble-clic POR-CARD (independiente entre cards). El
  // estado React `decidingMap` es para el render; este ref decide ANTES del re-render.
  const inFlightRef = useRef<Set<string>>(new Set());

  useEffect(() => {
    aliveRef.current = true;
    const timers = timersRef.current;
    return () => {
      aliveRef.current = false;
      for (const id of timers.values()) clearTimeout(id);
      timers.clear();
    };
  }, []);

  const setError = useCallback((cardId: string, msg: string) => {
    setCardErrors((prev) => ({ ...prev, [cardId]: msg }));
  }, []);

  const clearError = useCallback((cardId: string) => {
    setCardErrors((prev) => {
      if (!(cardId in prev)) return prev;
      const next = { ...prev };
      delete next[cardId];
      return next;
    });
  }, []);

  // marca/desmarca una card como "en vuelo" en el ESTADO (dispara re-render de SUS botones).
  const setDeciding = useCallback((cardId: string, on: boolean) => {
    setDecidingMap((prev) => {
      const has = cardId in prev;
      if (on === has) return prev; // sin cambio → no re-render espurio.
      const next = { ...prev };
      if (on) next[cardId] = true; else delete next[cardId];
      return next;
    });
  }, []);

  const finish = useCallback((cardId: string, timerKey: string) => {
    const timer = timersRef.current.get(timerKey);
    if (timer) { clearTimeout(timer); timersRef.current.delete(timerKey); }
    inFlightRef.current.delete(cardId);
    setDeciding(cardId, false);
  }, [setDeciding]);

  const run = useCallback(
    (cardId: string, action: () => Promise<void>, onApplied?: () => void) => {
      // Guard anti doble-clic POR-CARD: si esta card ya está en vuelo, no arranques otra. Usamos el
      // ref síncrono (el estado React no está actualizado entre 2 clicks del mismo tick).
      if (inFlightRef.current.has(cardId)) return;
      inFlightRef.current.add(cardId);

      const thisSeq = beginInvoke(seqRef.current, cardId);
      const timerKey = `${cardId}:${thisSeq}`;
      setDeciding(cardId, true);
      clearError(cardId);

      // Timeout: a los 15s, si esta acción sigue vigente, CONSUMIR el seq (bump) → cualquier
      // resolución posterior del MISMO invoke queda stale (no re-aplica). Re-habilitar con error.
      const timer = setTimeout(() => {
        timersRef.current.delete(timerKey);
        if (!aliveRef.current) return;
        if (shouldApply(seqRef.current, cardId, thisSeq)) {
          beginInvoke(seqRef.current, cardId); // consume el seq → invalida la resolución tardía.
          inFlightRef.current.delete(cardId);
          setDeciding(cardId, false);
          setError(cardId, "Error temporal — intentá de nuevo");
        }
      }, timeoutMs);
      timersRef.current.set(timerKey, timer);

      void (async () => {
        try {
          await action();
          if (!aliveRef.current) return;
          // SÓLO aplicamos (limpiar error + onApplied/refreshAll) si esta resolución sigue vigente.
          if (shouldApply(seqRef.current, cardId, thisSeq)) {
            finish(cardId, timerKey);
            clearError(cardId);
            onApplied?.(); // p.ej. refreshAll — gated: una respuesta tardía NO refresca/remueve la card.
          }
        } catch (e) {
          if (!aliveRef.current) return;
          if (shouldApply(seqRef.current, cardId, thisSeq)) {
            finish(cardId, timerKey);
            setError(cardId, String(e)); // la card muestra el error y NO desaparece.
          }
        }
      })();
    },
    [timeoutMs, clearError, setError, setDeciding, finish],
  );

  // `isDeciding` lee el ESTADO `decidingMap` (re-renderable) → al cambiar, re-evalúa los botones de
  // TODAS las cards concurrentes. El `inFlightRef` (síncrono) cubre el instante entre el click y el
  // re-render (un 2º click del mismo tick), pero la verdad de render es `decidingMap`.
  const isDeciding = useCallback(
    (cardId: string) => cardId in decidingMap || inFlightRef.current.has(cardId),
    [decidingMap],
  );

  return { cardErrors, run, isDeciding, clearError };
}
