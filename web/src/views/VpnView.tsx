// VPN view — Tailscale + WireGuard live status.

import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Button } from "../components/Button";

interface TailscalePeer {
  hostname: string;
  tailscale_ip: string | null;
  online: boolean;
  os: string | null;
  last_seen: string | null;
}
interface TailscaleStatus {
  installed: boolean;
  running: boolean;
  backend_state: string | null;
  self_ip: string | null;
  self_hostname: string | null;
  peers: TailscalePeer[];
}
interface WireguardInterface {
  name: string;
  public_key: string | null;
  peers_count: number;
  up: boolean;
}
interface VpnStatus { tailscale: TailscaleStatus; wireguard: WireguardInterface[]; }

export function VpnView() {
  const [data, setData] = useState<VpnStatus | null>(null);
  const [busy, setBusy] = useState(false);
  const [msg, setMsg] = useState<string | null>(null);
  const refresh = async () => {
    try { setData(await invoke<VpnStatus>("vpn_status")); } catch (e) { setMsg(String(e)); }
  };
  useEffect(() => { refresh(); const id = setInterval(refresh, 30000); return () => clearInterval(id); }, []);
  const bringUp = async () => {
    setBusy(true); setMsg(null);
    try { const out = await invoke<string>("vpn_up", { name: "tailscale" }); setMsg(out || "tailscale up"); await refresh(); }
    catch (e) { setMsg(String(e)); } finally { setBusy(false); }
  };
  if (!data) return <div className="page"><div className="page-header"><div className="page-title">VPN</div></div><div className="muted" style={{ padding: 14 }}>cargando…</div></div>;
  return (
    <div className="page">
      <div className="page-header">
        <div className="page-title">VPN</div>
        <div className="page-sub">
          Tailscale {data.tailscale.installed ? (data.tailscale.running ? "✓" : `· ${data.tailscale.backend_state ?? "off"}`) : "no instalado"}
          {data.wireguard.length > 0 && ` · WireGuard ${data.wireguard.length} iface`}
        </div>
      </div>
      {msg && <div className="card-block info">{msg}</div>}
      <section className="settings-section">
        <h3 className="section-title">Tailscale</h3>
        {!data.tailscale.installed
          ? <div className="muted">No instalado. <code>brew install --cask tailscale</code></div>
          : <>
              <div className="row-meta" style={{ fontFamily: "var(--mono)", fontSize: 12 }}>
                state: <strong>{data.tailscale.backend_state ?? "?"}</strong>{" "}
                · self: <code>{data.tailscale.self_hostname ?? "?"}</code>{" "}
                {data.tailscale.self_ip && <>@ <code>{data.tailscale.self_ip}</code></>}
              </div>
              <div className="actions-row" style={{ marginTop: 8 }}>
                {!data.tailscale.running && (
                  <Button variant="primary" onClick={bringUp} disabled={busy}>
                    {busy ? "conectando…" : "Tailscale up"}
                  </Button>
                )}
                <button onClick={refresh} disabled={busy}>Refresh</button>
              </div>
              <div className="mon-grid" style={{ marginTop: 14 }}>
                {data.tailscale.peers.map((p) => (
                  <div key={p.hostname} className={`mon ${p.online ? "up" : "down"}`}>
                    <div className="mon-head">
                      <span className={`dot ${p.online ? "up" : "down"}`} />
                      <span className="mon-label">{p.hostname}</span>
                      <span className="mon-addr muted">{p.tailscale_ip ?? "?"}</span>
                    </div>
                    <div className="mon-body" style={{ fontSize: 12 }}>
                      {p.online ? "online" : <span className="muted">offline</span>}
                      {p.os && <span className="muted" style={{ marginLeft: 10 }}>{p.os}</span>}
                    </div>
                  </div>
                ))}
              </div>
            </>}
      </section>
      {data.wireguard.length > 0 && (
        <section className="settings-section">
          <h3 className="section-title">WireGuard</h3>
          <div className="mon-grid">
            {data.wireguard.map((w) => (
              <div key={w.name} className={`mon ${w.up ? "up" : "down"}`}>
                <div className="mon-head">
                  <span className={`dot ${w.up ? "up" : "down"}`} />
                  <span className="mon-label">{w.name}</span>
                  <span className="mon-addr muted">{w.peers_count} peers</span>
                </div>
                {w.public_key && <div className="muted" style={{ fontFamily: "var(--mono)", fontSize: 11 }}>{w.public_key.slice(0, 22)}…</div>}
              </div>
            ))}
          </div>
        </section>
      )}
    </div>
  );
}
