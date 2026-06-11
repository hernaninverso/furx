// web/src/lib/router.ts — 015 T013 · US9 · router INTERNO + deep-linking (`furx://`).
//
// Direcciones internas ESTABLES para que la ⌘K palette navegue, el Help sea contextual y (Fase 2)
// el multi-window/móvil restauren contexto. Es navegación IN-APP (no el deep-link OS-level de
// tauri-plugin-deep-link, que es Fase 2). Parser PURO + un navegador que el Shell cablea a su
// `setView` (+ scroll a la sección de Settings). El estado de la ruta vigente vive en el Shell
// (efímero, window-scoped) — sin singleton: cada ventana tendrá su propia ruta (multi-window-ready).
//
// SSOT del union `View`: este módulo lo define y el Shell lo importa, así la nav agrupada (T020),
// el palette y el router comparten UNA sola lista de vistas (sin drift).

/// Vistas top-level de la app. Mirror de las que el Shell renderiza. (La vista "router" es la viz
/// de grafo de routing — homónima del módulo, sin relación; `furx://router` navega a ESA vista.)
export const VIEWS = [
  "panes", "incidents", "monitors", "audit", "settings", "saas", "health", "heatmap",
  "grafana", "ssh", "vpn", "latency", "search", "eval", "queue", "router", "replay",
  "tools", "memory", "plugins",
  // 015 T030 — vistas para los huérfanos backend que no tenían UI.
  "crashlog", "github",
  // 050 Ola 8 P2 (FR-003) — reliability board (éxito/latencia/costo por agente/modelo).
  "reliability",
  // 048 Cost-router Fase 1 — Savings Meter (ahorro medido del routing que Furx ya hace).
  "savings",
  // 047 FR-006 — Extensiones unificadas (Plugins + Skills en una vista con tabs).
  // `plugins`/`tools` quedan como rutas vivas (deep-links viejos) pero salen del sidebar.
  "extensions",
  // 053 — vistas para comandos que tenían backend sin UI.
  "policy", "presets",
  // 057 — "activity" = Action Center (centro de excepciones: ahorro + alertas accionables).
  // Es la entrada de espina "Actividad"; `monitors` pasa a vista de detalle (alcanzable por ⌘K).
  "activity",
] as const;
export type View = (typeof VIEWS)[number];

/// 015 T031 — modales POTENTES descubribles vía `furx://modal/<name>` + el palette. El Shell mapea
/// cada nombre a su estado de apertura (onOpenModal); el navigator NO toca el estado del Shell.
/// Sólo los que tienen apertura standalone real (honestidad): `signals` vive en Settings →
/// Integraciones (`furx://settings/integrations`) y `bestofn` dentro de Orquestación.
export const MODALS = ["agents", "orchestration", "council", "voice"] as const;
export type ModalName = (typeof MODALS)[number];

/// Secciones de Settings (mirror de los `<section id>` de Settings.tsx). Sub-rutas de `furx://settings/<id>`.
export const SETTINGS_SECTIONS = [
  "connect", "cloud", "endpoints", "privacy", "mobile", "integrations", "compat",
  "updates", "data", "advanced", "legal", "about",
] as const;
export type SettingsSection = (typeof SETTINGS_SECTIONS)[number];

/// Una ruta interna parseada.
export interface Route {
  view: View;
  /// sub-ruta reconocida (hoy: una sección de Settings). `undefined` = top de la vista.
  section?: string;
  /// un sub NO reconocido se preserva acá (fail-soft: se abre el top de la vista, no se invalida
  /// la ruta — un link viejo de Help/restore no debe varar al usuario).
  invalidSection?: string;
}

const SCHEME = "furx://";
const MODAL_PREFIX = "furx://modal/";
// 016 — Help/What's New son SUPERFICIES (overlays), NO vistas. Tienen su propio prefijo y parser,
// igual que los modales potentes (parseModalRoute), para que `parseRoute` siga devolviendo null y no
// haya drift con el union `View`. El Shell decide su efecto (abrir Help posicionado / abrir panel).
const HELP_PREFIX = "furx://help";
const WHATSNEW_PREFIX = "furx://whatsnew";

