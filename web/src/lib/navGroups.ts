// web/src/lib/navGroups.ts — 015 T020 · taxonomía ESTÁTICA de la nav agrupada (055: 4 dominios).
//
// SSOT de la estructura (qué vista va en qué dominio) — testeable (cobertura: ninguna vista del
// union `View` queda huérfana). El Shell la consume e INYECTA los badges dinámicos (panes.length,
// etc.) por vista; acá NO hay estado dinámico para que el test sea puro.

// `import type`: View se usa SÓLO como tipo → Node lo borra en el type-stripping (sin resolución
// en runtime, evita ERR_MODULE_NOT_FOUND con imports extensionless). vite/tsc lo resuelven igual.
import type { View } from "./router";
import type { SidebarGroupId } from "../components/SidebarGroups";
import type { LocaleKey } from "../locales/es";

export interface NavItem {
  view: View;
  label: string;
  icon: string;
}

export interface NavGroup {
  id: SidebarGroupId;
  label: string;
  items: NavItem[];
}

/// Los 4 dominios de la espina (055). Cobertura: la unión de `items.view` + ALIASED_VIEWS == el union
/// `View` (test lo verifica).
/// Decisión del usuario (T020): Extensiones = plugins + tools; Sistema = settings.
export const NAV_GROUPS: NavGroup[] = [
  {
    id: "work",
    label: "Trabajo",
    items: [
      { view: "panes", label: "Sesiones", icon: "▤" },
      { view: "queue", label: "Cola", icon: "⊞" },
    ],
  },
  {
    id: "intelligence",
    label: "Inteligencia",
    items: [
      { view: "memory", label: "Memoria", icon: "⌬" },
      { view: "search", label: "Buscar", icon: "⌕" },
    ],
  },
  {
    // 055 — "Actividad": UN solo surface que reemplaza los 8 diagnósticos + infra + config sueltos
    // (consenso del consejo: ningún competidor expone observabilidad/infra como nav primaria). `monitors`
    // es el entry-point; el resto (reliability/savings/health/heatmap/latency/grafana/crashlog,
    // saas/ssh/vpn, policy/presets, extensions) queda navegable por ⌘K como ruta aliased (deep-links vivos).
    // El `id` sigue siendo "observability" (estable: SidebarGroupId + MOBILE_NAV_SUBSET + deep-links).
    id: "observability",
    label: "Actividad",
    items: [
      { view: "activity", label: "Actividad", icon: "▦" },
    ],
  },
  {
    id: "system",
    label: "Sistema",
    items: [{ view: "settings", label: "Ajustes", icon: "⚙" }],
  },
];

/// 047 FR-006 — vistas navegables que NO se listan en el sidebar pero siguen vivas como
/// rutas (deep-links `furx://plugins` / `furx://tools`). Quedan FUSIONADAS bajo `extensions`
/// (tabs). Se cuentan en la cobertura para que ninguna vista del union `View` quede huérfana,
/// sin volver a aparecer como entrada propia del sidebar.
export const ALIASED_VIEWS: View[] = [
  // 055 — fuera de la espina (6 ítems) pero VIVAS como rutas/deep-links y alcanzables por ⌘K. La
  // reestructura sesión-céntrica saca del sidebar las superficies que ningún competidor expone como
  // nav primaria; nada se elimina, sólo se demota a la paleta de comandos.
  "incidents", "audit", "github", "eval", "router", "replay",
  "reliability", "savings", "health", "heatmap", "latency", "grafana", "crashlog",
  "saas", "ssh", "vpn", "extensions", "policy", "presets",
  "plugins", "tools",
  // 057 — `monitors` es ahora la vista de DETALLE detrás del Action Center "Actividad" (`activity`).
  "monitors",
];

/// Todas las vistas cubiertas por la nav (para chequear cobertura/orfandad). Incluye las
/// entradas reales del sidebar + las vistas aliased (deep-link-only) → la unión cubre todo `View`.
export function coveredViews(): View[] {
  return [...NAV_GROUPS.flatMap((g) => g.items.map((i) => i.view)), ...ALIASED_VIEWS];
}

/// Sólo las vistas que el sidebar realmente renderiza como ítems (sin las aliased).
export function sidebarViews(): View[] {
  return NAV_GROUPS.flatMap((g) => g.items.map((i) => i.view));
}

// ─────────────────────────── 022 P0c · i18n de la nav (US5/FR-008) ───────────────────────────
//
// SSOT estructural sigue en `NAV_GROUPS` (qué vista va en qué dominio + ícono + label fallback que
// usa la serialización móvil). El DESKTOP traduce el label vía estas keys del catálogo. Tipado como
// `LocaleKey`: una key inexistente reventaría el catálogo (los tests verifican cobertura 1:1).

