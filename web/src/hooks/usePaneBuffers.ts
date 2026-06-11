// PLAN_CLOSE J — extracted pane-buffer hook.
//
// Owns the last-8KB tail per pane id + per-pane Suggestion derived from a 3s
// poll of `suggest_for_text`. Buffers live in a ref so the polling task always
// sees the freshest writes without re-rendering on every PTY chunk (council
// MUST-FIX: ref-based reads, no stale-closure).

import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { Suggestion } from "../types";

const BUFFER_CAP_BYTES = 8 * 1024;
const SUGGEST_POLL_MS = 3000;

interface Api {
  /** Append a PTY chunk to the buffer for `paneId`. Keeps the trailing 8KB. */
  captureOutput: (paneId: string, data: string) => void;
  /** Current suggestion per pane (poll-derived). null while empty/unset. */
  paneSuggestions: Record<string, Suggestion | null>;
  /** Read the latest buffer tail for `paneId` (e.g. for "explain this error"). */
  bufferOf: (paneId: string) => string;
  /** Shallow copy of every buffer (for modals that need the whole map). */
  snapshotBuffers: () => Record<string, string>;
  /** Remove a pane's buffer (call from removePane to keep memory bounded). */
  forgetPane: (paneId: string) => void;
}

export function usePaneBuffers(): Api {
  const buffers = useRef<Record<string, string>>({});
  const [paneSuggestions, setPaneSuggestions] = useState<Record<string, Suggestion | null>>({});

  const captureOutput = useCallback((paneId: string, data: string) => {
    const cur = buffers.current[paneId] ?? "";
    const next = (cur + data).slice(-BUFFER_CAP_BYTES);
    buffers.current[paneId] = next;
  }, []);

  const bufferOf = useCallback((paneId: string) => buffers.current[paneId] ?? "", []);

  const snapshotBuffers = useCallback(() => ({ ...buffers.current }), []);

  const forgetPane = useCallback((paneId: string) => {
    delete buffers.current[paneId];
    setPaneSuggestions((prev) => {
      if (!(paneId in prev)) return prev;
      const next = { ...prev };
      delete next[paneId];
      return next;
    });
  }, []);

  // Codex audit MED #1: async setInterval can overlap if suggest_for_text
  // takes >3s. Use inFlight ref to skip ticks that would pile up.
  // Codex audit MED #2: filter updates against current buffers.current so a
  // pane removed mid-poll doesn't get reinserted into paneSuggestions.
  const inFlight = useRef(false);
  useEffect(() => {
    let cancelled = false;
    const id = window.setInterval(async () => {
      if (cancelled || inFlight.current) return;
      const ids = Object.keys(buffers.current);
      if (ids.length === 0) return;
      inFlight.current = true;
      try {
        const updates: Record<string, Suggestion | null> = {};
        for (const pid of ids) {
          if (cancelled) return;
          const text = buffers.current[pid];
          if (!text) { updates[pid] = null; continue; }
          try {
            const s = await invoke<Suggestion | null>("suggest_for_text", { text });
            updates[pid] = s ?? null;
          } catch {
            updates[pid] = null;
          }
        }
        if (cancelled) return;
        setPaneSuggestions((prev) => {
          // Codex MED #2: drop keys whose pane was removed during the poll.
          const next: Record<string, Suggestion | null> = { ...prev };
          for (const k of Object.keys(updates)) {
            if (k in buffers.current) next[k] = updates[k];
          }
          // Also evict any prev key whose buffer is gone.
          for (const k of Object.keys(next)) {
            if (!(k in buffers.current)) delete next[k];
          }
          return next;
        });
      } finally {
        inFlight.current = false;
      }
    }, SUGGEST_POLL_MS);
    return () => { cancelled = true; window.clearInterval(id); };
  }, []);

  return { captureOutput, paneSuggestions, bufferOf, snapshotBuffers, forgetPane };
}
