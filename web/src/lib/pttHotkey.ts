// web/src/lib/pttHotkey.ts — 059 · push-to-talk hotkey CONFIGURABLE.
//
// Formato persistido en settings (`ptt.hotkey`): "<Mod>+...+<Code>" con modificadores canónicos
// Alt/Control/Meta/Shift y, como último token, el `KeyboardEvent.code` de la tecla base
// (ej "Alt+Space", "Control+KeyT", "Meta+Shift+KeyV"). Default ⌥Space (Alt+Space).
//
// Diseño puro/testeable: no toca el DOM ni settings; el Shell parsea y matchea, Ajustes graba con
// `eventToHotkeyString`. Mantiene la lógica delicada de held-key del PTT intacta (settle idempotente).

export const DEFAULT_PTT_HOTKEY = "Alt+Space";

const MOD_TOKENS = ["Alt", "Control", "Meta", "Shift"] as const;
type ModToken = (typeof MOD_TOKENS)[number];

export interface ParsedHotkey {
  altKey: boolean;
  ctrlKey: boolean;
  metaKey: boolean;
  shiftKey: boolean;
  code: string;
}

// Único mapa token-canónico → flag booleano. Parse, release-detection, serialización y display TODOS
// derivan de acá + `MOD_TOKENS` (un solo orden canónico): agregar/renombrar un modificador es 1 edición,
// no 4 listas paralelas que pueden divergir (string persistido vs set de release vs glifo del label).
type ModFlag = "altKey" | "ctrlKey" | "metaKey" | "shiftKey";
const MOD_FLAG: Record<ModToken, ModFlag> = { Alt: "altKey", Control: "ctrlKey", Meta: "metaKey", Shift: "shiftKey" };

/// Modificadores presentes en `src` (ParsedHotkey o un KeyboardEvent), en orden canónico de `MOD_TOKENS`.
function modTokens(src: Record<ModFlag, boolean>): ModToken[] {
  return MOD_TOKENS.filter((m) => src[MOD_FLAG[m]]);
}

/// Parsea el string persistido. Si es inválido (vacío o SÓLO modificadores, sin tecla base) → default
/// (un PTT necesita una tecla base para soltar; sólo-modificador rompería el gesto held-key).
export function parsePttHotkey(s: string | null | undefined): ParsedHotkey {
  const raw = typeof s === "string" && s.trim() ? s.trim() : DEFAULT_PTT_HOTKEY;
  const parts = raw.split("+").map((p) => p.trim()).filter(Boolean);
  const h: ParsedHotkey = { altKey: false, ctrlKey: false, metaKey: false, shiftKey: false, code: "" };
  for (const p of parts) {
    const mod = MOD_TOKENS.find((m) => m === p);
    if (mod) h[MOD_FLAG[mod]] = true;
    else h.code = p;
  }
  if (!h.code) return parsePttHotkey(DEFAULT_PTT_HOTKEY);
  return h;
}

type EventLike = Pick<KeyboardEvent, "altKey" | "ctrlKey" | "metaKey" | "shiftKey" | "code">;

/// ¿el evento dispara el hotkey? Match EXACTO: el `code` coincide Y el set de modificadores es
/// idéntico (ni falta ni sobra ninguno). El match ocurre SÓLO al iniciar el gesto (keydown con
/// `!st.active`), no durante el hold, así que exigir exactitud no rompe el held-key y SÍ evita
/// colisiones (audit codex/deepseek): sin esto un hotkey "KeyT" dispararía con ⌘T, o "Alt+Space" con
/// ⌃⌥Space. El release se maneja aparte por `code`/modificadores en el keyup (settle idempotente).
export function matchesPttHotkey(e: EventLike, h: ParsedHotkey): boolean {
  return (
    e.code === h.code &&
    e.altKey === h.altKey &&
    e.ctrlKey === h.ctrlKey &&
    e.metaKey === h.metaKey &&
    e.shiftKey === h.shiftKey
  );
}

/// Nombres de `KeyboardEvent.key` de los modificadores configurados — para detectar el RELEASE del
/// gesto (el keyup de cualquiera de ellos, o de la tecla base, settlea; el settle es idempotente).
export function pttModifierKeyNames(h: ParsedHotkey): string[] {
  return modTokens(h);
}

/// Construye el string persistible desde un keydown (input de grabación en Ajustes). Devuelve null si
/// la tecla presionada ES un modificador (todavía no hay tecla base) → el input sigue esperando.
export function eventToHotkeyString(
  e: Pick<KeyboardEvent, "altKey" | "ctrlKey" | "metaKey" | "shiftKey" | "code" | "key">,
): string | null {
  if ((MOD_TOKENS as readonly string[]).includes(e.key)) return null;
  if (!e.code) return null;
  return [...modTokens(e), e.code].join("+");
}

// 059 — flag global de "captura": mientras Ajustes graba un hotkey nuevo, el handler global de PTT del
// Shell se silencia (sino presionar el combo dispararía una grabación de voz en vez de capturarse).
// `window` es el único punto compartido; los guards lo hacen seguro en Node (tests, sin window).
interface CaptureWindow { __furxPttCapture?: boolean }
export function setPttCapturing(on: boolean): void {
  if (typeof window !== "undefined") (window as unknown as CaptureWindow).__furxPttCapture = on;
}
export function isPttCapturing(): boolean {
  return typeof window !== "undefined" && Boolean((window as unknown as CaptureWindow).__furxPttCapture);
}

const SYM: Record<ModToken, string> = { Alt: "⌥", Control: "⌃", Meta: "⌘", Shift: "⇧" };

/// Etiqueta legible (⌥⌘⌃⇧ + tecla). "Space" tal cual; "KeyT"→"T"; "Digit1"→"1"; resto crudo.
export function formatPttHotkey(h: ParsedHotkey): string {
  const mods = modTokens(h).map((m) => SYM[m]);
  let key = h.code;
  if (h.code.startsWith("Key")) key = h.code.slice(3);
  else if (h.code.startsWith("Digit")) key = h.code.slice(5);
  return mods.join("") + key;
}
