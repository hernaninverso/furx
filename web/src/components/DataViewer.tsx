import { useEffect, useMemo, useState } from "react";

/**
 * spec 004 F3 — JSON/CSV data viewer pane. Hand-rolled (no new deps), read-only,
 * Estética A. Auto-detects JSON vs CSV. Bounded so a huge paste can't hang the render.
 */
const MAX_BYTES = 2_000_000; // 2 MB hard cap on parsed/rendered content
const MAX_JSON_NODES = 5000;  // stop expanding the tree past this many nodes
const MAX_CSV_ROWS = 2000;

type Detected = "json" | "csv" | "text";

function detect(text: string): Detected {
  const t = text.trim();
  if (!t) return "text";
  if (t[0] === "{" || t[0] === "[") {
    try { JSON.parse(t); return "json"; } catch { /* fall through */ }
  }
  // CSV heuristic: ≥2 lines and the first line has a comma (and roughly consistent columns).
  const lines = t.split(/\r?\n/).filter((l) => l.length > 0);
  if (lines.length >= 2 && lines[0].includes(",")) return "csv";
  return "text";
}

// Minimal RFC-4180-ish CSV parse (handles quoted fields + escaped quotes).
function parseCsv(text: string, maxRows: number): string[][] {
  const rows: string[][] = [];
  let row: string[] = [];
  let field = "";
  let inQuotes = false;
  const t = text.replace(/\r\n/g, "\n").replace(/\r/g, "\n");
  for (let i = 0; i < t.length; i++) {
    const c = t[i];
    if (inQuotes) {
      if (c === '"') {
        if (t[i + 1] === '"') { field += '"'; i++; } else inQuotes = false;
      } else field += c;
    } else if (c === '"') inQuotes = true;
    else if (c === ",") { row.push(field); field = ""; }
    else if (c === "\n") {
      row.push(field); rows.push(row); row = []; field = "";
      if (rows.length >= maxRows) break;
    } else field += c;
  }
  if (field.length > 0 || row.length > 0) { row.push(field); rows.push(row); }
  return rows;
}

function JsonNode({ k, v, budget }: { k: string | null; v: unknown; budget: { n: number } }) {
  if (budget.n <= 0) return null;
  budget.n--;
  const label = k !== null ? <span className="dv-key">{k}: </span> : null;
  if (v === null) return <div className="dv-row">{label}<span className="dv-null">null</span></div>;
  if (typeof v !== "object") {
    const cls = typeof v === "number" ? "dv-num" : typeof v === "boolean" ? "dv-bool" : "dv-str";
    return <div className="dv-row">{label}<span className={cls}>{typeof v === "string" ? `"${v}"` : String(v)}</span></div>;
  }
  const entries = Array.isArray(v) ? v.map((item, i) => [String(i), item] as const) : Object.entries(v as Record<string, unknown>);
  const open = entries.length <= 20;
  return (
    <details open={open} className="dv-node">
      <summary>{label}<span className="dv-meta">{Array.isArray(v) ? `[${entries.length}]` : `{${entries.length}}`}</span></summary>
      <div className="dv-children">
        {entries.map(([ck, cv]) => <JsonNode key={ck} k={ck} v={cv} budget={budget} />)}
      </div>
    </details>
  );
}

export default function DataViewer({
  initialContent, onContentChange,
}: {
  initialContent?: string;
  onContentChange?: (content: string) => void;
}) {
  const [content, setContent] = useState(initialContent ?? "");
  // Resync when the parent delivers new content (e.g. piped from another pane via the
  // inter-pane send modal). Functional guard → only applies when it actually differs, so
  // it never clobbers/cursors-reset during the user's own typing (same string → no-op).
  useEffect(() => {
    setContent((cur) => (initialContent !== undefined && initialContent !== cur ? initialContent : cur));
  }, [initialContent]);
  const tooBig = content.length > MAX_BYTES;
  const kind = useMemo<Detected>(() => (tooBig ? "text" : detect(content)), [content, tooBig]);

  const parsed = useMemo(() => {
    if (tooBig || !content.trim()) return null;
    if (kind === "json") { try { return JSON.parse(content); } catch { return null; } }
    if (kind === "csv") return parseCsv(content, MAX_CSV_ROWS);
    return null;
  }, [content, kind, tooBig]);

  function update(v: string) {
    setContent(v);
    onContentChange?.(v);
  }

  if (!content.trim()) {
    return (
      <div className="dv-empty">
        <p className="dv-hint">Paste JSON or CSV — or send another pane&apos;s output here (→).</p>
        <textarea className="dv-input" placeholder="{ }  or  a,b,c…" value={content} onChange={(e) => update(e.target.value)} spellCheck={false} />
      </div>
    );
  }

  return (
    <div className="dv-wrap">
      <div className="dv-bar">
        <span className="dv-badge">{tooBig ? "text (too large)" : kind}</span>
        <span className="dv-len">{content.length.toLocaleString()} chars</span>
        <button className="dv-clear" onClick={() => update("")} title="Clear">clear</button>
      </div>
      <div className="dv-out">
        {tooBig ? (
          <pre className="dv-pre">{content.slice(0, 100_000) + `\n… (truncated, ${content.length.toLocaleString()} chars total)`}</pre>
        ) : kind === "json" && parsed !== null ? (
          <div className="dv-json"><JsonNode k={null} v={parsed} budget={{ n: MAX_JSON_NODES }} /></div>
        ) : kind === "csv" && Array.isArray(parsed) ? (
          <table className="dv-table">
            <tbody>
              {(parsed as string[][]).map((r, ri) => (
                <tr key={ri} className={ri === 0 ? "dv-head" : ""}>
                  {r.map((cell, ci) => ri === 0 ? <th key={ci}>{cell}</th> : <td key={ci}>{cell}</td>)}
                </tr>
              ))}
            </tbody>
          </table>
        ) : (
          <pre className="dv-pre">{content}</pre>
        )}
      </div>
    </div>
  );
}
