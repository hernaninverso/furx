// web/src/lib/telemetry.ts — 016 US5 (T050 + T072 + T074) · telemetry opt-in privacy-by-default.
//
// Constitución F-IV (privacy by default) + F-VII (sink = Worker Cloudflare). BYOK intacto (F-I): la
// telemetry NUNCA lleva API keys, prompts, ni PII — y NUNCA viaja por la ruta LLM.
//
// Diseño (council T072):
//   - CATÁLOGO TIPADO POR EVENTO (schema cerrado): `trackEvent<K>(name, props)` exige EXACTAMENTE las
//     props de `TelemetryCatalog[K]`. Props fuera del schema son IMPOSIBLES por tipo (no compilan).
//   - Defensa-en-profundidad: además del tipo, un filtro allowlist por evento en runtime DESCARTA el
//     evento entero si llega un campo no permitido (no sanitiza — council T073 lo replica en el Worker).
//   - Lista negra por capa: claves prohibidas (apiKey/token/prompt/...) y valores que parezcan secreto
//     (sk-..., Bearer, paths) → DROP del evento. T072/T074.
//   - Gate: emite SÓLO si `opt_in.telemetry === true` Y `endpoints.telemetry` no vacío. Default OFF.
//   - Buffer FIFO tope 50; fire-and-forget (no reintentos agresivos); no bloquea la UI. FR-020.
//   - Sin install-id en v1 (council). El payload no lleva identificador de usuario.

// Lectura de settings: el invoke CRUDO de Tauri (NO el envuelto con gate de aprobación). `settings_all`
// es Safe y no debe pasar por el flujo de aprobación; además evita arrastrar la cadena capability/
// approvalBus al grafo de los tests node. Telemetry NO necesita el gate (no ejecuta comandos).
import { invoke as rawInvoke } from "@tauri-apps/api/core";

/* ── Catálogo de eventos (schema cerrado). Sólo categorías/flags/enums — NUNCA contenido. ──────── */
export interface TelemetryCatalog {
  /** Help abierto. `source` = de dónde (palette|topbar|deeplink). */
  help_opened: { source: "palette" | "topbar" | "deeplink" };
  /** Tour completado (sin props — sólo el hecho). */
  tour_completed: Record<string, never>;
  /** Tour saltado, con el índice del paso (número, no contenido). */
  tour_skipped: { step: number };
  /** Comando ejecutado. SÓLO la categoría del comando (nunca el id ni los args). */
  command_executed: { category: string };
  /** Idioma cambiado. SÓLO el código de idioma destino. */
  language_changed: { to: string };
  /** What's New abierto. */
  whatsnew_opened: Record<string, never>;
}

export type TelemetryEventName = keyof TelemetryCatalog;

/// Allowlist EXPLÍCITA de nombres de prop por evento (segundo filtro, defensa-en-profundidad). Si un
/// evento trae una key fuera de esta lista → DROP del evento entero (no se sanitiza). T072.
const ALLOWED_PROPS: Record<TelemetryEventName, readonly string[]> = {
  help_opened: ["source"],
  tour_completed: [],
  tour_skipped: ["step"],
  command_executed: ["category"],
  language_changed: ["to"],
  whatsnew_opened: [],
};

/// M2 (audit): validadores de VALOR por campo — enums cerrados / patrones acotados. Más allá del
/// allowlist de KEYS: aunque un cliente alterado castee el tipo, un valor categórico fuera del enum
/// se DROPEA (cliente Y Worker espejan esto). Las categorías de comando se limitan a un slug corto.
const SAFE_CATEGORY = /^[a-z0-9_-]{1,32}$/;
const VALUE_OK: { [K in TelemetryEventName]?: Record<string, (v: unknown) => boolean> } = {
  help_opened: { source: (v) => v === "palette" || v === "topbar" || v === "deeplink" },
  tour_skipped: { step: (v) => typeof v === "number" && Number.isInteger(v) && v >= 0 && v <= 100 },
  command_executed: { category: (v) => typeof v === "string" && SAFE_CATEGORY.test(v) },
  language_changed: { to: (v) => v === "es" || v === "en" },
};

