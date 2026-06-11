import { useState } from "react";

/**
 * spec 004 F1 — web (context-viewer) pane. A PINNED URL in a sandboxed iframe — NOT a
 * general browser (no free address bar, no tabs/history). Council 6/6: conditional yes,
 * for docs/dashboards/localhost while the LLM works.
 *
 * BYOK / security (F-I): a sandboxed <iframe> has NO access to the Tauri IPC bridge
 * (only the top-level webview does), so embedded content cannot reach the Keychain or
 * any secret-reading command. `sandbox` omits `allow-same-origin` + `allow-top-navigation`
 * (can't read app-origin storage, can't break out) and `allow-popups` (no phishing windows).
 * Note: like any iframe it can still issue requests to the user's local network (localhost,
 * private IPs) — inherent to a context-viewer and the council's intended use; we assume the
 * user only pins trusted URLs.
 */
function normalizeUrl(raw: string): string | null {
  // Strip surrounding whitespace AND internal control/whitespace chars (\t \r \n, NBSP,
  // BOM) that the WHATWG URL parser would silently remove, so a crafted
  // "https://x\njavascript:…" can't sneak past the scheme check (audit hardening).
  const t = raw.replace(/[\s\u0000-\u001F\u007F-\u009F\u00A0\uFEFF]+/g, "");
  if (!t) return null;
  const withScheme = /^https?:\/\//i.test(t) ? t : `https://${t}`;
  try {
    const u = new URL(withScheme);
    if (u.protocol !== "http:" && u.protocol !== "https:") return null;
    // Defense in depth: never return a dangerous-scheme href.
    if (/^(javascript|data|file|blob|about):/i.test(u.href)) return null;
    return u.toString();
  } catch {
    return null;
  }
}

export default function WebPane({
  url, onPin,
}: {
  url?: string;
  onPin: (url: string) => void;
}) {
  const [draft, setDraft] = useState(url ?? "");
  const pinned = url ? normalizeUrl(url) : null;

  if (!pinned) {
    return (
      <div className="wp-empty">
        <p className="wp-hint">Pin a URL — docs, a dashboard, a localhost preview. Not a general browser.</p>
        <form
          className="wp-pinform"
          onSubmit={(e) => { e.preventDefault(); const n = normalizeUrl(draft); if (n) onPin(n); }}
        >
          <input
            className="wp-input" type="text" value={draft} spellCheck={false}
            placeholder="https://docs.example.com  ·  http://localhost:3000"
            onChange={(e) => setDraft(e.target.value)}
          />
          <button className="wp-go" type="submit" disabled={!normalizeUrl(draft)}>Pin</button>
        </form>
      </div>
    );
  }

  return (
    <div className="wp-wrap">
      <div className="wp-bar">
        <span className="wp-badge">web</span>
        <span className="wp-url" title={pinned}>{pinned}</span>
        <button className="wp-edit" onClick={() => onPin("")} title="Unpin / change URL">edit</button>
      </div>
      <iframe
        className="wp-frame"
        src={pinned}
        title="Furx web pane"
        sandbox="allow-scripts allow-forms"
        referrerPolicy="no-referrer"
      />
    </div>
  );
}
