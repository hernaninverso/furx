// spec-022 US1 (audit MED 2) — derivación del DISPLAY de un plugin en la UI.
//
// La identidad VISIBLE de un plugin (su label) sale del `name` SANITIZADO
// (alfanumérico + guion/underscore, truncado), NUNCA de campos libres del manifest
// (description) que podrían traer datos arbitrarios/maliciosos. El backend ya valida
// el name (is_safe_name: `^[A-Za-z0-9_-]+$`, len<64), pero defendemos también en el
// front para no confiar en una sola capa.
//
// La description, si se muestra, va como TEXTO PLANO truncado (React ya escapa HTML;
// acá además acotamos longitud y removemos controles para que no rompa el layout ni
// inyecte secuencias raras). Es texto secundario, JAMÁS el label/identidad.

const MAX_LABEL = 48;
const MAX_DESCRIPTION = 240;

/** Label visible del plugin: name sanitizado + truncado. Nunca description. */
export function pluginLabel(name: string): string {
  const safe = (name ?? "").replace(/[^A-Za-z0-9_-]+/g, "").slice(0, MAX_LABEL);
  return safe || "plugin";
}

/**
 * Texto secundario opcional (description) como texto plano truncado. Devuelve `null`
 * si no hay description útil. Remueve caracteres de control y colapsa whitespace; NO
 * interpreta markdown/HTML (React lo renderiza como texto literal de todos modos).
 */
export function pluginDescription(description?: string | null): string | null {
  if (!description) return null;
  const cleaned = description
    // Caracteres de control (C0 + DEL) → espacio; luego colapsa whitespace.
    // eslint-disable-next-line no-control-regex
    .replace(/[\x00-\x1F\x7F]+/g, " ")
    .replace(/\s+/g, " ")
    .trim();
  if (!cleaned) return null;
  return cleaned.length > MAX_DESCRIPTION ? cleaned.slice(0, MAX_DESCRIPTION) + "…" : cleaned;
}
