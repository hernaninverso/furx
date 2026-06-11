// F8 — Merge review modal. Triggered by `furx:merge-suggest` event (emitted
// by services/merge_watcher.rs when ~/.furx/worktrees/<...> changes), or
// manually from the cards rail. Reads diff stat + risky paths from the
// existing `worktree_merge_review` Tauri command (no execute).
//
// US8 (spec 015): migrado a ModalFrame canónico como prueba del patrón.
// Mismo comportamiento (loading/error/risky/diff + Cerrar) — solo cambia el
// frame: header/body/footer canónicos + estados loading/error del frame.

import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { ModalFrame } from "./canonical";

interface MergeReview {
  branch: string;
  diff_stat: string;
  risky_paths: string[];
}

interface Props {
  repoPath: string;
  branch: string;
  onClose: () => void;
}

export function MergeReviewModal({ repoPath, branch, onClose }: Props) {
  const [review, setReview] = useState<MergeReview | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState<boolean>(true);

  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        const r = await invoke<MergeReview>("worktree_merge_review", {
          repoPath, branch,
        });
        if (!cancelled) { setReview(r); setError(null); }
      } catch (e) {
        if (!cancelled) setError(e instanceof Error ? e.message : String(e));
      } finally {
        if (!cancelled) setLoading(false);
      }
    })();
    return () => { cancelled = true; };
  }, [repoPath, branch]);

  const hasRisky = (review?.risky_paths.length ?? 0) > 0;

  return (
    <ModalFrame
      title={`Merge review · ${branch}`}
      subtitle={<>F8 · diff stat + risky paths (no execute)</>}
      maxWidth={700}
      onClose={onClose}
      loading={loading}
      error={error ? `merge review failed: ${error}` : null}
      footer={
        <button type="button" className="fxc-btn" onClick={onClose}>
          Cerrar
        </button>
      }
    >
      <div className="muted" style={{ fontSize: 11, fontFamily: "var(--mono)", marginBottom: 8 }}>
        repo: <code>{repoPath}</code>
      </div>
      {review && (
        <>
          {hasRisky && (
            <div className="card-block warn" role="alert" style={{ borderLeftColor: "var(--red)", marginBottom: 10 }}>
              <strong>⚠ risky paths</strong>
              <ul style={{ margin: "6px 0 0 16px", padding: 0, fontFamily: "var(--mono)", fontSize: 12 }}>
                {review.risky_paths.map((p) => <li key={p}>{p}</li>)}
              </ul>
            </div>
          )}
          <div className="muted small" style={{ marginBottom: 6 }}>diff --stat ...{review.branch}</div>
          <pre
            aria-label="git diff --stat"
            style={{
              background: "var(--bg2)", border: "1px solid var(--line)", borderRadius: 6,
              padding: 10, maxHeight: 360, overflow: "auto", fontSize: 11,
              fontFamily: "var(--mono)", color: "var(--text)", whiteSpace: "pre-wrap",
              margin: 0,
            }}
          >
            {review.diff_stat || "(no changes)"}
          </pre>
        </>
      )}
    </ModalFrame>
  );
}