/// i18n key del label de un dominio de nav. `nav.<id>`.
export function navGroupLabelKey(id: SidebarGroupId): LocaleKey {
  return `nav.${id}` as LocaleKey;
}

/// i18n key del label de un ítem de nav. `nav.item.<view>`.
export function navItemLabelKey(view: View): LocaleKey {
  return `nav.item.${view}` as LocaleKey;
}

// ─────────────────────────── 017 mobile companion ───────────────────────────
//
// Subset CURADO de dominios para la bottom-nav del companion móvil (spec 017,
// US1/US4). SSOT único: el móvil NO duplica ids/labels — el desktop materializa
// `buildNavSpec()` y lo empuja al bridge (frame NavSpec), que lo reenvía firmado
// al móvil. Curaduría declarativa (decisión clarify + plan §Phase-0.5): sólo los
// dominios accionables desde el pulgar — Infraestructura/Extensiones/Sistema
// quedan FUERA del corte móvil (SSH/VPN/plugins no se accionan desde el teléfono).
//
// El test `navSpec.test.ts` falla si un id de este subset no existe en
// `NAV_GROUPS` (sin literales huérfanos) — cobertura análoga al
// `registry_covers_all_handler_commands` de Rust.
export const MOBILE_NAV_SUBSET: SidebarGroupId[] = [
  "work",
  "intelligence",
  "observability",
];

/// Forma serializable de la NavSpec que viaja por el bridge (frame `NavSpec`).
/// SÓLO labels/ids públicos — NUNCA estado dinámico ni datos sensibles (FR-015).
/// Idéntica a `NavItem` por contrato (mismos campos públicos) → alias, sin drift manual: agregar un
/// campo a `NavItem` (ej badge) lo propaga al shape móvil sin tener que espejar la interfaz a mano.
export type MobileNavItem = NavItem;
export interface MobileNavDomain {
  domainId: SidebarGroupId;
  label: string;
  items: MobileNavItem[];
}
export interface MobileNavSpec {
  /// Versión del shape del NavSpec (para feature-detection en el móvil).
  version: number;
  domains: MobileNavDomain[];
}

/// Versión del shape del NavSpec. Bump cuando cambie la forma (no el contenido).
export const MOBILE_NAV_SPEC_VERSION = 1;

/**
 * Translator i18n inyectado (subset estructural de `t` de lib/i18n). Se pasa para que los labels
 * de la NavSpec móvil salgan del MISMO catálogo que el desktop (`nav.<id>` / `nav.item.<view>`),
 * eliminando la divergencia es↔móvil (audit MED 2). Tipado laxo (sin dep de React/Tauri).
 */
export type NavTranslator = (key: LocaleKey) => string;

/**
 * Materializa la `MobileNavSpec` desde el SSOT (`NAV_GROUPS`) SÓLO para los ids
 * de `MOBILE_NAV_SUBSET`, preservando el orden del subset. NO inventa items: cada
 * dominio/ítem sale tal cual de `NAV_GROUPS`. Ids del subset que no existan en
 * `NAV_GROUPS` se OMITEN (el test de cobertura garantiza que el subset es válido,
 * así que en la práctica no se omite nada — el filtro es defensa, no curaduría).
 *
 * Los labels se materializan DESDE el catálogo i18n vía el translator inyectado (audit MED 2):
 * el móvil recibe los MISMOS textos traducidos que el desktop para el locale activo, en vez de los
 * literales ES de `NAV_GROUPS` (que sólo son fallback estructural). Sin translator → fallback a los
 * literales de `NAV_GROUPS` (sólo para tests/llamadas legacy).
 */
export function buildNavSpec(t?: NavTranslator): MobileNavSpec {
  const byId = new Map(NAV_GROUPS.map((g) => [g.id, g]));
  const domains: MobileNavDomain[] = [];
  for (const id of MOBILE_NAV_SUBSET) {
    const g = byId.get(id);
    if (!g) continue; // id huérfano → omitir (el test lo caza antes)
    domains.push({
      domainId: g.id,
      label: t ? t(navGroupLabelKey(g.id)) : g.label,
      items: g.items.map((i) => ({
        view: i.view,
        label: t ? t(navItemLabelKey(i.view)) : i.label,
        icon: i.icon,
      })),
    });
  }
  return { version: MOBILE_NAV_SPEC_VERSION, domains };
}
