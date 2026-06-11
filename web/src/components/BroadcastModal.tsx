import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { PaneCfg } from "../types";
import { Modal } from "./Modal";
import { Button } from "./Button";

interface Props { panes: PaneCfg[]; onClose: () => void; }

// Codex audit LOW #6: explicit TS contract for broadcast_audit_sent invoke
// payload — keeps frontend/backend in lock-step (Rust: BroadcastAuditInput).
interface BroadcastAuditInput {
  count: number;
  length: number;
  message_hash?: string;
  success_count?: number;
  failure_count?: number;
}

/// BLOQUE A · F4 — SubtleCrypto SHA-256 with a hex-string output. Used for
/// optional message_hash dedup in audit. Returns empty string if SubtleCrypto
/// is unavailable (e.g. legacy WebView) — backend rejects non-hex anyway.
async function sha256Hex(text: string): Promise<string> {
  try {
    if (typeof crypto === "undefined" || !crypto.subtle) return "";
    const data = new TextEncoder().encode(text);
    const buf = await crypto.subtle.digest("SHA-256", data);
    return Array.from(new Uint8Array(buf))
      .map((b) => b.toString(16).padStart(2, "0"))
      .join("");
  } catch {
    return "";
  }
}

export function BroadcastModal({ panes, onClose }: Props) {
  const candidates = panes.filter((p) => p.mode !== "zsh");
  const [text, setText] = useState("");
  const [selected, setSelected] = useState<Set<string>>(new Set(candidates.map((p) => p.id)));
  const [appendEnter, setAppendEnter] = useState(true);
  const [sending, setSending] = useState(false);
  const toggle = (id: string) => setSelected((s) => {
    const n = new Set(s);
    if (n.has(id)) n.delete(id); else n.add(id);
    return n;
  });

  const send = async () => {
    if (!text.trim() || sending) return;
    setSending(true);
    const payload = appendEnter ? `${text}\n` : text;
    const correlation = `broadcast-${Date.now()}`;
    const targets = Array.from(selected);

    // BLOQUE A · F4 (Codex tauri-boundary must-fix): fire ALL pty_writes in
    // parallel, then count successes vs failures; only emit broadcast.sent
    // once we have a definitive outcome. Each per-pane error is logged but
    // never blocks the rest.
    const results = await Promise.all(
      targets.map((id) =>
        invoke("pty_write", {
          paneId: id,
          data: payload,
          correlationId: correlation,
          actionId: `${correlation}-${id}`,
        })
          .then(() => true)
          .catch((e) => { console.error("broadcast to", id, e); return false; })
      )
    );
    const success = results.filter(Boolean).length;
    const failure = results.length - success;

    // Hash the message so downstream audit can dedup without storing raw text
    // (security-privacy must-fix: never put the message body into audit).
    const messageHash = await sha256Hex(text);
    const auditPayload: BroadcastAuditInput = {
      count: targets.length,
      length: text.length,
      success_count: success,
      failure_count: failure,
    };
    if (messageHash) auditPayload.message_hash = messageHash;
    try {
      await invoke<void>("broadcast_audit_sent", { payload: auditPayload });
    } catch (e) {
      console.error("broadcast_audit_sent failed", e);
    }

    setSending(false);
    onClose();
  };
  return (
    <Modal
      title="Enviar a varios paneles"
      subtitle={<>El mismo mensaje a todos los paneles seleccionados. <kbd>⌘B</kbd> para abrir, <kbd>⌘↩</kbd> para enviar.</>}
      maxWidth={600}
      onClose={onClose}
      onSubmit={send}
      canSubmit={!sending && !!text.trim() && selected.size > 0}
    >
      <textarea
        value={text}
        onChange={(e) => setText(e.target.value)}
        placeholder="texto a enviar…"
        rows={5}
        autoFocus
        aria-label="Broadcast message"
        style={{ width: "100%", background: "var(--bg2)", border: "1px solid var(--line)", borderRadius: 6, padding: 10, fontFamily: "var(--mono)", fontSize: 12, color: "var(--text)", outline: "none" }}
      />
      <div style={{ marginTop: 14, display: "flex", flexWrap: "wrap", gap: 8 }}>
        {candidates.length === 0 && <div className="muted">no hay panes Claude / Codex / Gemini / Aider abiertos.</div>}
        {candidates.map((p) => (
          <label key={p.id} className="check" style={{ padding: "5px 10px", margin: 0 }}>
            <input type="checkbox" checked={selected.has(p.id)} onChange={() => toggle(p.id)} />
            {p.title} · <code style={{ marginLeft: 4 }}>{p.mode}</code>
          </label>
        ))}
      </div>
      <label className="check" style={{ marginTop: 12, padding: "5px 10px" }}>
        <input type="checkbox" checked={appendEnter} onChange={(e) => setAppendEnter(e.target.checked)} />
        agregar Enter al final
      </label>
      <div className="wizard-actions">
        <button onClick={onClose}>Cancelar</button>
        <Button variant="primary" onClick={send} disabled={sending || !text.trim() || selected.size === 0}>
          {sending ? "Enviando…" : (
            <>Enviar a {selected.size} pane{selected.size === 1 ? "" : "s"} <kbd style={{ marginLeft: 6 }}>⌘↩</kbd></>
          )}
        </Button>
      </div>
    </Modal>
  );
}
