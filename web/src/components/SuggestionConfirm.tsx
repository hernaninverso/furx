// I — Confirm modal before any pty_write triggered by a suggestion badge.
// V1+V2 reviewers flagged auto-write as a foot-gun.

import { useEffect } from "react";
import { Button } from "./Button";

export interface SuggestionAction {
  kind: string;
  label: string;
  hint: string;
  /** The exact text that will be sent to the PTY (with trailing \n). */
  pty_text: string;
}

interface Props {
  paneTitle: string;
  action: SuggestionAction;
  onConfirm: () => void;
  onCancel: () => void;
}

export function SuggestionConfirm({ paneTitle, action, onConfirm, onCancel }: Props) {
  useEffect(() => {
    const k = (e: KeyboardEvent) => {
      if (e.key === "Escape") onCancel();
      if (e.key === "Enter") { e.preventDefault(); onConfirm(); }
    };
    window.addEventListener("keydown", k);
    return () => window.removeEventListener("keydown", k);
  }, [onConfirm, onCancel]);

  return (
    <div className="wizard-backdrop" onClick={onCancel}>
      <div className="wizard" onClick={(e) => e.stopPropagation()} role="dialog" aria-modal="true" aria-label="Confirm suggestion action" style={{ maxWidth: 480 }}>
        <header className="wizard-header">
          <span className="hex" />
          <div>
            <h2>Confirm action · {action.label}</h2>
            <div className="muted">Pane <code>{paneTitle}</code> · {action.hint}</div>
          </div>
        </header>
        <main className="wizard-body">
          <p style={{ marginBottom: 8 }}>Voy a escribir esto al pane:</p>
          <pre style={{ background: "var(--bg2)", border: "1px solid var(--line)", borderRadius: 6, padding: 10, fontSize: 12, color: "var(--cyan)", whiteSpace: "pre-wrap", fontFamily: "var(--mono)", maxHeight: 200, overflowY: "auto" }}>{action.pty_text}</pre>
          <div className="wizard-actions">
            <button onClick={onCancel}>Cancel</button>
            <Button variant="primary" onClick={onConfirm}>Send <kbd style={{ marginLeft: 6 }}>↩</kbd></Button>
          </div>
        </main>
      </div>
    </div>
  );
}
