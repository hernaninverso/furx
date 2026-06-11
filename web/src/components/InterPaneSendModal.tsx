// F7 — Inter-pane send last output.
// User clicks "→" on a source pane's header, picks a target pane, the modal
// grabs the last N lines from the source buffer and writes them (wrapped in
// `[from <source>]: ... [/from]`) to the target via pty_write.

import { useEffect, useState, useMemo } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { PaneCfg } from "../types";
import { Modal } from "./Modal";
import { Button } from "./Button";

interface Props {
  source: PaneCfg;
  panes: PaneCfg[];
  /** Returns the most recent buffer tail for the source pane (8KB cap upstream). */
  getBuffer: (paneId: string) => string;
  /** spec 004 follow-up — deliver into a data/compare view pane (no PTY) instead of pty_write. */
  onDeliverToView?: (targetId: string, kind: "data" | "compare", text: string) => void;
  onClose: () => void;
}

const DEFAULT_LINES = 50;
const MAX_LINES = 500;
// Cap payload to keep an LLM target from being flooded — also matches the
// 8KB pane-buffer upstream cap on Tauri's side.
const MAX_LENGTH = 8 * 1024;

// Council BLOQUE C MUST-FIX (sec/data 4/5): the source buffer can contain
// secrets (Bearer tokens, sk- API keys, passwords echoed back). Redact before
// the user can hit "Send" — defence-in-depth over the backend scrubber that
// also runs on crash logs.
const SECRET_PATTERNS: { re: RegExp; replacement: string }[] = [
  { re: /\bbearer\s+[A-Za-z0-9._-]{16,}/gi, replacement: "Bearer <redacted>" },
  { re: /\bsk-(?:ant-)?[A-Za-z0-9_-]{16,}/g, replacement: "<redacted-sk>" },
  { re: /\bghp_[A-Za-z0-9]{20,}/g, replacement: "<redacted-gh>" },
  { re: /\bAKIA[0-9A-Z]{12,}/g, replacement: "<redacted-aws>" },
  { re: /\bcfut_[A-Za-z0-9._-]{16,}/g, replacement: "<redacted-cf-tun>" },
  { re: /\b(api[_-]?key|password|secret|access[_-]?token)\s*[=:]\s*['"]?[A-Za-z0-9._/+=-]{8,}['"]?/gi, replacement: "$1=<redacted>" },
];

function redactSecrets(text: string): { redacted: string; hits: number } {
  let hits = 0;
  let out = text;
  for (const { re, replacement } of SECRET_PATTERNS) {
    out = out.replace(re, (m) => { hits += 1; return typeof replacement === "string" && replacement.includes("$1")
      ? m.replace(re, replacement)
      : replacement;
    });
  }
  return { redacted: out, hits };
}

export function InterPaneSendModal({ source, panes, getBuffer, onDeliverToView, onClose }: Props) {
  const candidates = useMemo(() => panes.filter((p) => p.id !== source.id), [panes, source.id]);
  const [targetId, setTargetId] = useState<string | null>(candidates[0]?.id ?? null);
  const [lines, setLines] = useState<number>(DEFAULT_LINES);
  const [sending, setSending] = useState<boolean>(false);
  const [error, setError] = useState<string | null>(null);

  // Snapshot the buffer ONCE when the modal opens so a slow user pick doesn't
  // capture buffer drift between preview and send (edge-case from council).
  const [snapshot, setSnapshot] = useState<string>("");
  useEffect(() => { setSnapshot(getBuffer(source.id)); }, [getBuffer, source.id]);

  const { tail, redactionHits } = useMemo(() => {
    const all = snapshot.split("\n");
    const take = Math.max(1, Math.min(MAX_LINES, lines));
    const slice = all.slice(-take).join("\n");
    const capped = slice.length > MAX_LENGTH ? slice.slice(-MAX_LENGTH) : slice;
    const { redacted, hits } = redactSecrets(capped);
    return { tail: redacted, redactionHits: hits };
  }, [snapshot, lines]);

  const canSend = !sending && !!targetId && tail.trim().length > 0;

  const handleSend = async () => {
    if (!targetId || sending) return;
    if (targetId === source.id) { setError("source and target must differ"); return; }
    // Council MUST-FIX (5/5): both source AND target may disappear between the
    // open-modal click and the actual send. We snapshotted the source buffer
    // on mount so its data is safe, but we still refuse to address a vanished
    // target (or a vanished source — there's nothing to "originate" from).
    if (!candidates.find((c) => c.id === targetId)) {
      setError("target pane is no longer available");
      return;
    }
    if (!panes.find((p) => p.id === source.id)) {
      setError("source pane no longer exists");
      return;
    }
    setSending(true);
    setError(null);
    const correlation = `interpane-${Date.now()}`;

    // spec 004 follow-up — a data/compare target has no PTY: deliver the (already-redacted)
    // tail straight into the view's content instead of pty_write.
    const target = panes.find((p) => p.id === targetId);
    if (target && (target.kind === "data" || target.kind === "compare") && onDeliverToView) {
      try {
        onDeliverToView(targetId, target.kind, tail);
        await invoke("interpane_send_audit", {
          payload: { source_pane_id: source.id, target_pane_id: targetId, length: tail.length, lines: tail.split("\n").length },
        }).catch((e) => console.warn("interpane_send_audit failed (non-fatal)", e));
        onClose();
      } catch (e) {
        setError(e instanceof Error ? e.message : String(e));
      } finally { setSending(false); }
      return;
    }

    const prefix = `[from ${source.title} · ${source.mode}]:\n`;
    const suffix = `\n[/from]\n`;
    const payload = `${prefix}${tail}${suffix}`;
    try {
      await invoke("pty_write", {
        paneId: targetId,
        data: payload,
        correlationId: correlation,
        actionId: `${correlation}-${targetId}`,
      });
      await invoke("interpane_send_audit", {
        payload: {
          source_pane_id: source.id,
          target_pane_id: targetId,
          length: payload.length,
          lines: tail.split("\n").length,
        },
      }).catch((e) => console.warn("interpane_send_audit failed (non-fatal)", e));
      onClose();
    } catch (e) {
      console.error("interpane send failed", e);
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setSending(false);
    }
  };

  return (
    <Modal
      title="Enviar último output a otro panel"
      subtitle={<>Las últimas {lines} líneas de <code>{source.title}</code> se enviarán como mensaje al panel que elijas.</>}
      maxWidth={620}
      onClose={onClose}
      onSubmit={handleSend}
      canSubmit={canSend}
    >
      {candidates.length === 0 && (
        <div className="muted">No hay otros paneles abiertos para enviar.</div>
      )}
      {candidates.length > 0 && (
        <>
          <label style={{ display: "block", marginBottom: 8, fontSize: ".85rem" }}>
            Destino:
            <select
              value={targetId ?? ""}
              onChange={(e) => setTargetId(e.target.value)}
              onClick={(e) => e.stopPropagation()}
              style={{ marginLeft: 8, padding: "4px 8px", background: "var(--bg2)", color: "var(--text)", border: "1px solid var(--line)", borderRadius: 4 }}
              aria-label="Panel destino"
            >
              {candidates.map((p) => (
                <option key={p.id} value={p.id}>
                  {p.title} · {p.kind && p.kind !== "terminal" ? `→ ${p.kind}` : p.mode}
                </option>
              ))}
            </select>
          </label>
          <label style={{ display: "block", marginBottom: 8, fontSize: ".85rem" }}>
            Líneas (max {MAX_LINES}):
            <input
              type="number"
              min={1}
              max={MAX_LINES}
              value={lines}
              onChange={(e) => setLines(Math.max(1, Math.min(MAX_LINES, parseInt(e.target.value, 10) || DEFAULT_LINES)))}
              style={{ marginLeft: 8, width: 80, padding: "4px 8px", background: "var(--bg2)", color: "var(--text)", border: "1px solid var(--line)", borderRadius: 4 }}
            />
          </label>
          <div className="muted" style={{ marginBottom: 6, fontSize: ".8rem" }}>
            Vista previa ({tail.length} chars · {tail.split("\n").length} lines{redactionHits > 0 ? ` · ${redactionHits} secret(s) redacted` : ""}):
          </div>
          <pre
            aria-label="Outgoing payload preview"
            style={{
              background: "var(--bg2)", border: "1px solid var(--line)", borderRadius: 6,
              padding: 8, fontSize: 11, lineHeight: 1.4, fontFamily: "var(--mono)",
              maxHeight: 220, overflow: "auto", margin: 0, color: "var(--text)",
            }}
          >
            {tail.length > 0 ? tail : "(buffer vacío — nada que enviar)"}
          </pre>
        </>
      )}
      {error && (
        <div role="alert" style={{ marginTop: 10, color: "var(--red)", fontSize: ".85rem" }}>
          {error}
        </div>
      )}
      <div className="wizard-actions" style={{ marginTop: 12 }}>
        <button onClick={onClose}>Cancelar</button>
        <Button variant="primary" onClick={handleSend} disabled={!canSend}>
          {sending ? "Enviando…" : <>Enviar <kbd style={{ marginLeft: 6 }}>⌘↩</kbd></>}
        </Button>
      </div>
    </Modal>
  );
}
