// 010-furx-signals — Settings → Integraciones (BYOK + control remoto).
// Telegram token + webhook → Keychain (NUNCA a DB/backend). Allowlist de chat_ids +
// pairing de un uso. Filtros por canal. Estética V3 (reusa las clases settings-* existentes).
import { useEffect, useState } from "react";
import { invoke } from "../lib/invoke"; // 015 T015: invoke con flujo de aprobación universal
import { Button } from "./Button";

type AllowEntry = { chat_id: string; label: string | null; paired_via: string; created_at: string };
type Delivery = {
  event_id: string;
  channel: string;
  status: string;
  attempts: number;
  last_error: string | null;
  type: string;
  severity: string;
};

const CHANNELS = ["desktop", "mobile", "telegram", "webhook"] as const;
const EVENT_TYPES = [
  "task.done",
  "task.failed",
  "task.awaiting_review",
  "agent.input_requested",
  "council.ready",
] as const;

export function SignalsPanel({
  get,
  setKey,
  setMsg,
}: {
  get: (key: string) => unknown;
  setKey: (key: string, value: unknown) => void;
  setMsg: (s: string | null) => void;
}) {
  const [allow, setAllow] = useState<AllowEntry[]>([]);
  const [pairCode, setPairCode] = useState<string | null>(null);
  const [deliveries, setDeliveries] = useState<Delivery[]>([]);
  const [tgToken, setTgToken] = useState("");
  const [webhookSecret, setWebhookSecret] = useState("");
  const [newChat, setNewChat] = useState("");
  const [newLabel, setNewLabel] = useState("");

  const refresh = async () => {
    try {
      const [a, d] = await Promise.all([
        invoke<AllowEntry[]>("signals_list_allowlist"),
        invoke<Delivery[]>("signals_recent_deliveries", { limit: 25 }),
      ]);
      setAllow(a);
      setDeliveries(d);
    } catch (e) {
      setMsg(`error: ${String(e)}`);
    }
  };
  useEffect(() => {
    refresh();
  }, []);

  const saveTgToken = async () => {
    if (!tgToken.trim()) return;
    try {
      await invoke("signals_set_telegram_secret", { secret: tgToken });
      setTgToken("");
      setMsg("Token de Telegram guardado en el Keychain ✓");
    } catch (e) {
      setMsg(`error: ${String(e)}`);
    }
  };

  const saveWebhookSecret = async () => {
    try {
      await invoke("signals_set_webhook_secret", { secret: webhookSecret });
      setWebhookSecret("");
      setMsg("Secreto de webhook guardado en el Keychain ✓");
    } catch (e) {
      setMsg(`error: ${String(e)}`);
    }
  };

  const genPair = async () => {
    try {
      const code = await invoke<string>("signals_create_pair_code");
      setPairCode(code);
      setMsg(null);
    } catch (e) {
      setMsg(`error: ${String(e)}`);
    }
  };

  const addChat = async () => {
    if (!newChat.trim()) return;
    try {
      await invoke("signals_add_allowlist", { chatId: newChat.trim(), label: newLabel.trim() || null });
      setNewChat("");
      setNewLabel("");
      await refresh();
    } catch (e) {
      setMsg(`error: ${String(e)}`);
    }
  };

  const removeChat = async (chatId: string) => {
    try {
      await invoke("signals_remove_allowlist", { chatId });
      await refresh();
    } catch (e) {
      setMsg(`error: ${String(e)}`);
    }
  };

  const subEnabled = (etype: string, channel: string): boolean => {
    // Convención de keys de settings para reflejar el toggle en UI (la verdad vive en
    // signal_subscriptions; acá mostramos el estado deseado y lo persistimos vía comando).
    const v = get(`signals.sub.${etype}.${channel}`);
    // Audit codex LOW: webhook está OFF por default en el backend → reflejarlo (no mostrar
    // un estado falso). El resto de canales: ON salvo que se desactive explícitamente.
    return channel === "webhook" ? v === true : v !== false;
  };

  const toggleSub = async (etype: string, channel: string, enabled: boolean) => {
    try {
      await invoke("signals_set_subscription", {
        eventType: etype,
        channel,
        enabled,
        minSeverity: "info",
      });
      setKey(`signals.sub.${etype}.${channel}`, enabled);
    } catch (e) {
      setMsg(`error: ${String(e)}`);
    }
  };

  return (
    <>
      <h4 style={{ marginTop: 4, marginBottom: 8, fontSize: 13, color: "var(--text)" }}>Telegram (BYOK)</h4>
      <div className="muted" style={{ marginBottom: 10, fontSize: 12 }}>
        El bot token / HMAC va al Keychain de macOS, <strong>nunca</strong> a la DB ni al backend.
        El relay POSTea a <code>127.0.0.1:43117/furx/v1/command</code> firmado (HMAC + nonce).
      </div>
      <div className="form-row">
        <label>Bot token / HMAC secret</label>
        <div className="form-input">
          <input
            type="password"
            value={tgToken}
            onChange={(e) => setTgToken(e.target.value)}
            placeholder="se guarda en el Keychain"
          />
          <button onClick={saveTgToken} disabled={!tgToken.trim()}>
            Guardar
          </button>
        </div>
      </div>

      <h4 style={{ marginTop: 20, marginBottom: 8, fontSize: 13, color: "var(--text)" }}>Control remoto</h4>
      <div className="muted" style={{ marginBottom: 10, fontSize: 12 }}>
        Sólo los chat_ids de la allowlist pueden mandar comandos: <code>/status</code>,{" "}
        <code>/cancel &lt;task&gt;</code>, <code>/reply &lt;task&gt; &lt;texto&gt;</code>,{" "}
        <code>/ready &lt;task&gt;</code>. Nada de shell ni acciones destructivas.
      </div>
      <div className="actions-row" style={{ marginBottom: 10 }}>
        <button onClick={genPair}>Generar código de pairing</button>
        {pairCode && (
          <span className="row-meta">
            Mandá <code>/pair {pairCode}</code> desde tu Telegram (válido 10 min, un solo uso).
          </span>
        )}
      </div>

      <div className="form-row">
        <label>Agregar chat_id manualmente</label>
        <div className="form-input">
          <input value={newChat} onChange={(e) => setNewChat(e.target.value)} placeholder="chat_id" />
          <input
            value={newLabel}
            onChange={(e) => setNewLabel(e.target.value)}
            placeholder="etiqueta (opcional)"
            style={{ marginLeft: 6 }}
          />
          <button onClick={addChat} disabled={!newChat.trim()}>
            Agregar
          </button>
        </div>
      </div>

      {allow.length > 0 && (
        <div style={{ marginTop: 8 }}>
          {allow.map((a) => (
            <div key={a.chat_id} className="row-meta" style={{ display: "flex", gap: 8, alignItems: "center" }}>
              <code>{a.chat_id}</code>
              {a.label && <span className="muted">{a.label}</span>}
              <span className="muted">· {a.paired_via}</span>
              <Button variant="danger" onClick={() => removeChat(a.chat_id)} style={{ marginLeft: "auto" }}>
                Quitar
              </Button>
            </div>
          ))}
        </div>
      )}

      <h4 style={{ marginTop: 20, marginBottom: 8, fontSize: 13, color: "var(--text)" }}>Filtros por canal</h4>
      <div className="muted" style={{ marginBottom: 10, fontSize: 12 }}>
        Elegí qué eventos van a qué canal (evita la fatiga de notificaciones).
      </div>
      <div style={{ overflowX: "auto" }}>
        <table style={{ width: "100%", fontSize: 12, borderCollapse: "collapse" }}>
          <thead>
            <tr>
              <th style={{ textAlign: "left", padding: "4px 8px" }}>Evento</th>
              {CHANNELS.map((c) => (
                <th key={c} style={{ padding: "4px 8px" }}>
                  {c}
                </th>
              ))}
            </tr>
          </thead>
          <tbody>
            {EVENT_TYPES.map((et) => (
              <tr key={et}>
                <td style={{ padding: "4px 8px", fontFamily: "var(--mono)" }}>{et}</td>
                {CHANNELS.map((c) => (
                  <td key={c} style={{ textAlign: "center", padding: "4px 8px" }}>
                    <input
                      type="checkbox"
                      checked={subEnabled(et, c)}
                      onChange={(e) => toggleSub(et, c, e.target.checked)}
                    />
                  </td>
                ))}
              </tr>
            ))}
          </tbody>
        </table>
      </div>

      <h4 style={{ marginTop: 20, marginBottom: 8, fontSize: 13, color: "var(--text)" }}>Webhook genérico (BYOK)</h4>
      <div className="form-row">
        <label>URL del webhook</label>
        <div className="form-input">
          <input
            value={(get("signals.webhook_url") as string) ?? ""}
            onChange={(e) => setKey("signals.webhook_url", e.target.value)}
            placeholder="https://… (debe estar en la allowlist)"
          />
        </div>
      </div>
      <div className="form-row">
        <label>HMAC secret del webhook</label>
        <div className="form-input">
          <input
            type="password"
            value={webhookSecret}
            onChange={(e) => setWebhookSecret(e.target.value)}
            placeholder="se guarda en el Keychain (opcional)"
          />
          <button onClick={saveWebhookSecret}>Guardar</button>
        </div>
      </div>

      <h4 style={{ marginTop: 20, marginBottom: 8, fontSize: 13, color: "var(--text)" }}>Entregas recientes</h4>
      <div className="actions-row" style={{ marginBottom: 8 }}>
        <button onClick={refresh}>Refrescar</button>
      </div>
      {deliveries.length === 0 ? (
        <div className="muted">Sin entregas todavía.</div>
      ) : (
        <div style={{ fontSize: 11, fontFamily: "var(--mono)" }}>
          {deliveries.map((d, i) => (
            <div key={`${d.event_id}-${d.channel}-${i}`} className="row-meta">
              <code>{d.type}</code> · {d.channel} ·{" "}
              <strong className={d.status === "sent" ? "" : "muted"}>{d.status}</strong>
              {d.attempts > 0 && <span className="muted"> ({d.attempts}x)</span>}
              {d.last_error && <span className="muted"> · {d.last_error}</span>}
            </div>
          ))}
        </div>
      )}
    </>
  );
}
