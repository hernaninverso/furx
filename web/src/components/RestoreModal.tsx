// F / F24 — Boot-restore modal. Shown when FURX_* tmux sessions exist at startup.

import { invoke } from "@tauri-apps/api/core";
import { Modal } from "./Modal";
import { Button } from "./Button";

export interface FurxSession { name: string; created: string | null; }

interface RestoreUiPayload {
  schema_version: number;
  panes: Array<{ id: string; mode: string; title?: string }> | null;
  layout: { panes?: unknown; grid_cols?: string; grid_rows?: string } | null;
}

interface Props {
  sessions: FurxSession[];
  onClose: () => void;
  onRestoreUi?: (payload: RestoreUiPayload) => void;
}

export function RestoreModal({ sessions, onClose, onRestoreUi }: Props) {
  const handle = async (mode: "attach" | "ui" | "full") => {
    if (mode === "full" && !confirm("Full restore matará el tmux server actual. ¿Seguro?")) {
      return;
    }
    try {
      if (mode === "attach") await invoke("boot_restore_attach");
      if (mode === "ui") {
        const payload = await invoke<RestoreUiPayload | null>("boot_restore_ui");
        if (payload) onRestoreUi?.(payload);
      }
      if (mode === "full") await invoke("boot_restore_full");
    } catch (e) {
      console.error("boot restore", mode, e);
    }
    onClose();
  };

  const display = sessions.slice(0, 20);
  const overflow = sessions.length - display.length;

  return (
    <Modal
      title="Restore previous session?"
      subtitle={`Detected ${sessions.length} FURX_* tmux session${sessions.length === 1 ? "" : "s"} from a previous run.`}
      maxWidth={540}
      onClose={onClose}
    >
      {sessions.length === 0
        ? <p className="muted">No sessions to restore.</p>
        : <ul style={{ paddingLeft: 18, marginBottom: 12, maxHeight: 200, overflowY: "auto", fontFamily: "var(--mono)", fontSize: 12 }}>
            {display.map((s) => <li key={s.name}>{s.name}{s.created ? ` · ${s.created}` : ""}</li>)}
            {overflow > 0 && <li className="muted">+ {overflow} more…</li>}
          </ul>}
      <p style={{ fontSize: 12, color: "var(--muted)", marginTop: 8 }}>
        <strong>Attach existing</strong>: each pane re-attaches to its named tmux session (zero data loss).<br />
        <strong>Restore UI only</strong>: re-paints the panes layout; tmux state untouched.<br />
        <strong>Full restart</strong>: kills tmux server, starts fresh (loses scrollback).
      </p>
      <div className="wizard-actions">
        <Button variant="primary" onClick={() => handle("attach")}>Attach existing</Button>
        <button onClick={() => handle("ui")}>Restore UI only</button>
        <Button variant="danger" onClick={() => handle("full")}>Full restart</Button>
        <button onClick={onClose}>Dismiss</button>
      </div>
    </Modal>
  );
}
