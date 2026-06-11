// BLOQUE 3 · Settings → Connect status panel.
// Lista todas las provider_credentials con badge, latency, last error, re-test, delete.
// Event-driven hot-reload (listen al evento `provider.test` que el audit emite).

import { useEffect, useState, useCallback, useRef } from "react";
import { invoke } from "../lib/invoke"; // 015 T015: invoke con flujo de aprobación universal
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type { ProviderCredential, PingResult } from "../types";
import { Button } from "./Button";

interface Props {
  onOpenConnect: () => void;
}

export function ConnectStatusPanel({ onOpenConnect }: Props) {
  const [creds, setCreds] = useState<ProviderCredential[]>([]);
  const [busy, setBusy] = useState<string | null>(null);
  const [msg, setMsg] = useState<string | null>(null);
  // MED-4 fix (Codex B3): per-call generation counter — drop stale responses.
  const refreshGen = useRef(0);
  const mounted = useRef(true);

  const refresh = useCallback(async () => {
    const my = ++refreshGen.current;
    try {
      const list = await invoke<ProviderCredential[]>("provider_list");
      if (mounted.current && my === refreshGen.current) {
        setCreds(list);
      }
    } catch (e) {
      if (mounted.current) setMsg(`Error: ${e}`);
    }
  }, []);

  useEffect(() => {
    mounted.current = true;
    refresh();
    // MED-3 fix (Codex B3): also handle the case where listen() resolves AFTER
    // the component unmounts — use a setter so cleanup always sees the latest.
    let unlistenRef: { current: UnlistenFn | undefined } = { current: undefined };
    let cancelled = false;
    (async () => {
      try {
        const fn = await listen("provider:changed", () => {
          if (mounted.current) refresh();
        });
        if (cancelled) {
          fn(); // already unmounted — clean up immediately
        } else {
          unlistenRef.current = fn;
        }
      } catch { /* ignore */ }
    })();
    return () => {
      mounted.current = false;
      cancelled = true;
      if (unlistenRef.current) unlistenRef.current();
    };
  }, [refresh]);

  const testOne = async (alias: string) => {
    setBusy(alias); setMsg(null);
    try {
      const r = await invoke<PingResult>("provider_test", { alias });
      setMsg(r.ok ? `${alias} · ✓ ${r.latency_ms} ms` : `${alias} · ✗ ${r.error ?? ""}`);
      await refresh();
    } catch (e) { setMsg(String(e)); }
    finally { setBusy(null); }
  };

  const delOne = async (alias: string) => {
    if (!confirm(`Eliminar provider ${alias}?`)) return;
    setBusy(alias); setMsg(null);
    try {
      await invoke<boolean>("provider_delete", { alias });
      await refresh();
      setMsg(`${alias} eliminado`);
    } catch (e) { setMsg(String(e)); }
    finally { setBusy(null); }
  };

  return (
    <div className="connect-status-panel">
      {msg && <div className="card-block info">{msg}</div>}
      {creds.length === 0 ? (
        <div className="muted">
          Sin providers conectados. <a role="button" onClick={onOpenConnect}>Conectar primero</a>.
        </div>
      ) : (
        <div className="status-list">
          {creds.map((c) => (
            <div key={c.alias} className="key-row">
              <span className={`status-dot ${dotClass(c.status)}`} />
              <div style={{ flex: 1 }}>
                <code>{c.alias}</code>
                <span className="muted small"> · {c.provider}</span>
                {c.last_ping_ms !== null && (
                  <span className="muted small"> · {c.last_ping_ms} ms</span>
                )}
                {c.last_error_msg && (
                  <div className="muted small" title={c.last_error_msg}>
                    {c.last_error_msg.slice(0, 80)}
                  </div>
                )}
              </div>
              <span className="actions">
                <button onClick={() => testOne(c.alias)} disabled={busy === c.alias}>
                  {busy === c.alias ? "…" : "Test"}
                </button>
                <Button variant="danger" onClick={() => delOne(c.alias)} disabled={busy === c.alias}>
                  Eliminar
                </Button>
              </span>
            </div>
          ))}
        </div>
      )}
      <div className="wizard-actions" style={{ marginTop: 12 }}>
        <Button variant="primary" onClick={onOpenConnect}>Agregar más</Button>
      </div>
    </div>
  );
}

function dotClass(status: string): string {
  switch (status) {
    case "healthy": return "green";
    case "amber": return "amber";
    case "red": return "red";
    default: return "gray";
  }
}
