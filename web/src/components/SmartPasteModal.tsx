import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { PasteClassification } from "../types";
import { Modal } from "./Modal";
import { Button } from "./Button";

interface Props { focusedPaneId: string | null; onClose: () => void; }

export function SmartPasteModal({ focusedPaneId, onClose }: Props) {
  const [text, setText] = useState<string | null>(null);
  const [cls, setCls] = useState<PasteClassification | null>(null);
  const [err, setErr] = useState<string | null>(null);
  useEffect(() => {
    (async () => {
      try {
        const t = await invoke<string | null>("clipboard_read");
        if (!t || t.trim().length === 0) { setErr("clipboard vacío"); return; }
        setText(t);
        const c = await invoke<PasteClassification>("smartpaste_classify", { text: t });
        setCls(c);
      } catch (e) { setErr(String(e)); }
    })();
  }, []);
  const sendToFocused = async (wrap: string) => {
    if (!focusedPaneId || !text) return;
    const data = wrap.replace("{}", text) + "\n";
    await invoke("pty_write", { paneId: focusedPaneId, data, actionId: null, correlationId: null }).catch(console.error);
    onClose();
  };
  return (
    <Modal title="Smart paste" subtitle="F12 · clipboard classification (sin auto-poll, user-initiated)" maxWidth={620} onClose={onClose}>
      {err && <div className="card-block info" style={{ borderLeftColor: "var(--red)" }}>{err}</div>}
      {cls && (
        <>
          <div style={{ display: "flex", alignItems: "center", gap: 10, marginBottom: 10 }}>
            <span className="sev-tag" style={{ fontSize: 11 }}>{cls.kind}</span>
            <span className="muted" style={{ fontSize: 11 }}>{cls.bytes}B · {cls.lines} lines</span>
          </div>
          <pre style={{ background: "var(--bg2)", border: "1px solid var(--line)", borderRadius: 6, padding: 10, maxHeight: 200, overflow: "auto", fontSize: 11, color: "var(--text)", whiteSpace: "pre-wrap" }}>{cls.preview}</pre>
          <p className="muted" style={{ marginTop: 10, fontSize: 12 }}>{cls.action_hint}</p>
          {!focusedPaneId && <div className="card-block info" style={{ borderLeftColor: "var(--amber)" }}>Sin pane focado — focuseá un pane primero.</div>}
          <div className="wizard-actions">
            <button onClick={onClose}>Cerrar</button>
            <Button variant="primary" disabled={!focusedPaneId} onClick={() => sendToFocused("{}")}>
              Send raw
            </Button>
            <Button variant="primary" disabled={!focusedPaneId} onClick={() => sendToFocused("Acá pego un " + cls.kind + " para que mires:\n\n```\n{}\n```\n\n¿Qué ves mal o qué hacemos?")}>
              Send framed
            </Button>
          </div>
        </>
      )}
    </Modal>
  );
}
