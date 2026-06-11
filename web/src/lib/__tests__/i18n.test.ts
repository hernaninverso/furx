// 016 US1 (T016 + T070 + T071) — tests del boundary i18n. `node --experimental-strip-types`.
// Cubre: resolución es/en, interpolación {name}, fallback al source ante miss, NUNCA key cruda,
// PARIDAD de keys es↔en (council T070, además del check de tsc), persistencia con guards (T071).
import { es } from "../../locales/es.ts";
import { en } from "../../locales/en.ts";
import { pt } from "../../locales/pt.ts";
import { it } from "../../locales/it.ts";
import { fr } from "../../locales/fr.ts";
import { de } from "../../locales/de.ts";
import { translate, getLocale, setLocale, SOURCE_LOCALE } from "../i18n.ts";

let pass = 0, fail = 0;
function ok(cond: boolean, name: string) { if (cond) pass++; else { fail++; console.log(`FAIL ${name}`); } }
function eq(a: unknown, b: unknown, name: string) { ok(JSON.stringify(a) === JSON.stringify(b), `${name} (got ${JSON.stringify(a)} want ${JSON.stringify(b)})`); }

// ── Paridad de keys + PLACEHOLDERS de TODOS los locales vs el source (063 — 6 idiomas). ─────────
// Falla si a un locale le falta/sobra una key, o si un {x} no coincide con el source.
const esKeys = Object.keys(es).sort();
ok(esKeys.length > 0, "el source tiene al menos una key");
function placeholders(s: string): string[] {
  return [...s.matchAll(/\{(\w+)\}/g)].map((m) => m[1]).sort();
}
const OTHER_LOCALES: Record<string, Record<string, string>> = { en, pt, it, fr, de };
for (const [name, cat] of Object.entries(OTHER_LOCALES)) {
  eq(Object.keys(cat).sort(), esKeys, `paridad de keys es↔${name} (mismas keys exactas)`);
  for (const k of esKeys) {
    const pe = placeholders((es as Record<string, string>)[k]);
    const pn = placeholders(cat[k] ?? "");
    eq(pn, pe, `placeholders coinciden para "${k}" en ${name}`);
  }
}

// ── Resolución es/en ───────────────────────────────────────────────────────────────────────────
eq(translate("es", "help.title"), "Centro de ayuda", "resuelve es");
eq(translate("en", "help.title"), "Help Center", "resuelve en");

// ── Interpolación {name} ─────────────────────────────────────────────────────────────────────
eq(translate("es", "help.subtitle", { count: 3 }), "3 entradas", "interpola count es");
eq(translate("en", "tour.progress", { current: 2, total: 5 }), "Step 2 of 5", "interpola dos params en");
// Param ausente → deja el placeholder literal (no rompe, no muestra undefined).
eq(translate("es", "help.subtitle", {} as { count: string | number }), "{count} entradas", "param ausente deja placeholder");

// ── Fallback al source ante miss + NUNCA key cruda ─────────────────────────────────────────────
// Forzamos un miss: pedimos una key inexistente vía cast. Debe devolver "" (no la key cruda).
const missing = translate("en", "no.such.key" as never);
eq(missing, "", "key desconocida → cadena vacía (no la key cruda)");
ok(!missing.includes("no.such.key"), "nunca expone la key cruda al usuario");

// ── Persistencia / setLocale con guards (T071) ─────────────────────────────────────────────────
// Sin localStorage definido (entorno node puro), setLocale NO debe lanzar.
ok(typeof localStorage === "undefined", "entorno test: sin localStorage (camino de guard)");
const before = getLocale();
ok(SOURCE_LOCALE === "es", "source locale = es");
setLocale("en");
eq(getLocale(), "en", "setLocale('en') aplica en memoria sin localStorage");
setLocale("es");
eq(getLocale(), "es", "setLocale('es') vuelve");
// locale inválido se ignora (no rompe).
setLocale("zz" as never);
eq(getLocale(), "es", "locale inválido ignorado");
// restaurar
setLocale(before);

console.log(`i18n: ${pass} passed, ${fail} failed`);
process.exit(fail === 0 ? 0 : 1);