/// Parsea `furx://modal/<name>` → el nombre del modal (o null si no es una ruta de modal válida).
/// Se chequea ANTES que parseRoute (un modal NO es una vista).
export function parseModalRoute(input: unknown): ModalName | null {
  if (typeof input !== "string" || !input.startsWith(MODAL_PREFIX)) return null;
  const name = input.slice(MODAL_PREFIX.length).replace(/\/+$/, "");
  return (MODALS as readonly string[]).includes(name) ? (name as ModalName) : null;
}

/// 016 US2 — destino de Help. `furx://help` (top) o `furx://help/<section>` (contextual).
/// `<section>` es libre (no se valida contra un enum: las secciones de Help son dinámicas —
/// dominios de navGroups + ids de comando). `undefined` = abrir Help en el top.
export interface HelpRoute {
  /// sección/ancla de Help donde posicionarse (dominio o id de comando). `undefined` = top.
  section?: string;
}

/// Parsea `furx://help[/<section>]` → HelpRoute (o null si no es una ruta de Help).
/// Se chequea ANTES que parseRoute (Help NO es una vista). Fail-soft: cualquier sección se preserva.
export function parseHelpRoute(input: unknown): HelpRoute | null {
  if (typeof input !== "string") return null;
  if (input === HELP_PREFIX || input === HELP_PREFIX + "/") return {};
  if (!input.startsWith(HELP_PREFIX + "/")) return null;
  const section = input.slice(HELP_PREFIX.length + 1).replace(/\/+$/, "");
  return section === "" ? {} : { section };
}

/// Parsea `furx://whatsnew` → true si es la ruta de What's New (sin sub-rutas). Se chequea ANTES que
/// parseRoute (What's New NO es una vista).
export function isWhatsNewRoute(input: unknown): boolean {
  if (typeof input !== "string") return false;
  const trimmed = input.replace(/\/+$/, "");
  return trimmed === WHATSNEW_PREFIX;
}

/// Construye una URL canónica de Help para que Help/palette armen links type-safe.
export function buildHelpRoute(section?: string): string {
  return section ? `${HELP_PREFIX}/${section}` : HELP_PREFIX;
}

function isView(s: string): s is View {
  return (VIEWS as readonly string[]).includes(s);
}
function isSettingsSection(s: string): s is SettingsSection {
  return (SETTINGS_SECTIONS as readonly string[]).includes(s);
}

/**
 * Parsea una ruta interna `furx://<view>[/<sub>]`.
 *   - esquema inválido o VIEW desconocida → `null` (error de programación; el caller no navega).
 *   - sub desconocido → NO invalida: devuelve `{view, invalidSection}` (fail-soft → top de la vista).
 *   - `furx://settings/mobile` → `{view:"settings", section:"mobile"}`.
 */
export function parseRoute(input: unknown): Route | null {
  if (typeof input !== "string" || !input.startsWith(SCHEME)) return null;
  const path = input.slice(SCHEME.length).replace(/^\/+|\/+$/g, "");
  if (path === "") return null;
  const [viewSeg, ...rest] = path.split("/");
  if (!isView(viewSeg)) return null;
  const view = viewSeg;
  const sub = rest.join("/");
  if (sub === "") return { view };
  // Hoy sólo `settings` tiene subs conocidos (sus section ids). Otras vistas: fail-soft.
  if (view === "settings" && isSettingsSection(sub)) return { view, section: sub };
  return { view, invalidSection: sub };
}

/**
 * Construye una URL `furx://` canónica desde una vista (+ sección opcional), para que el palette y
 * el Help armen links type-safe en vez de strings ad-hoc.
 */
export function buildRoute(view: View, section?: string): string {
  return section ? `${SCHEME}${view}/${section}` : `${SCHEME}${view}`;
}

/// Un destino de navegación ofrecible por el palette (vista o sección de Settings).
export interface NavTarget {
  /// URL `furx://…` canónica.
  route: string;
  /// etiqueta legible.
  label: string;
}

/// Etiquetas legibles de las secciones de Settings (mirror de Settings.tsx).
const SECTION_LABELS: Record<SettingsSection, string> = {
  connect: "Conexión", cloud: "Cuenta cloud", endpoints: "Servicios", privacy: "Privacidad",
  mobile: "Móvil", integrations: "Integraciones", compat: "Sistema", updates: "Actualizaciones",
  data: "Datos", advanced: "Avanzado", legal: "Legal", about: "Acerca de",
};

