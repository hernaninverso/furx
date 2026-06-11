import { useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

interface CouncilVoice { voice_alias: string; model: string; status: string; voice_position: number; response: string; }
interface RegressionCase { case_id: string; verdict: string; baseline: string; candidate: string; }
interface RecentCouncil { id: string; model: string; ts: number; child_count: number; }

function relTime(ts: number): string {
  const d = (Date.now() - ts) / 1000;
  if (d < 60) return `${Math.floor(d)}s`;
  if (d < 3600) return `${Math.floor(d / 60)}m`;
  if (d < 86400) return `${Math.floor(d / 3600)}h`;
  return `${Math.floor(d / 86400)}d`;
}

/**
 * spec 004 F4 — response comparator pane. Two responses side-by-side with line-level
 * diff highlighting (Council voices, or a regression candidate vs baseline). Hand-rolled
 * LCS diff (no deps), read-only render, Estética A. Sources: paste / piped pane output
 * (auto-pulling council/regression results is a follow-up).
 */
const MAX = 200_000;
// Audit: the LCS dp table is O(n×m) lines. Cap line counts so a pathological paste
// (thousands of tiny lines) can't allocate a huge table / hang the render.
const MAX_LINES = 1500;

type Line = { text: string; tag: "same" | "add" | "del" };

// Classic LCS over lines → aligned left/right with same/add/del tags.
function diffLines(a: string, b: string): { left: Line[]; right: Line[] } {
  const A = a.split("\n");
  const B = b.split("\n");
  const n = A.length, m = B.length;
  // LCS length table (capped to keep it cheap on big inputs).
  const dp: number[][] = Array.from({ length: n + 1 }, () => new Array(m + 1).fill(0));
  for (let i = n - 1; i >= 0; i--)
    for (let j = m - 1; j >= 0; j--)
      dp[i][j] = A[i] === B[j] ? dp[i + 1][j + 1] + 1 : Math.max(dp[i + 1][j], dp[i][j + 1]);
  const left: Line[] = [], right: Line[] = [];
  let i = 0, j = 0;
  while (i < n && j < m) {
    if (A[i] === B[j]) { left.push({ text: A[i], tag: "same" }); right.push({ text: B[j], tag: "same" }); i++; j++; }
    else if (dp[i + 1][j] >= dp[i][j + 1]) { left.push({ text: A[i], tag: "del" }); right.push({ text: "", tag: "same" }); i++; }
    else { left.push({ text: "", tag: "same" }); right.push({ text: B[j], tag: "add" }); j++; }
  }
  while (i < n) { left.push({ text: A[i++], tag: "del" }); right.push({ text: "", tag: "same" }); }
  while (j < m) { left.push({ text: "", tag: "same" }); right.push({ text: B[j++], tag: "add" }); }
  return { left, right };
}

function Col({ lines, side }: { lines: Line[]; side: "l" | "r" }) {
  return (
    <div className="cmp-col">
      {lines.map((ln, i) => (
        <div key={i} className={`cmp-line cmp-${ln.tag === "same" ? "same" : side === "l" ? "del" : "add"}`}>
          {ln.text || " "}
        </div>
      ))}
    </div>
  );
}

export default function ComparatorView({
  initialLeft, initialRight, onChange,
}: {
  initialLeft?: string;
  initialRight?: string;
  onChange?: (left: string, right: string) => void;
}) {
  const [left, setLeft] = useState(initialLeft ?? "");
  const [right, setRight] = useState(initialRight ?? "");
  const [editing, setEditing] = useState(!initialLeft && !initialRight);
  // Resync when a side is delivered externally (piped from another pane). Functional
  // guard → no clobber during the user's own typing.
  useEffect(() => { setLeft((cur) => (initialLeft !== undefined && initialLeft !== cur ? initialLeft : cur)); }, [initialLeft]);
  useEffect(() => { setRight((cur) => (initialRight !== undefined && initialRight !== cur ? initialRight : cur)); }, [initialRight]);

  // spec 004 F4 — load real Council voices (with response text) from the cloud by parent
  // trace id; pick which voice shows on each side. The actual responses are resolved
  // server-side (R2) in one call.
  const [traceId, setTraceId] = useState("");
  const [voices, setVoices] = useState<CouncilVoice[]>([]);
  const [loadErr, setLoadErr] = useState<string | null>(null);
  const [loadingVoices, setLoadingVoices] = useState(false);
  // Recent council parents for the dropdown (loaded once when the load view is shown).
  const [recents, setRecents] = useState<RecentCouncil[]>([]);
  useEffect(() => {
    if (!editing) return;
    let off = false;
    invoke<RecentCouncil[]>("cloud_recent_councils")
      .then((r) => { if (!off) setRecents(r); })
      .catch(() => { /* not signed in / offline — dropdown just stays empty */ });
    return () => { off = true; };
  }, [editing]);

  async function loadCouncil(idOverride?: string) {
    const tid = (idOverride ?? traceId).trim();
    if (!tid) return;
    if (idOverride) setTraceId(idOverride);
    setLoadingVoices(true); setLoadErr(null); setRegCases([]); // clear the other source (audit)
    try {
      const vs = await invoke<CouncilVoice[]>("cloud_council_compare", { traceId: tid });
      if (!vs.length) { setLoadErr("no voices found for that trace id"); return; }
      setVoices(vs);
      set(vs[0]?.response ?? "", vs[1]?.response ?? vs[0]?.response ?? "");
      setEditing(false);
    } catch (e) {
      setLoadErr(typeof e === "string" ? e : String(e));
    } finally { setLoadingVoices(false); }
  }
  function pickVoice(side: "l" | "r", idx: number) {
    const r = voices[idx]?.response ?? "";
    if (side === "l") set(r, right); else set(left, r);
  }

  // F4 regression source — candidate (left) vs baseline (right) per case.
  const [regCases, setRegCases] = useState<RegressionCase[]>([]);
  async function loadRegression() {
    if (!traceId.trim()) return;
    setLoadingVoices(true); setLoadErr(null); setVoices([]); // clear the other source (audit)
    try {
      const cs = await invoke<RegressionCase[]>("cloud_regression_compare", { runId: traceId.trim() });
      if (!cs.length) { setLoadErr("no cases found for that run id"); return; }
      setRegCases(cs);
      set(cs[0]?.candidate ?? "", cs[0]?.baseline ?? "");
      setEditing(false);
    } catch (e) {
      setLoadErr(typeof e === "string" ? e : String(e));
    } finally { setLoadingVoices(false); }
  }
  function pickCase(idx: number) {
    const c = regCases[idx];
    if (c) set(c.candidate, c.baseline);
  }

  const tooBig = left.length > MAX || right.length > MAX
    || left.split("\n").length > MAX_LINES || right.split("\n").length > MAX_LINES;
  const diff = useMemo(() => (editing || tooBig ? null : diffLines(left, right)), [left, right, editing, tooBig]);

  function set(l: string, r: string) {
    setLeft(l); setRight(r); onChange?.(l, r);
  }

  if (editing) {
    return (
      <div className="cmp-edit">
        {recents.length > 0 && (
          <select className="cmp-load-input" defaultValue="" onChange={(e) => { if (e.target.value) loadCouncil(e.target.value); }}>
            <option value="">recent councils…</option>
            {recents.map((r) => (
              <option key={r.id} value={r.id}>{r.model} · {relTime(r.ts)} ago · {r.child_count} voices</option>
            ))}
          </select>
        )}
        <div className="cmp-load">
          <input className="cmp-load-input" value={traceId} spellCheck={false}
            placeholder="council trace id / regression run id…"
            onChange={(e) => setTraceId(e.target.value)} />
          <button className="cmp-load-btn" disabled={loadingVoices || !traceId.trim()} onClick={() => loadCouncil()}>council</button>
          <button className="cmp-load-btn" disabled={loadingVoices || !traceId.trim()} onClick={loadRegression}>regression</button>
        </div>
        {loadErr && <div className="cmp-err">{loadErr}</div>}
        <textarea className="cmp-input" placeholder="Response A (paste / pipe ← / load council)" value={left} onChange={(e) => set(e.target.value, right)} spellCheck={false} />
        <textarea className="cmp-input" placeholder="Response B" value={right} onChange={(e) => set(left, e.target.value)} spellCheck={false} />
        <button className="cmp-go" disabled={!left && !right} onClick={() => setEditing(false)}>Compare ⊟</button>
      </div>
    );
  }

  return (
    <div className="cmp-wrap">
      <div className="cmp-bar">
        <span className="cmp-badge">diff</span>
        {voices.length > 0 ? (
          <>
            <select className="cmp-pick" onChange={(e) => pickVoice("l", Number(e.target.value))} title="Left voice" defaultValue="0">
              {voices.map((v, i) => <option key={i} value={i}>L: {v.voice_alias || v.model}</option>)}
            </select>
            <select className="cmp-pick" onChange={(e) => pickVoice("r", Number(e.target.value))} title="Right voice" defaultValue="1">
              {voices.map((v, i) => <option key={i} value={i}>R: {v.voice_alias || v.model}</option>)}
            </select>
          </>
        ) : regCases.length > 0 ? (
          <select className="cmp-pick" onChange={(e) => pickCase(Number(e.target.value))} title="Case (candidate vs baseline)" defaultValue="0">
            {regCases.map((c, i) => <option key={i} value={i}>case {i + 1} · {c.verdict}</option>)}
          </select>
        ) : (
          <span className="cmp-len">{left.split("\n").length} / {right.split("\n").length} lines</span>
        )}
        <button className="cmp-edit-btn" onClick={() => setEditing(true)}>edit</button>
      </div>
      {tooBig ? (
        <pre className="cmp-pre">(inputs too large to diff — {left.length + right.length} chars)</pre>
      ) : (
        <div className="cmp-grid">
          <Col lines={diff!.left} side="l" />
          <Col lines={diff!.right} side="r" />
        </div>
      )}
    </div>
  );
}
