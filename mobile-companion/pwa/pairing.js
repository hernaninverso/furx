// mobile-companion/pwa/pairing.js — 065 · helpers PUROS de pairing (council v4 + audit-3).
//
// El companion escanea el QR del desktop (o tipea el short-code), conecta al bridge SIN secreto
// (pre-Hello), manda pairing_redeem{token}, y recibe pairing_grant{secret}. El CANJE por WS lo hace el
// wiring (index.html) con el transporte unificado — browser `WebSocket` o el IPC nativo `ws_connect`
// (audit codex HIGH: WKWebView de iOS bloquea `new WebSocket` directo; la app nativa usa el transporte
// Rust). Este módulo NO toca red para el WS: solo parseo, validación y construcción de candidatos.

const SHORT_ALPHA_RE = /^[2-9A-HJ-NP-Z]{8}$/; // Base32 sin 0/1/I/O ambiguos
const LAN_PORT = 43118;
const TS_PORT = 43119;

export function isValidPairingHost(ip) {
  const parts = String(ip).split(".").map(Number);
  if (parts.length !== 4 || parts.some((n) => Number.isNaN(n) || n < 0 || n > 255)) return false;
  const [a, b] = parts;
  return (
    (a === 100 && b >= 64 && b <= 127) || // Tailscale CGNAT 100.64.0.0/10
    a === 10 ||
    (a === 172 && b >= 16 && b <= 31) ||
    (a === 192 && b === 168)
  );
  // NO 127.x.x.x: el móvil no alcanza el loopback del desktop.
}

/// Parsea `furx://pair?v=1&t=…&h=…&p=…&exp=…&n=…&ts=…`. Devuelve {token,hosts,tsIp,port,exp,name} o {error}.
export function parsePairingUri(uri) {
  let url;
  try {
    url = new URL(uri);
  } catch {
    return { error: "uri_invalid" };
  }
  if (url.protocol !== "furx:" || url.hostname !== "pair") return { error: "uri_invalid" };
  const p = url.searchParams;
  if (p.get("v") !== "1") return { error: `unsupported_version:${p.get("v")}` };
  const t = p.get("t");
  if (!t || t.length !== 64 || !/^[0-9a-f]+$/i.test(t)) return { error: "token_invalid" };
  // exp DEBE ser entero positivo y dentro de la gracia (audit codex): parseo ESTRICTO (solo dígitos —
  // `parseInt` aceptaba `9999999999junk`); un exp ausente/0/inválido NO se acepta.
  const expRaw = p.get("exp") || "";
  const exp = /^\d+$/.test(expRaw) ? Number(expRaw) : NaN;
  const now = Math.floor(Date.now() / 1000);
  if (!Number.isInteger(exp) || exp <= 0 || now > exp + 45) return { error: "token_expired" };
  // Puerto LAN: SOLO 43118 (el de Tailscale es 43119 fijo, no viaja en `p`). Parseo estricto (rechaza
  // `43118junk`). Ausente → default LAN_PORT (nuestro generador siempre lo emite).
  const portRaw = p.get("p");
  const port = portRaw === null ? LAN_PORT : /^\d+$/.test(portRaw) ? Number(portRaw) : NaN;
  if (port !== LAN_PORT) return { error: "port_invalid" };
  const hosts = (p.get("h") || "").split(",").filter((ip) => ip && isValidPairingHost(ip));
  const tsRaw = p.get("ts");
  const tsIp = tsRaw && isValidPairingHost(tsRaw) ? tsRaw : null;
  // URLSearchParams.get() YA decodifica el %-encoding → NO re-decodificar (un `decodeURIComponent` extra
  // tiraba "URI malformed" si el nombre decodificado contenía un `%`). Audit codex.
  const name = p.get("n") || "Desktop";
  return { token: t, hosts, tsIp, port, exp, name };
}

/// URLs WS candidatas (LAN en 43118 + Tailscale en 43119). El wiring intenta en orden.
export function buildCandidateUrls(payload) {
  return [
    ...payload.hosts.map((ip) => `ws://${ip}:${payload.port || LAN_PORT}/ws`),
    ...(payload.tsIp ? [`ws://${payload.tsIp}:${TS_PORT}/ws`] : []),
  ];
}

/// device_id estable (UUID, primer lanzamiento) — para el log de auditoría del desktop y el bind del
/// retry idempotente (el desktop solo reenvía el grant al MISMO device_id).
export function getDeviceId() {
  let id = localStorage.getItem("furx.device-id");
  if (!id) {
    id = (crypto.randomUUID && crypto.randomUUID()) || `dev-${Date.now()}-${Math.random().toString(16).slice(2)}`;
    localStorage.setItem("furx.device-id", id);
  }
  return id;
}

export function getDeviceName() {
  const ua = navigator.userAgent || "";
  if (ua.includes("iPhone")) return "iPhone";
  if (ua.includes("iPad")) return "iPad";
  if (ua.includes("Android")) return "Android";
  return "Mobile";
}

/// Frame de canje (pre-Hello, sin HMAC).
export function redeemFrame(token) {
  return { type: "pairing_redeem", token, device_id: getDeviceId(), device_name: getDeviceName() };
}

/// Resuelve el token efímero desde un short-code de 8 chars vía POST /pair-shortcode. Prueba SOLO los
/// hosts provistos (del campo host o el origin del PWA), en el puerto LAN. Devuelve {token, host}.
export async function fetchShortCodeToken(shortCode, hosts) {
  const code = String(shortCode).trim().toUpperCase();
  if (!SHORT_ALPHA_RE.test(code)) throw new Error("short_code_invalid");
  const validHosts = (hosts || []).filter(isValidPairingHost);
  if (validHosts.length === 0) throw new Error("no_valid_host");
  // El bridge sirve /pair-shortcode en :43119 (Tailscale, el bind remoto real) o :43118 (LAN). Probar
  // ambos por host (audit codex: hardcodear :43118 rompía el fallback remoto por Tailscale). Devuelve el
  // puerto que respondió para que el canje WS use el MISMO.
  const PORTS = [TS_PORT, LAN_PORT];
  for (const ip of validHosts) {
    for (const prt of PORTS) {
      // Timeout manual con AbortController (audit deepseek B1): `AbortSignal.timeout` no existe en
      // browsers viejos → sin él el fetch podría colgarse indefinidamente.
      const ctrl = new AbortController();
      const to = setTimeout(() => ctrl.abort(), 3000);
      try {
        const resp = await fetch(`http://${ip}:${prt}/pair-shortcode`, {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify({ code }),
          signal: ctrl.signal,
        });
        if (resp.ok) return { token: (await resp.json()).token, host: ip, port: prt };
      } catch {
        continue;
      } finally {
        clearTimeout(to);
      }
    }
  }
  throw new Error("short_code_not_found");
}