/**
 * Destinos de navegación (las 20 vistas + las secciones de Settings) para que el palette los
 * ofrezca como entradas SINTÉTICAS. NO son comandos Tauri: viven sólo en el front, así que NO
 * tocan el Command Registry de Rust ni su test de cobertura 1:1 (decisión C del council).
 */
/// Etiquetas legibles de los modales potentes.
const MODAL_LABELS: Record<ModalName, string> = {
  agents: "Galería de agentes",
  orchestration: "Orquestación (agentes)",
  council: "Council",
  voice: "Voz",
};

export function navTargets(): NavTarget[] {
  const out: NavTarget[] = VIEWS.map((v) => ({ route: buildRoute(v), label: `Ir a: ${v}` }));
  for (const s of SETTINGS_SECTIONS) {
    out.push({ route: buildRoute("settings", s), label: `Settings → ${SECTION_LABELS[s]}` });
  }
  // 015 T031 — modales potentes descubribles desde el palette.
  for (const m of MODALS) {
    out.push({ route: `${MODAL_PREFIX}${m}`, label: `Abrir: ${MODAL_LABELS[m]}` });
  }
  // 016 US2/US3 — Help y What's New descubribles desde el palette (⌘K → "Ayuda"/"Novedades"). El
  // Shell sólo abre el overlay si el flag correspondiente está ON (navigate es no-op si no).
  out.push({ route: HELP_PREFIX, label: "Ayuda" });
  out.push({ route: WHATSNEW_PREFIX, label: "Novedades" });
  return out;
}

/// El navegador que el Shell cablea: parsea + aplica (setView + scroll a la sección).
export interface InternalNavigator {
  /// Navega a `url`. Devuelve la `Route` aplicada, o `null` si la ruta era inválida (no navega).
  navigate: (url: string) => Route | null;
}

/**
 * Crea el navegador inyectando los efectos del Shell (`setView`, y opcionalmente `scrollToSection`
 * para las sub-rutas de Settings). Mantiene el módulo PURO/testeable: el Shell provee los efectos.
 *
 * `scrollToSection(id) -> boolean`: true si encontró el elemento y scrolleó. El navegador
 * REINTENTA por unos frames (audit MED deepseek/AIE): tras `setView` la vista de Settings puede no
 * estar montada en el 1er rAF (más aún si fuera lazy en el futuro); reintentar hasta ~12 frames
 * (~200ms) cubre el mount sin asumir render síncrono. Bounded → no loop infinito si el id no existe.
 */
export function createNavigator(
  setView: (v: View) => void,
  scrollToSection?: (id: string) => boolean,
  /// 015 T031 — abre un modal potente por nombre. El navigator se mantiene AGNÓSTICO del estado del
  /// Shell: éste inyecta el callback que mapea cada nombre a su `setXOpen(true)`.
  onOpenModal?: (modal: ModalName) => void,
  /// 016 US2/US3 — abrir Help (con sección contextual opcional) / What's New. El Shell inyecta los
  /// setters. El navigator sigue agnóstico del estado del Shell.
  onOpenHelp?: (section?: string) => void,
  onOpenWhatsNew?: () => void,
): InternalNavigator {
  return {
    navigate(url: string): Route | null {
      // 015 T031 — ruta de modal: abre el modal (sin tocar la vista de fondo) y termina.
      const modal = parseModalRoute(url);
      if (modal) {
        onOpenModal?.(modal);
        return null;
      }
      // 016 US2 — Help (con sección contextual opcional). Es un overlay, no una vista.
      const help = parseHelpRoute(url);
      if (help) {
        onOpenHelp?.(help.section);
        return null;
      }
      // 016 US3 — What's New (overlay no-modal).
      if (isWhatsNewRoute(url)) {
        onOpenWhatsNew?.();
        return null;
      }
      const route = parseRoute(url);
      if (!route) return null;
      setView(route.view);
      if (route.section && scrollToSection) {
        const target = route.section;
        // Guard (audit LOW gemini): fuera del browser (SSR/CI sin rAF) caemos a un único intento.
        const raf =
          typeof requestAnimationFrame !== "undefined"
            ? requestAnimationFrame
            : (cb: () => void) => setTimeout(cb, 16);
        let attempts = 12;
        const attempt = () => {
          if (scrollToSection(target)) return; // encontró + scrolleó
          if (--attempts > 0) raf(attempt);
        };
        raf(attempt);
      }
      return route;
    },
  };
}
