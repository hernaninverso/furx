// web/src/lib/i18n.ts — 016 US1 · boundary i18n PROPIO (sin dependencias, council T070/T071).
//
// `t(key, params?)` resuelve la cadena del locale activo con interpolación `{name}`. Diseño:
//   - `es` (source) define las keys (`LocaleKey`); `en` se tipa con `satisfies Record<LocaleKey,string>`
//     → paridad de keys verificada por `tsc -b` (falta/sobra ⇒ build rojo). T070.
//   - Params de interpolación TIPADOS por key: `TParams<K>` deriva los placeholders `{x}` del string
//     SOURCE vía template-literal types. `t("help.subtitle", { count })` exige `count`; una key sin
//     placeholders no acepta params. Council T070.
//   - Miss (key faltante en el locale activo) → texto del SOURCE (es) como fallback; si tampoco está
//     (imposible por tipos, pero defensivo) → la propia key NO se muestra cruda: se devuelve "" + warn.
//     warn SÓLO en DEV. FR-003.
//   - Persistencia: `localStorage('furx.locale')` con GUARDS (try/catch, fallback memoria) — T071.
//   - Anti-FOUC: el script inline de index.html setea `<html lang>` + global `__FURX_LOCALE__` antes
//     del bundle; este módulo lo lee al inicializar (sin flash de idioma). FR-004.
//   - React: `<I18nProvider>` + `useT()` re-renderizan al cambiar idioma SIN reiniciar. FR-005.

import { createContext, createElement, useContext, useEffect, useState, type ReactNode } from "react";
// `.ts` explícito en imports de VALOR relativos: lo exige el type-stripping nativo de Node (con el
// que corre la suite, scripts/test-all.mjs); tsconfig tiene `allowImportingTsExtensions:true` y Vite
// resuelve `.ts` sin problema. (Los `import type` no necesitan extensión: Node los borra.)
import { es, type LocaleKey } from "../locales/es.ts";
import { en } from "../locales/en.ts";
import { pt } from "../locales/pt.ts";
import { it } from "../locales/it.ts";
import { fr } from "../locales/fr.ts";
import { de } from "../locales/de.ts";

/// Idiomas soportados (063 — 6 locales). Agregar uno = sumar un archivo de locale + su import acá.
export const LOCALES = ["es", "en", "pt", "it", "fr", "de"] as const;
export type Locale = (typeof LOCALES)[number];

/// SOURCE locale: define las keys y es el fallback final. (es) — council T070.
export const SOURCE_LOCALE: Locale = "es";

/// Catálogos por idioma. `es` es `Record<LocaleKey, string>` por construcción; el resto por `satisfies`.
const CATALOGS: Record<Locale, Record<LocaleKey, string>> = { es, en, pt, it, fr, de };

const STORAGE_KEY = "furx.locale";

/* ── Tipado de params de interpolación por key (template-literal types) ──────────────────────────
 * `Placeholders<S>` extrae los nombres `{x}` de un string literal S como union. `TParams<K>` los
 * mapea a `{ [P]: string | number }`; si no hay placeholders → `undefined` (params no requeridos). */
type Placeholders<S extends string> =
  S extends `${string}{${infer P}}${infer Rest}` ? P | Placeholders<Rest> : never;

type SourceString<K extends LocaleKey> = (typeof es)[K];

/// Params requeridos por la key K (derivados del SOURCE). Sin placeholders ⇒ `undefined`.
export type TParams<K extends LocaleKey> =
  [Placeholders<SourceString<K>>] extends [never]
    ? undefined
    : Record<Placeholders<SourceString<K>>, string | number>;

/* ── M3 (audit): paridad de PLACEHOLDERS en build-time ───────────────────────────────────────────
 * `satisfies Record<LocaleKey,string>` en en.ts ya garantiza paridad de KEYS. Esto agrega paridad de
 * placeholders: para cada key, el set de `{x}` en `en` debe ser idéntico al del SOURCE `es`. Un
 * mismatch (p.ej. `es:"{count}"` vs `en:"{cuenta}"`) hace que `PlaceholderParity` incluya `false`
 * (la union deja de ser `true`) y el `tsc -b` FALLA acá — no en runtime con un placeholder sin
 * reemplazar. */
type SetEq<A, B> = [A] extends [B] ? ([B] extends [A] ? true : false) : false;
type PlaceholderParity = {
  [K in LocaleKey]: SetEq<Placeholders<(typeof es)[K]>, Placeholders<(typeof en)[K]>>;
}[LocaleKey];
const _placeholderParity: PlaceholderParity extends true ? true : never = true;
void _placeholderParity;

/* ── Estado del idioma activo (singleton por webview) ────────────────────────────────────────── */

let activeLocale: Locale = readInitialLocale();
const listeners = new Set<(l: Locale) => void>();

/// Lee el idioma inicial: global anti-FOUC (set por index.html) → localStorage → navigator → source.
/// Todos los accesos a storage/global con GUARDS (T071): modo privado/sandbox NUNCA rompe el boot.
function readInitialLocale(): Locale {
  // 1) global seteado por el script anti-FOUC (ya resolvió storage/navigator antes del paint).
  try {
    const g = (globalThis as { __FURX_LOCALE__?: unknown }).__FURX_LOCALE__;
    if (typeof g === "string" && isLocale(g)) return g;
  } catch { /* ignore */ }
  // 2) localStorage directo (si el global no estaba — p.ej. tests / SSR).
  try {
    if (typeof localStorage !== "undefined") {
      const raw = localStorage.getItem(STORAGE_KEY);
      if (raw && isLocale(raw)) return raw;
    }
  } catch { /* ignore */ }
  // 3) idioma del SO.
  try {
    const navLang = typeof navigator !== "undefined" ? navigator.language?.slice(0, 2) : undefined;
    if (navLang && isLocale(navLang)) return navLang;
  } catch { /* ignore */ }
  // 4) fallback: inglés (default de producto, brand wave 4 2026-06-09 — espeja el anti-FOUC de
  //    index.html). El SOURCE sigue siendo `es` (define keys + fallback de strings faltantes),
  //    pero el idioma DEFAULT cuando nada decide es EN; ES queda elegible vía LanguageSwitch.
  return "en";
}

