// 1.9 — Auto-PR description modal.

import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Modal } from "./Modal";
import { Button } from "./Button";

interface PrDescription { branch: string; markdown: string; commits_count: number; elapsed_ms: number; }

interface Props { defaultRepo: string; onClose: () => void; }

export function PrDescriptionModal({ defaultRepo, onClose }: Props) {
  const [repo, setRepo] = useState(defaultRepo);
  // Codex MED: if defaultRepo arrives after mount (e.g. home_dir invoke), adopt it
  // unless the user has already edited the field.
  const userEdited = useRef(false);
  useEffect(() => {
    if (!userEdited.current && defaultRepo && defaultRepo !== repo) {
      setRepo(defaultRepo);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [defaultRepo]);
  const [base, setBase] = useState("master");
  const [data, setData] = useState<PrDescription | null>(null);
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<string | null>(null);
  const run = async () => {
    setBusy(true); setErr(null);
    try { setData(await invoke<PrDescription>("pr_description", { repoPath: repo, base })); }
    catch (e) { setErr(String(e)); }
    finally { setBusy(false); }
  };
  return (
    <Modal title="Auto-PR description" subtitle="audit + git diff stat → AIE markdown" maxWidth={820} onClose={onClose}>
      <div className="form-row">
        <label>Repo path</label>
        <div className="form-input">
          <input value={repo} onChange={(e) => { userEdited.current = true; setRepo(e.target.value); }} />
        </div>
      </div>
      <div className="form-row">
        <label>Base ref</label>
        <div className="form-input">
          <input value={base} onChange={(e) => setBase(e.target.value)} />
          <Button variant="primary" onClick={run} disabled={busy || !repo.trim()}>{busy ? "generando…" : "Generate"}</Button>
        </div>
      </div>
      {err && <div className="card-block info" style={{ borderLeftColor: "var(--red)" }}>error: {err}</div>}
      {data && (
        <>
          <div className="muted" style={{ fontSize: 11, marginTop: 10 }}>
            branch <code>{data.branch}</code> · {data.commits_count} commits · {data.elapsed_ms}ms
          </div>
          <pre style={{ background: "var(--bg2)", border: "1px solid var(--line)", borderRadius: 6, padding: 12, marginTop: 8, maxHeight: 360, overflowY: "auto", fontSize: 12, color: "var(--text)", whiteSpace: "pre-wrap", fontFamily: "var(--mono)" }}>{data.markdown}</pre>
        </>
      )}
      <div className="wizard-actions">
        <button onClick={onClose}>Cerrar</button>
        {data && <Button variant="primary" onClick={() => navigator.clipboard.writeText(data.markdown).then(onClose)}>Copy markdown</Button>}
      </div>
    </Modal>
  );
}