/// Lista NEGRA de nombres de prop: jamás deben aparecer (aunque el tipo lo impidiera, runtime guard).
const FORBIDDEN_KEYS = [
  "apikey", "api_key", "key", "keys", "token", "secret", "password", "pass",
  "authorization", "bearer", "prompt", "promptcontent", "content", "message",
  "messages", "args", "argv", "command", "cmd", "path", "filepath", "filename",
  "file", "url", "href", "cwd", "home", "email", "user", "username", "id", "installid",
];

/// Heurística de "valor parece secreto/PII": sk-/pk-, Bearer, JWT, rutas absolutas, emails, URLs.
const SECRETISH_VALUE = /\b(sk|pk|rk)-[a-z0-9]/i;
const BEARER_VALUE = /bearer\s+/i;
const PATH_VALUE = /^(\/|~\/|[a-z]:\\)/i;
const EMAIL_VALUE = /[^\s@]+@[^\s@]+\.[^\s@]+/;
const URL_VALUE = /https?:\/\//i;
const JWT_VALUE = /^eyJ[a-z0-9_-]+\.[a-z0-9_-]+\./i;

export interface TelemetryEvent {
  event: TelemetryEventName;
  props: Record<string, string | number | boolean>;
  ts: number;
}

/* ── Buffer FIFO acotado ─────────────────────────────────────────────────────────────────────── */
const BUFFER_CAP = 50;
const buffer: TelemetryEvent[] = [];

/// Sólo para tests: limpia el buffer y la config cacheada.
export function __resetTelemetryForTest(): void {
  buffer.length = 0;
  _config = null;
  _sent.length = 0;
}

/* ── Config (opt-in + endpoint), cacheada con TTL corto, leída de settings (Tauri) ─────────────── */
interface TelemetryConfig {
  enabled: boolean;
  endpoint: string;
}
let _config: { value: TelemetryConfig; at: number } | null = null;
const CONFIG_TTL_MS = 5000;

/// Refresca la config desde settings (`settings_all`). El caller (Settings/Shell) la puede invalidar.
export async function refreshTelemetryConfig(): Promise<TelemetryConfig> {
  try {
    const all = await rawInvoke<Array<[string, unknown]>>("settings_all");
    const map = new Map(all);
    const enabled = map.get("opt_in.telemetry") === true;
    const endpoint = typeof map.get("endpoints.telemetry") === "string" ? (map.get("endpoints.telemetry") as string) : "";
    const value: TelemetryConfig = { enabled, endpoint };
    _config = { value, at: Date.now() };
    return value;
  } catch {
    const value: TelemetryConfig = { enabled: false, endpoint: "" };
    _config = { value, at: Date.now() };
    return value;
  }
}

export function invalidateTelemetryConfig(): void {
  _config = null;
}

/// Sólo para tests: fija la config (opt-in + endpoint) sin pegarle a Tauri.
export function __setConfigForTest(enabled: boolean, endpoint: string): void {
  _config = { value: { enabled, endpoint }, at: Date.now() };
}

/* ── Validación PII / allowlist (PURA, testeable) ────────────────────────────────────────────── */

/// True si `props` es seguro para emitir bajo el schema de `event`. DROP del evento si:
///   - hay una key fuera de la allowlist del evento; o
///   - hay una key en la lista negra; o
///   - algún valor parece secreto/PII; o
///   - algún valor no es primitivo (string/number/boolean) — defensa contra objetos anidados.
export function isPropsSafe(event: TelemetryEventName, props: Record<string, unknown>): boolean {
  const allowed = ALLOWED_PROPS[event];
  for (const [k, v] of Object.entries(props)) {
    const key = k.toLowerCase();
    if (!allowed.includes(k)) return false;          // fuera del schema → DROP
    if (FORBIDDEN_KEYS.includes(key)) return false;  // key prohibida → DROP
    if (typeof v !== "string" && typeof v !== "number" && typeof v !== "boolean") return false;
    if (typeof v === "string" && looksSecret(v)) return false; // valor sospechoso → DROP
    const validator = VALUE_OK[event]?.[k];          // M2: valor fuera del enum cerrado → DROP
    if (validator && !validator(v)) return false;
  }
  return true;
}