function isLocale(s: string): s is Locale {
  return (LOCALES as readonly string[]).includes(s);
}

/// Idioma activo actual.
export function getLocale(): Locale {
  return activeLocale;
}

/// Cambia el idioma activo, persiste (con guard) y notifica a los consumidores (re-render). FR-005.
export function setLocale(locale: Locale): void {
  if (!isLocale(locale) || locale === activeLocale) {
    // aún si es el mismo, persistir explícitamente la elección (override de navigator).
    if (isLocale(locale)) persistLocale(locale);
    return;
  }
  activeLocale = locale;
  persistLocale(locale);
  try {
    if (typeof document !== "undefined") document.documentElement.lang = locale;
  } catch { /* ignore */ }
  for (const l of listeners) l(locale);
}

function persistLocale(locale: Locale): void {
  try {
    if (typeof localStorage !== "undefined") localStorage.setItem(STORAGE_KEY, locale);
  } catch { /* storage no disponible/lleno — el estado en memoria igual aplica */ }
  try {
    (globalThis as { __FURX_LOCALE__?: string }).__FURX_LOCALE__ = locale;
  } catch { /* ignore */ }
}

/// Suscripción cruda a cambios de idioma (para el provider/hook). Devuelve unsubscribe.
export function subscribeLocale(fn: (l: Locale) => void): () => void {
  listeners.add(fn);
  return () => { listeners.delete(fn); };
}

/* ── Interpolación + resolución ──────────────────────────────────────────────────────────────── */

function interpolate(template: string, params?: Record<string, string | number>): string {
  if (!params) return template;
  return template.replace(/\{(\w+)\}/g, (m, name: string) =>
    name in params ? String(params[name]) : m,
  );
}

/// Resuelve una key contra un locale dado. Devuelve `null` si la key no existe en ESE locale (para
/// que el caller caiga al source). NO interpola acá (lo hace `translate`).
function lookup(locale: Locale, key: LocaleKey): string | null {
  const cat = CATALOGS[locale];
  const raw = cat[key];
  return typeof raw === "string" ? raw : null;
}

/**
 * Núcleo de traducción (puro, testeable sin React). Resuelve en `locale`, cae al SOURCE si falta, y
 * NUNCA expone la key cruda: si falta en ambos (imposible por tipos) → "" + warn(DEV).
 */
export function translate<K extends LocaleKey>(
  locale: Locale,
  key: K,
  params?: TParams<K>,
): string {
  let raw = lookup(locale, key);
  if (raw === null) {
    // miss en el locale activo → fallback al source. warn sólo en DEV (FR-003).
    if (locale !== SOURCE_LOCALE && isDev()) {
      console.warn(`[i18n] miss "${key}" en locale "${locale}" → fallback al source`);
    }
    raw = lookup(SOURCE_LOCALE, key);
  }
  if (raw === null) {
    if (isDev()) console.warn(`[i18n] key desconocida "${key}" — devolviendo cadena vacía (no la key cruda)`);
    return "";
  }
  return interpolate(raw, params as Record<string, string | number> | undefined);
}

function isDev(): boolean {
  // Vite expone import.meta.env.DEV; en Node/tests cae a false sin romper.
  try {
    return Boolean((import.meta as unknown as { env?: { DEV?: boolean } }).env?.DEV);
  } catch {
    return false;
  }
}

/// `t()` standalone (usa el idioma activo del módulo). Útil fuera de React (datos, locales en tests).
export function t<K extends LocaleKey>(key: K, params?: TParams<K>): string {
  return translate(activeLocale, key, params);
}

/* ── React provider + hook ───────────────────────────────────────────────────────────────────── */

interface I18nContextValue {
  locale: Locale;
  setLocale: (l: Locale) => void;
  t: <K extends LocaleKey>(key: K, params?: TParams<K>) => string;
}

const I18nContext = createContext<I18nContextValue | null>(null);

/// Provider: re-renderiza el árbol al cambiar idioma (sin reinicio, FR-005). Se suscribe al singleton
/// para que cambios vía `setLocale()` standalone también propaguen.
export function I18nProvider({ children }: { children: ReactNode }) {
  const [locale, setLocaleState] = useState<Locale>(getLocale);
  useEffect(() => subscribeLocale(setLocaleState), []);
  const value: I18nContextValue = {
    locale,
    setLocale,
    t: <K extends LocaleKey>(key: K, params?: TParams<K>) => translate(locale, key, params),
  };
  return createElement(I18nContext.Provider, { value }, children);
}

/// Hook: devuelve `{ t, locale, setLocale }`. Fuera del provider, cae al `t` standalone (degradación
/// segura: nunca rompe un render por falta de provider).
export function useI18n(): I18nContextValue {
  const ctx = useContext(I18nContext);
  if (ctx) return ctx;
  return { locale: getLocale(), setLocale, t };
}

/// Atajo: sólo la función de traducción (el caso más común en JSX).
export function useT(): I18nContextValue["t"] {
  return useI18n().t;
}
