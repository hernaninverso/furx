// web/src/lib/flags.ts — 015 T020 · feature-flags LOCALES (FR-014). Mínimo, localStorage-backed.
//
// Bootstrap del cimiento de flags: T020 lo necesita para gatear la nav agrupada (su mecanismo de
// ROLLBACK). T022 ("feature flags locales") lo generalizará (UI, scopes, más flags) — por ahora un
// registry chico + getFlag/setFlag/useFlag. Cada flag tiene un default; el valor vive en
// localStorage bajo `furx.flag.<key>`. Cambiar un flag emite un evento `furx:flag` para que los
// componentes en la MISMA pestaña re-rendericen (cross-tab vía el evento `storage` del browser).

import { useEffect, useState } from "react";

const PREFIX = "furx.flag.";

export interface FlagDef {
  /// clave estable (sufijo de `furx.flag.`).
  key: string;
  /// valor por defecto si nunca se seteó.
  default: boolean;
  /// etiqueta legible (para un toggle de UI).
  label: string;
  description?: string;
  /// 015 T022: ¿la feature que este flag gatea YA está implementada y cableada? `false` → en la UI
  /// el toggle se muestra DISABLED ("Próximamente") para NO exponer algo que no hace nada (honestidad).
  impl: boolean;
  /// 018 T053: feature implementada+auditada+tests verdes, ACTIVABLE como opt-in, pero la smoke E2E
  /// LIVE (T050: detach con agente vivo + 2 monitores) aún no se corrió. `true` → badge "beta" +
  /// aviso experimental en el panel. Consenso 3-frontera: opt-in default-off detrás de beta hasta T050.
  beta?: boolean;
}

/// Registry de flags conocidos (FR-014). `groupedNav` es el único cableado a una feature live (T020);
/// el resto son placeholders de features futuras (impl:false → disabled en la UI hasta que se cableen).
export const FLAGS = {
  /// Nav agrupada por dominios (T020). OFF = lista plana de TODAS las vistas (rollback). LIVE.
  groupedNav: {
    key: "sidebar.grouped",
    default: true,
    label: "Navegación agrupada",
    description: "Agrupa las vistas en dominios. Apagar = lista plana de todas las vistas (rollback).",
    impl: true,
  },
  /// Workspace editable (layouts libres). Fase 2. ACTIVABLE como opt-in beta (default OFF) — gatea
  /// TODO el surface 018: render v1 (dockview) + detach-to-window + multi-monitor + edición. El detach
  /// se expone vía `leaseWindowLabel` en WorkspaceView, así que este flag solo ya habilita detach.
  newWorkspace: {
    key: "newWorkspace",
    default: false,
    label: "Workspace editable",
    description: "Editor de layout libre con detach a ventanas y multi-monitor. Beta hasta validar 2 monitores en vivo.",
    impl: true,
    beta: true,
  },
  /// Command palette v2 (la ⌘K del kernel ya es la activa; este flag queda para A/B futuro).
  commandPaletteV2: {
    key: "commandPaletteV2",
    default: false,
    label: "Command palette v2",
    description: "Variante experimental de la ⌘K. Próximamente.",
    impl: false,
  },
  /// Ventanas desacopladas (detach-to-window). Fase 2. NOTA: hoy NO gatea código propio — el detach
  /// se habilita junto con `newWorkspace` (vía `leaseWindowLabel` en WorkspaceView). Queda impl:false
  /// para no implicar una capacidad standalone que no está cableada por separado (honestidad).
  detachedWindows: {
    key: "detachedWindows",
    default: false,
    label: "Ventanas desacopladas",
    description: "Incluido en «Workspace editable». Próximamente como toggle independiente.",
    impl: false,
  },
  /// Companion móvil (la app móvil es un feature aparte; este flag gatearía su UI en desktop).
  mobileCompanion: {
    key: "mobileCompanion",
    default: false,
    label: "Companion móvil",
    description: "Controles del companion móvil en el desktop. Próximamente.",
    impl: false,
  },
  /// Paneles de plugins (UI propia de plugins embebida). Próximamente.
  pluginPanels: {
    key: "pluginPanels",
    default: false,
    label: "Paneles de plugins",
    description: "Permitir que los plugins rendericen paneles de UI propios. Próximamente.",
    impl: false,
  },
  /// 016 US2 — Help Center contextual y buscable (derivado del registry + navGroups).
  helpCenter: {
    key: "helpCenter",
    // 059 — default ON: el pill "? Ayuda" del TopBar + el HelpCenter deben verse en un producto
    // vendible (antes default false → la ayuda quedaba oculta para el usuario nuevo).
    default: true,
    label: "Centro de ayuda",
    description: "Ayuda contextual y buscable derivada de los comandos y los dominios de navegación.",
    impl: true,
  },
  /// 016 US3 — What's New consciente de versión (no-modal).
  whatsNew: {
    key: "whatsNew",
    default: true, // 059 — ON: avisar qué cambió al actualizar (no-modal, no interrumpe).
    label: "Novedades",
    description: "Muestra qué cambió al actualizar Furx, sin interrumpir (indicador no-modal).",
    impl: true,
  },
  /// 016 US4 — Tours de onboarding guiados sobre los dominios principales.
  tours: {
    key: "tours",
    default: true, // 059 — ON: onboarding guiado (se ofrece, no se fuerza).
    label: "Tours guiados",
    description: "Recorrido de onboarding que orienta por los dominios principales. Se ofrece, no se fuerza.",
    impl: true,
  },
  /// 047 FR-008 — refresco de incidentes/atención DIRIGIDO-POR-EVENTOS (push del event bus) en vez de
  /// puro polling por intervalo. ON: se suscribe a TaskChanged/AgentStateChanged/CommandExecuted y
  /// refresca al instante; el intervalo baja a una red de seguridad lenta (fallback). OFF (default):
  /// polling por intervalo como hoy (cero regresión). Default OFF: el push aún no se validó en vivo.
  orchestrationSSE: {
    key: "orchestrationSSE",
    default: false,
    label: "Actualización por eventos (incidentes)",
    description: "Refresca incidentes y la cola de atención por push del backend en vez de sondeo periódico. Si el push falla, cae al sondeo. Beta.",
    impl: true,
    beta: true,
  },
  /// 016 US5 — Telemetry opt-in privacy-by-default (eventos agregados, nunca contenido/keys).
  telemetry: {
    key: "telemetry",
    default: false,
    label: "Telemetría de uso",
    description: "Instrumentación de eventos agregados. Sólo emite con opt-in + endpoint configurado. Nunca contenido ni claves.",
    impl: true,
  },
} as const satisfies Record<string, FlagDef>;

