import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Button } from "../components/Button";

// Mirror of bases/allowlist.rs on the frontend side. Defaults are loopback-only
// (no infrastructure baked in); users add their own Grafana hosts via the
// `endpoints.allowlist_extra` setting (CSV / `*.host` for suffix). Validated at
// parse time so a malformed setting can't open the iframe sandbox to arbitrary hosts.
const EXACT_DEFAULTS = ["localhost", "127.0.0.1"] as const;
const SUFFIX_DEFAULTS = [] as const;

function parseExtraAllowlist(raw: unknown): { exact: string[]; suffix: string[] } {
  const exact: string[] = [];
  const suffix: string[] = [];
  if (typeof raw !== "string" || !raw.trim()) return { exact, suffix };
  // Same DNS-label rules as backend parse_host: lowercase, labels ≤63,
  // alnum + dash only, no leading/trailing dot or dash, IPv4 octets ≤255.
  const labelOk = (s: string) => {
    if (s.length === 0 || s.length > 253 || s.startsWith(".") || s.endsWith(".")) return false;
    const parts = s.split(".");
    const allDigits = parts.length === 4 && parts.every((p) => /^\d+$/.test(p));
    if (allDigits) return parts.every((p) => { const n = parseInt(p, 10); return n >= 0 && n <= 255; });
    return parts.every((p) =>
      p.length > 0 && p.length <= 63
      && /^[a-z0-9-]+$/.test(p)
      && !p.startsWith("-") && !p.endsWith("-"));
  };
  for (const piece of raw.split(",")) {
    const t = piece.trim().toLowerCase();
    if (!t) continue;
    if (t.startsWith("*.")) {
      const body = t.slice(2);
      if (labelOk(body)) suffix.push(body);
    } else if (labelOk(t)) {
      exact.push(t);
    }
  }
  return { exact, suffix };
}

function makeIsAllowed(extra: { exact: string[]; suffix: string[] }) {
  const exactAll = new Set<string>([...EXACT_DEFAULTS, ...extra.exact]);
  const suffixAll = [...SUFFIX_DEFAULTS, ...extra.suffix];
  return (u: string): boolean => {
    if (!u) return false;
    try {
      const parsed = new URL(u);
      const h = parsed.hostname.toLowerCase();
      if (exactAll.has(h)) return true;
      return suffixAll.some((s) => h === s || h.endsWith(`.${s}`));
    } catch { return false; }
  };
}

type Hb = { state: "unknown" | "up" | "down"; latency_ms: number | null; checked_at: number | null; error: string | null };

export function GrafanaView() {
  const [url, setUrl] = useState<string>("");
  const [reload, setReload] = useState(0);
  const [isAllowed, setIsAllowed] = useState<(u: string) => boolean>(() => makeIsAllowed({ exact: [], suffix: [] }));
  // BLOQUE F · F14 — 60s HEAD heartbeat so the user sees iframe-vs-network
  // status at a glance (iframe can render a stale snapshot even after the
  // backend went away).
  const [hb, setHb] = useState<Hb>({ state: "unknown", latency_ms: null, checked_at: null, error: null });
  useEffect(() => {
    invoke<unknown>("settings_get", { key: "endpoints.grafana" })
      .then((v) => setUrl(typeof v === "string" ? v : ""))
      .catch((e) => { console.warn("grafana endpoints lookup failed", e); setUrl(""); });
    // Post-J ext 2 hardening: load user-provided extras for the iframe allowlist.
    invoke<unknown>("settings_get", { key: "endpoints.allowlist_extra" })
      .then((v) => setIsAllowed(() => makeIsAllowed(parseExtraAllowlist(v))))
      .catch(() => { /* defaults-only allowlist is fine */ });
  }, []);
  const allowed = isAllowed(url);
  useEffect(() => {
    if (!url || !allowed) return;
    let cancelled = false;
    const ping = async () => {
      const t0 = performance.now();
      try {
        // mode:'no-cors' avoids CORS preflight noise on Grafana root; we only
        // care whether the network reaches the host, not the status code.
        await fetch(url, { method: "HEAD", mode: "no-cors", cache: "no-store" });
        if (!cancelled) {
          setHb({ state: "up", latency_ms: Math.round(performance.now() - t0), checked_at: Date.now(), error: null });
        }
      } catch (e) {
        if (!cancelled) {
          setHb({ state: "down", latency_ms: null, checked_at: Date.now(), error: e instanceof Error ? e.message : String(e) });
        }
      }
    };
    void ping();
    const id = window.setInterval(() => { void ping(); }, 60_000);
    return () => { cancelled = true; window.clearInterval(id); };
  }, [url, allowed, reload]);
  return (
    <div className="page">
      <div className="page-header">
        <div className="page-title">Grafana</div>
        <div className="page-sub">iframe sandbox + host allowlist · setear <code>endpoints.grafana</code> en Settings</div>
      </div>
      {!url
        ? <div className="empty"><div className="head">No URL configurada</div><div className="body muted">Definí <code>endpoints.grafana</code> en Settings.</div></div>
        : !allowed
        ? <div className="card-block info" style={{ borderLeftColor: "var(--red)" }}>URL fuera de allowlist. Defaults: {EXACT_DEFAULTS.join(", ")} + *.{SUFFIX_DEFAULTS.join(", *.")}. Agregar más vía Settings → <code>endpoints.allowlist_extra</code> (CSV, prefix <code>*.</code> para suffix).</div>
        : <>
            <div style={{ marginBottom: 10, display: "flex", gap: 8, alignItems: "center" }}>
              <code style={{ flex: 1, padding: "6px 10px" }}>{url}</code>
              <span
                title={hb.error ?? (hb.checked_at ? `checked ${new Date(hb.checked_at).toLocaleTimeString()}${hb.latency_ms != null ? ` · ${hb.latency_ms}ms` : ""}` : "first check pending")}
                style={{
                  display: "inline-flex", alignItems: "center", gap: 6,
                  padding: "2px 8px", borderRadius: 999,
                  background: hb.state === "up" ? "rgba(107,217,122,.15)" : hb.state === "down" ? "rgba(248,81,73,.15)" : "rgba(160,177,200,.15)",
                  color: hb.state === "up" ? "var(--green, #4f8a45)" : hb.state === "down" ? "var(--red, #c0492f)" : "var(--text2, #a0b1c8)",
                  border: "1px solid currentColor", fontSize: 12,
                }}
                role="status"
                aria-live="polite"
              >
                <span style={{ width: 6, height: 6, borderRadius: "50%", background: "currentColor", display: "inline-block" }} />
                {hb.state === "up" ? "up" : hb.state === "down" ? "down" : "—"}
                {hb.latency_ms != null && hb.state === "up" && (
                  <span style={{ opacity: .8, marginLeft: 4 }}>{hb.latency_ms}ms</span>
                )}
              </span>
              <Button variant="ghost" onClick={() => setReload((r) => r + 1)}>↻ recargar</Button>
            </div>
            <iframe
              key={reload}
              src={url}
              sandbox="allow-scripts allow-same-origin"
              referrerPolicy="no-referrer"
              title="Grafana"
              style={{ width: "100%", height: "calc(100vh - 220px)", background: "#0a0f17", border: "1px solid var(--line)", borderRadius: 6 }}
            />
          </>}
    </div>
  );
}
