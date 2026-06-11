import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { SshHost, SshHostPing, PaneCfg } from "../types";
import { Button } from "../components/Button";

interface FurxSession { name: string; created: string | null }

interface Props {
  panes: PaneCfg[];
  focusedPane: string | null;
  /** BLOQUE G · F22 — when set, clicking a host opens a NEW pane via the
   *  shell-managed addPane flow instead of writing `ssh host\n` into the
   *  currently focused pane (which could corrupt an in-progress command). */
  onOpenSshPane?: (host: SshHost) => void;
}

const SSH_POLL_MS = 30_000;

export function SshView({ panes, focusedPane, onOpenSshPane }: Props) {
  const [hosts, setHosts] = useState<SshHost[]>([]);
  const [pings, setPings] = useState<Record<string, SshHostPing>>({});
  const [whisper, setWhisper] = useState<{ ready: boolean; install_hint: string; whisper_cli: string | null; sox: string | null; model_path: string | null } | null>(null);
  const load = async () => {
    const h = await invoke<SshHost[]>("ssh_hosts").catch(() => []);
    setHosts(h);
  };
  const refreshAll = async () => {
    for (const h of hosts) {
      try {
        const p = await invoke<SshHostPing>("ssh_ping", { hostName: h.name });
        setPings((prev) => ({ ...prev, [h.name]: p }));
      } catch {
        /* leave previous ping in place on transient failures */
      }
    }
  };
  useEffect(() => { void load(); invoke<typeof whisper>("whisper_check").then(setWhisper).catch((e) => { console.warn("whisper_check unavailable", e); }); }, []);
  useEffect(() => {
    if (hosts.length === 0) return;
    void refreshAll();
    // BLOQUE G · F22 — Codex gap: hosts now auto-poll every 30s.
    const id = window.setInterval(() => { void refreshAll(); }, SSH_POLL_MS);
    return () => window.clearInterval(id);
  // refreshAll only depends on `hosts` for the closure capture; React deps lint
  // would force us to include it, but adding it would loop. The setInterval
  // re-creates whenever hosts changes anyway.
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [hosts]);

  const connect = (h: SshHost) => {
    // BLOQUE G · F22 — Codex gap: spawn a NEW pane instead of writing ssh into
    // an existing pane (avoids clobbering an active command). Fall back to the
    // legacy write-into-focused-pane behaviour when the host doesn't supply an
    // onOpenSshPane callback (back-compat for callers that haven't wired it).
    if (onOpenSshPane) {
      onOpenSshPane(h);
      return;
    }
    const target = focusedPane ?? panes[0]?.id;
    const cmd = `ssh ${h.name}\n`;
    if (target) invoke("pty_write", { paneId: target, data: cmd, actionId: null, correlationId: null }).catch(console.error);
  };
  return (
    <div className="page">
      <div className="page-header">
        <div className="page-title">SSH · ~/.ssh/config</div>
        <div className="page-sub">{hosts.length} hosts · click conecta en el pane focado</div>
      </div>
      <Button variant="ghost" onClick={refreshAll}>ping all</Button>
      {hosts.length === 0
        ? <div className="empty"><span className="glyph" /><div className="head">Sin hosts</div><div className="body muted">No se encontraron Host blocks en ~/.ssh/config.</div></div>
        : <div className="mon-grid" style={{ marginTop: 14 }}>
            {hosts.map((h) => {
              const p = pings[h.name];
              return (
                <div key={h.name} className={`mon ${p?.up ? "up" : (p ? "down" : "")}`} onClick={() => connect(h)} style={{ cursor: "pointer" }}>
                  <div className="mon-head">
                    <span className={`dot ${p?.up ? "up" : (p ? "down" : "unknown")}`} />
                    <span className="mon-label">{h.name}</span>
                    <span className="mon-addr muted">{h.hostname ?? h.name}:{h.port}</span>
                  </div>
                  <div className="mon-body">
                    {p
                      ? (p.up ? <>up · <strong>{p.latency_ms}ms</strong></> : <span className="muted">down · {p.error}</span>)
                      : <span className="muted">checking…</span>}
                    {h.user && <span className="muted" style={{ marginLeft: 10 }}>user: {h.user}</span>}
                  </div>
                </div>
              );
            })}
          </div>}
      {whisper && (
        <div className="card-block info" style={{ marginTop: 18, borderLeftColor: whisper.ready ? "var(--green)" : "var(--amber)" }}>
          <strong>F19 whisper voice · {whisper.ready ? "ready" : "needs setup"}</strong>
          <div className="muted" style={{ fontSize: 11, fontFamily: "var(--mono)", marginTop: 6 }}>
            whisper-cli: {whisper.whisper_cli ?? "—"} · sox: {whisper.sox ?? "—"} · model: {whisper.model_path ?? "—"}
          </div>
          {!whisper.ready && (
            <pre style={{ background: "var(--bg2)", padding: 10, marginTop: 8, fontSize: 11, color: "var(--cyan)", whiteSpace: "pre-wrap" }}>{whisper.install_hint}</pre>
          )}
        </div>
      )}
      <TmuxSessionsPanel panes={panes} focusedPane={focusedPane} />
    </div>
  );
}

// 053 — sesiones tmux Furx activas
function TmuxSessionsPanel({ panes, focusedPane }: Pick<Props, "panes" | "focusedPane">) {
  const [sessions, setSessions] = useState<FurxSession[]>([]);
  const [loading, setLoading] = useState(true);

  const load = async () => {
    setLoading(true);
    try {
      const s = await invoke<FurxSession[]>("tmux_list_furx_sessions");
      setSessions(s);
    } catch {
      setSessions([]);
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => { void load(); }, []);

  const attach = (name: string) => {
    const target = focusedPane ?? panes[0]?.id;
    if (!target) return;
    // 058 (ultrareview audit, defense-in-depth): los nombres de sesión Furx son SIEMPRE
    // `FURX_<[A-Za-z0-9_]>` (los sanitiza `furx_session_name` al crearlos), pero validamos acá igual
    // antes de interpolarlos en un comando de shell — sin chars raros no hay inyección posible.
    if (!/^FURX_[A-Za-z0-9_]+$/.test(name)) return;
    // 058 (ultrareview audit fix): `-L furx` — las sesiones Furx viven en el socket DEDICADO; sin esto
    // el attach pegaría al server tmux por defecto del usuario y NO encontraría la sesión (o adjuntaría
    // una homónima ajena). Coincide con `list_furx_sessions`/spawn, que ya usan `-L furx`.
    const cmd = `tmux -L furx attach-session -t ${name}\n`;
    invoke("pty_write", { paneId: target, data: cmd, actionId: null, correlationId: null }).catch(console.error);
  };

  return (
    <div style={{ marginTop: 24 }}>
      <div style={{ display: "flex", alignItems: "center", gap: 10, marginBottom: 10 }}>
        <strong style={{ fontSize: 13 }}>Sesiones tmux Furx</strong>
        <Button variant="ghost" onClick={load}>Refrescar</Button>
      </div>
      {loading ? (
        <div className="muted" style={{ fontSize: 12 }}>cargando…</div>
      ) : sessions.length === 0 ? (
        <div className="muted" style={{ fontSize: 12 }}>Sin sesiones tmux activas.</div>
      ) : (
        <div className="mon-grid">
          {sessions.map((s) => (
            <div
              key={s.name}
              className="mon"
              style={{ cursor: "pointer" }}
              onClick={() => attach(s.name)}
            >
              <div className="mon-head">
                <span className="dot up" />
                <span className="mon-label">{s.name}</span>
                {s.created && <span className="mon-addr muted">{s.created}</span>}
              </div>
              <div className="mon-body">
                <Button variant="ghost" onClick={(e) => { e.stopPropagation(); attach(s.name); }}>Conectar</Button>
              </div>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