export type FlagName = keyof typeof FLAGS;

/// Lee el valor actual de un flag (default si nunca se seteó, fuera del browser, o si el acceso a
/// localStorage lanza — modo privado/sandboxed; audit MED). Fail-safe al default.
export function getFlag(name: FlagName): boolean {
  const def = FLAGS[name];
  try {
    if (typeof localStorage === "undefined") return def.default;
    const raw = localStorage.getItem(PREFIX + def.key);
    return raw === null ? def.default : raw === "true";
  } catch {
    return def.default;
  }
}

/// Setea un flag y notifica a los consumidores de la misma pestaña (evento `furx:flag`).
export function setFlag(name: FlagName, value: boolean): void {
  try {
    if (typeof localStorage === "undefined") return;
    localStorage.setItem(PREFIX + FLAGS[name].key, String(value));
  } catch {
    /* localStorage no disponible/lleno — el evento igual notifica a la sesión actual */
  }
  if (typeof window !== "undefined") {
    window.dispatchEvent(new CustomEvent("furx:flag", { detail: { name, value } }));
  }
}

/// Hook React: devuelve `[valor, setValor]` y re-renderiza cuando el flag cambia (misma pestaña vía
/// `furx:flag`, otras pestañas vía `storage`).
export function useFlag(name: FlagName): [boolean, (v: boolean) => void] {
  const [value, setValue] = useState<boolean>(() => getFlag(name));
  useEffect(() => {
    const sync = () => setValue(getFlag(name));
    window.addEventListener("furx:flag", sync);
    window.addEventListener("storage", sync);
    return () => {
      window.removeEventListener("furx:flag", sync);
      window.removeEventListener("storage", sync);
    };
  }, [name]);
  return [value, (v: boolean) => setFlag(name, v)];
}
