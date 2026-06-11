// 1.6 — Auto-standup modal. Click → invoke standup_today → render markdown.

import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Modal } from "./Modal";
import { Button } from "./Button";

interface Props { onClose: () => void; }

export function StandupModal({ onClose }: Props) {
  const [md, setMd] = useState<string | null>(null);
  const [busy, setBusy] = useState(true);
  const [err, setErr] = useState<string | null>(null);
  useEffect(() => {
    invoke<string>("standup_today")
      .then((r) => setMd(r))
      .catch((e) => setErr(String(e)))
      .finally(() => setBusy(false));
  }, []);
  return (
    <Modal title="Daily standup" subtitle="Auto-generado desde audit + cards open" maxWidth={720} onClose={onClose}>
      {busy && <div className="muted">generando…</div>}
      {err && <div className="card-block info" style={{ borderLeftColor: "var(--red)" }}>error: {err}</div>}
      {md && <pre style={{ background: "var(--bg2)", border: "1px solid var(--line)", borderRadius: 6, padding: 12, maxHeight: 420, overflowY: "auto", fontSize: 12, color: "var(--text)", whiteSpace: "pre-wrap", fontFamily: "var(--mono)" }}>{md}</pre>}
      <div className="wizard-actions">
        <button onClick={onClose}>Cerrar</button>
        {md && <Button variant="primary" onClick={() => navigator.clipboard.writeText(md).then(onClose)}>Copy</Button>}
      </div>
    </Modal>
  );
}