export function looksSecret(value: string): boolean {
  return (
    SECRETISH_VALUE.test(value) ||
    BEARER_VALUE.test(value) ||
    PATH_VALUE.test(value) ||
    EMAIL_VALUE.test(value) ||
    URL_VALUE.test(value) ||
    JWT_VALUE.test(value)
  );
}

/* ── trackEvent (TIPADO por evento) ──────────────────────────────────────────────────────────── */

/// Para tests: registro de lo que SÍ se "envió" (cuando hay sink fake). En prod es fire-and-forget.
const _sent: TelemetryEvent[] = [];
export function __sentForTest(): TelemetryEvent[] {
  return _sent;
}

/// Sink inyectable (default = fetch al Worker). Tests inyectan un fake para inspeccionar el payload.
type Sink = (endpoint: string, ev: TelemetryEvent) => void;
let _sink: Sink | null = null;
export function __setSinkForTest(sink: Sink | null): void {
  _sink = sink;
}

/**
 * Emite un evento de telemetry. Props TIPADAS por `TelemetryCatalog[K]` (fuera del schema = no
 * compila). Runtime: gate opt-in+endpoint, allowlist/anti-PII (DROP si falla), buffer FIFO acotado,
 * fire-and-forget. NUNCA lanza (no debe romper la UI). FR-018/019/020/021/022.
 *
 * Async sólo internamente (lee config cacheada); el caller la llama sin await (fire-and-forget).
 */
export function trackEvent<K extends TelemetryEventName>(name: K, props: TelemetryCatalog[K]): void {
  void emit(name, props as Record<string, unknown>);
}

/// Sólo para tests: variante awaitable de `trackEvent` para aserciones determinísticas.
export async function __trackForTest(name: TelemetryEventName, props: Record<string, unknown>): Promise<void> {
  await emit(name, props);
}

async function emit(name: TelemetryEventName, props: Record<string, unknown>): Promise<void> {
  try {
    // 1) Anti-PII / allowlist ANTES de cualquier red (defensa-en-profundidad).
    if (!isPropsSafe(name, props)) return; // DROP silencioso del evento entero
    // 2) Gate: opt-in + endpoint (config cacheada con TTL).
    const cfg = _config && Date.now() - _config.at < CONFIG_TTL_MS ? _config.value : await refreshTelemetryConfig();
    if (!cfg.enabled || !cfg.endpoint) return; // OFF o sin endpoint → 0 red
    // 3) Buffer FIFO acotado.
    const ev: TelemetryEvent = { event: name, props: props as Record<string, string | number | boolean>, ts: Date.now() };
    buffer.push(ev);
    while (buffer.length > BUFFER_CAP) buffer.shift();
    // 4) Fire-and-forget al sink (default = fetch al Worker). Sin reintentos agresivos.
    flush(cfg.endpoint);
  } catch {
    /* telemetry NUNCA rompe la UI */
  }
}

function flush(endpoint: string): void {
  const sink: Sink = _sink ?? defaultSink;
  while (buffer.length > 0) {
    const ev = buffer.shift()!;
    try { sink(endpoint, ev); _sent.push(ev); } catch { /* descartar */ }
  }
}

function defaultSink(endpoint: string, ev: TelemetryEvent): void {
  if (typeof fetch === "undefined") return;
  // body mínimo: {event, props, ts, v}. v = versión de schema (no la versión de la app).
  const body = JSON.stringify({ event: ev.event, props: ev.props, ts: ev.ts, v: 1 });
  void fetch(endpoint.replace(/\/+$/, "") + "/t", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body,
    keepalive: true,
  }).catch(() => { /* fire-and-forget: un fallo de red no reintenta ni bloquea */ });
}
