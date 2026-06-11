// web/src/lib/sentenceCase.ts — 022 P0c · US5/FR-009 · convención sentence-case del catálogo i18n.
//
// Regla (enforced por test, no por inspección manual): TODO valor del catálogo base (`es.ts`) debe
// estar en **sentence case** — la primera palabra con mayúscula inicial, el resto en minúscula salvo
// nombres propios / siglas de la allowlist. NO Title-Case ("Incidentes Abiertos" ✗).
//
// Diseño de la verificación (pura, testeable sin React / Tauri):
//   - Se tokeniza por espacios; se consideran sólo los "tokens alfabéticos" (con al menos una letra).
//   - PRIMER token: debe empezar en mayúscula. EXCEPCIÓN: si el valor empieza con un placeholder
//     `{x}`, un dígito, o un símbolo/emoji (no letra), la "primera palabra" es dinámica/no-prosa →
//     no se exige mayúscula inicial (ej. "{count} entradas", "24h").
//   - Tokens siguientes: NO pueden empezar en mayúscula, SALVO que (a) estén en la allowlist de
//     nombres propios/siglas (PROPER_NOUNS ∪ KNOWN_ACRONYMS), o (b) el token anterior cierre una
//     oración (`.`/`?`/`!`/`:`) → es el comienzo de una oración nueva y la mayúscula es legítima.
//   - Una sigla all-caps (PR, AIE, MCP…) DEBE estar en la allowlist para ser válida en cualquier
//     posición. Un token all-caps ARBITRARIO **no** allowlistado se trata como VIOLACIÓN de
//     sentence-case (ej. "ABIERTOS" en "Incidentes ABIERTOS" falla) — antes pasaba por la regla laxa
//     "cualquier all-caps de 2+ letras es sigla", que dejaba colar Title-Case en mayúsculas.

/** Nombres propios y siglas permitidos en mayúscula/Title-Case en cualquier posición. */
export const PROPER_NOUNS: readonly string[] = [
  // marca / producto
  "Furx",
  // agentes / modelos
  "Claude", "Codex", "Gemini", "Cursor", "Aider", "Grok",
  // siglas / tecnologías
  "AIE", "MCP", "PR", "PRs", "TG", "BYOK", "SSH", "VPN", "GitHub", "SaaS",
  "CLI", "API", "URL", "UI", "ID", "OK", "TTS", "PTT", "LLM", "IA", "AI",
  "Ed25519", "WASM", "QR", "HMAC",
  // ── brand wave 4 (2026-06-09): catálogo wizard/connect/accounts/empty ──
  // proveedores / productos / modelos (nombres propios reales)
  "Anthropic", "OpenAI", "OpenRouter", "Cerebras", "Groq", "Mistral", "SambaNova",
  "Google", "Ollama", "LiteLLM", "Tailscale", "SQLite", "Keychain", "Terminal",
  "Llama", "Qwen", "DeepSeek", "GPT", "Opus", "Sonnet", "Haiku", "Flash", "Studio",
  // tokens compuestos tal como quedan tras el strip de puntuación del checker
  "Apache", "Apache-20", "Council", "Council/eval", "Mode", "Engine", "Experiment",
  "Quick", "Start", "Connect", "Settings", "Ajustes", "Setup", "Verify", "Pro",
  "License", "NOTICE", "DB", "LM", "SMS", "MTok", "APIs", "AIza", "FURX_*", "⌘N",
  "GPT-4o", "Qwen-3-235B", "Llama-33-70B", "Llama-405B", "DeepSeek-V31",
  "Google/GitHub", "Cloud", "OpenAI-compatible",
  // labels de botones citados «…» dentro de los instructivos (referencias a UI, no prosa)
  "Abrir", "Conectar", "Create", "Key", "Keys", "Code", "Servicios", "SIGSTOP",
] as const;

/**
 * Siglas técnicas conocidas (all-caps) admitidas en cualquier posición. Es un SET EXPLÍCITO: sólo
 * estas (∪ PROPER_NOUNS) cuentan como sigla. Un all-caps arbitrario NO listado falla la convención.
 */
export const KNOWN_ACRONYMS: readonly string[] = [
  "HTTP", "HTTPS", "JSON", "YAML", "HTML", "CSS", "SQL", "URL", "URI", "DNS",
  "TLS", "SSL", "JWT", "OAuth", "RBAC", "CRUD", "REST", "GRPC", "IDE", "OS",
  "RAM", "CPU", "GPU", "PDF", "CSV", "XML", "UUID", "SDK", "CDN", "VM", "USD",
] as const;

const PROPER_NOUN_SET = new Set(PROPER_NOUNS);
const KNOWN_ACRONYM_SET = new Set(KNOWN_ACRONYMS);

/**
 * ¿`tok` (ya stripeado de puntuación de borde) es un nombre propio/sigla PERMITIDO?
 * SÓLO si está en la allowlist (PROPER_NOUNS ∪ KNOWN_ACRONYMS). Un all-caps arbitrario NO listado
 * devuelve false (→ se reporta como violación) — endurecido por audit MED 3.
 */
function isProperNoun(tok: string): boolean {
  return PROPER_NOUN_SET.has(tok) || KNOWN_ACRONYM_SET.has(tok);
}

const UPPER_RE = /[A-ZÁÉÍÓÚÑ]/;
const LETTER_RE = /[A-Za-zÁÉÍÓÚÑáéíóúñ]/;

/** Primer carácter alfabético de un token (saltando puntuación/símbolos de borde). `null` si no hay. */
function firstLetter(tok: string): string | null {
  for (const ch of tok) if (LETTER_RE.test(ch)) return ch;
  return null;
}

/** ¿El token termina cerrando una oración? (sufijo `.`/`?`/`!`/`:`, ignorando comillas/paréntesis). */
function endsSentence(tok: string): boolean {
  return /[.?!:]["'»)\]]?$/.test(tok);
}

/** ¿El primer carácter NO-espacio del valor es una letra? (si no, el arranque es dinámico/no-prosa). */
function startsWithLiteralLetter(value: string): boolean {
  const trimmed = value.replace(/^\s+/, "");
  // Empieza con placeholder `{x}` → dinámico.
  if (trimmed.startsWith("{")) return false;
  const ch = trimmed[0];
  return ch !== undefined && LETTER_RE.test(ch);
}

/** Resultado de validar un valor: ok + la lista de tokens que violan la convención. */
export interface SentenceCaseResult {
  ok: boolean;
  /** tokens offending (con su motivo) para el reporte del test. */
  offenders: { token: string; reason: "first-not-upper" | "mid-title-case" }[];
}

/**
 * Valida que `value` respete sentence-case. PURA. `offenders` vacío ⇒ ok.
 * `allow` extiende la allowlist por-llamada (no se usa hoy, pero deja la puerta abierta).
 */
export function checkSentenceCase(value: string, allow: ReadonlySet<string> = PROPER_NOUN_SET): SentenceCaseResult {
  const offenders: SentenceCaseResult["offenders"] = [];
  const rawTokens = value.split(/\s+/).filter((t) => t.length > 0);
  // Tokens alfabéticos (con al menos una letra) — los símbolos puros (·, …, ✱) se ignoran.
  const alphaIdx: number[] = [];
  rawTokens.forEach((t, i) => { if (firstLetter(t) !== null) alphaIdx.push(i); });
  if (alphaIdx.length === 0) return { ok: true, offenders };

  const allowTok = (tok: string) => allow.has(tok.replace(/[.,;:!?»«"'()[\]…]/g, "")) || isProperNoun(tok.replace(/[.,;:!?»«"'()[\]…]/g, ""));

  // PRIMER token alfabético: exigir mayúscula inicial SÓLO si el valor arranca con letra literal.
  const firstTokRaw = rawTokens[alphaIdx[0]];
  if (startsWithLiteralLetter(value)) {
    const fl = firstLetter(firstTokRaw)!;
    if (!UPPER_RE.test(fl) && !allowTok(firstTokRaw)) {
      offenders.push({ token: firstTokRaw, reason: "first-not-upper" });
    }
  }

  // Tokens siguientes: no mayúscula salvo allowlist o tras cierre de oración.
  for (let n = 1; n < alphaIdx.length; n++) {
    const i = alphaIdx[n];
    const tok = rawTokens[i];
    const fl = firstLetter(tok)!;
    if (!UPPER_RE.test(fl)) continue; // minúscula → siempre ok
    if (allowTok(tok)) continue; // nombre propio / sigla
    // ¿oración nueva? el token RAW inmediatamente anterior cierra oración.
    const prevRaw = rawTokens[i - 1];
    if (prevRaw && endsSentence(prevRaw)) continue;
    offenders.push({ token: tok, reason: "mid-title-case" });
  }

  return { ok: offenders.length === 0, offenders };
}
