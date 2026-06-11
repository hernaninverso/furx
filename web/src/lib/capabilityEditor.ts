// 022 US11 / FR-016 — editor de CAPABILITIES por agent-profile (lógica pura, testeable).
//
// Activar un plugin/MCP/codebase-memory en un perfil = agregar su `name` (slug) al
// array `agent_profile.plugins`. Al guardar el perfil (`agent_profile_update`), el
// backend dispara la inyección MCP (`mcp_inject.rs`) y el indexado del repo
// (`codebase_index.rs`). Esta capa NO baja ninguna verificación de firma/sandbox: el
// motor Rust sigue siendo el dueño del gate. Acá sólo:
//   1) derivamos qué grants BYOK declara un plugin (para mostrarlos ANTES de activar),
//   2) clasificamos el plugin (MCP server / codebase-memory / tool) para distinguirlo,
//   3) decidimos si es activable (un plugin globalmente disabled o sin firma NO),
//   4) toggléamos el name dentro del array de forma inmutable y deduplicada.
//
// La fuente de los plugins es `plugins_list` (disco = SoT, ya unificado en P0a). Cada
// Plugin trae `manifest.permissions: string[]` con strings ya derivados por el backend
// (`describe_permissions`): "mcp", "net: <host>", "net: ninguno", "shell", "byok: <s>".
// Mapeamos esos strings a badges tipados; no inventamos permisos que el backend no declaró.

/** Forma mínima de un plugin instalado (subset de `Plugin` de plugins_list). */
export interface InstalledPlugin {
  /** identidad estable = nombre en disco; es lo que se persiste en agent_profile.plugins. */
  name: string;
  version: string;
  /** enable/disable GLOBAL (P0a). Un plugin global-disabled NO se puede activar por perfil. */
  enabled: boolean;
  /** firma Ed25519 válida (fail-closed). verified=false → no activable (el motor lo rehúsa). */
  verified: boolean;
  /** permisos derivados por el backend (describe_permissions): strings legibles. */
  permissions: string[];
}

/** Nombre del plugin especial que dispara el indexado del repo del proyecto. */
export const CODEBASE_MEMORY_PLUGIN = "codebase-memory";

/** Clasificación visual de un plugin para distinguirlo en el selector. */
export type PluginKind = "codebase-memory" | "mcp" | "tool";

/** Tono semántico de un badge de grant (mapea a tokens V3 / colores de severidad). */
export type GrantTone = "neutral" | "info" | "warn" | "danger";

/** Un badge de grant BYOK / permiso sensible que el plugin declara. */
export interface GrantBadge {
  /** clave estable para React key + tests. */
  key: string;
  /** glifo accesible (decorativo; el label lleva la semántica). */
  icon: string;
  /** texto en español, sentence-case, sin la palabra prohibida (F-III). */
  label: string;
  /** título largo para tooltip/aria. */
  title: string;
  tone: GrantTone;
}

/**
 * Clasifica un plugin por su nombre + permisos. `codebase-memory` es especial (dispara
 * indexado). Cualquier plugin que declare "mcp" es un server MCP. El resto, tool.
 */
export function pluginKind(plugin: Pick<InstalledPlugin, "name" | "permissions">): PluginKind {
  if (plugin.name === CODEBASE_MEMORY_PLUGIN) return "codebase-memory";
  if (plugin.permissions.some((p) => p === "mcp")) return "mcp";
  return "tool";
}

/** Etiqueta legible (español) del tipo de plugin para el chip de categoría. */
export function pluginKindLabel(kind: PluginKind): string {
  switch (kind) {
    case "codebase-memory":
      return "Memoria de código";
    case "mcp":
      return "Servidor MCP";
    case "tool":
      return "Herramienta";
  }
}

/**
 * Marcadores que NO son grants reales (no representan un permiso concedido): el tipo
 * "mcp" y la AUSENCIA explícita de red. Estos se OMITEN del listado de badges. Todo lo
 * demás que el backend declare en `manifest.permissions` SÍ se muestra (catch-all),
 * para que NINGÚN permiso declarado quede oculto al usuario.
 */
const NON_GRANT_MARKERS = new Set(["mcp", "net: ninguno", "net: none", ""]);

/**
 * Deriva los badges de grant que el usuario otorga al activar un plugin, a partir de
 * los permisos declarados (strings del backend). Transparencia + gobierno: el usuario
 * ve QUÉ concede ANTES de activar. NO incluye "mcp" (eso es tipo, no grant) ni
 * "net: ninguno"/"net: none" (la AUSENCIA de red no es un grant). Orden estable:
 * secrets → red → shell → escritura de archivos → otros (catch-all), para que lo más
 * sensible (Keychain) salga primero.
 *
 * `shell` y `fs_write` son grants DISTINTOS y se muestran por separado: un manifest que
 * declara AMBOS produce AMBOS badges (nunca uno se come al otro).
 *
 * CATCH-ALL (HIGH fix): cualquier permiso declarado que NO matchee una categoría
 * conocida (ni sea un marcador NON_GRANT) se muestra igual como badge `warn` con el
 * string CRUDO sanitizado, para que el usuario vea EXACTAMENTE qué pidió el plugin.
 * Así no hay falsos negativos: ni `filesystem`, `exec`, `db`, ni cualquier permiso
 * futuro desconocido queda oculto.
 */
export function grantBadges(permissions: string[]): GrantBadge[] {
  const secrets: GrantBadge[] = [];
  const nets: GrantBadge[] = [];
  let shell: GrantBadge | null = null;
  let fsWrite: GrantBadge | null = null;
  const others: GrantBadge[] = [];
  const seenOther = new Set<string>();

  for (const perm of permissions) {
    const p = perm.trim();
    if (NON_GRANT_MARKERS.has(p)) continue;

    // byok: <secret> → acceso a un secreto del Keychain (lo más sensible).
    const byok = matchPrefix(p, "byok:");
    if (byok !== null) {
      const name = byok.trim();
      secrets.push({
        key: `byok:${name}`,
        icon: "🔑",
        label: name ? `Acceso a Keychain · ${name}` : "Acceso a Keychain",
        title: name
          ? `El plugin pide leer el secreto «${name}» de tu Keychain (BYOK). El valor nunca se persiste ni se loguea.`
          : "El plugin pide acceso a un secreto de tu Keychain (BYOK).",
        tone: "danger",
      });
      continue;
    }

    // net: <host> → acceso de red a ese host.
    const net = matchPrefix(p, "net:");
    if (net !== null) {
      const host = net.trim();
      nets.push({
        key: `net:${host}`,
        icon: "🌐",
        label: host ? `Red · ${host}` : "Acceso de red",
        title: host
          ? `El plugin puede hacer red hacia «${host}» (default-deny: sólo ese host).`
          : "El plugin declara acceso de red.",
        tone: "warn",
      });
      continue;
    }

    // shell → puede correr comandos. Grant SEPARADO de fs_write.
    if (p === "shell") {
      shell = shell ?? {
        key: "shell",
        icon: "📟",
        label: "Shell",
        title: "El plugin puede ejecutar comandos de shell.",
        tone: "danger",
      };
      continue;
    }

    // fs-write / archivos (algunos manifests legacy declaran permisos de archivo crudos).
    // Grant SEPARADO de shell: si un manifest declara AMBOS, se muestran los dos.
    if (p.startsWith("fs_write") || p.startsWith("fs-write")) {
      fsWrite = fsWrite ?? {
        key: "fs-write",
        icon: "📁",
        label: "Escritura de archivos",
        title: "El plugin puede escribir archivos.",
        tone: "warn",
      };
      continue;
    }

    // CATCH-ALL: permiso declarado que el front NO reconoce. NO se descarta — se muestra
    // como badge `warn` con el string crudo SANITIZADO, así el usuario ve qué pidió.
    // Dedup por valor crudo (un permiso repetido produce un solo badge).
    const raw = sanitizePerm(p);
    if (raw && !seenOther.has(raw)) {
      seenOther.add(raw);
      others.push({
        key: `other:${raw}`,
        icon: "⚠️",
        label: `Permiso adicional · ${raw}`,
        title: `El plugin declara el permiso «${raw}», que Furx no reconoce explícitamente. Se muestra crudo para transparencia: revisá el manifest antes de activar.`,
        tone: "warn",
      });
    }
  }

  return [
    ...secrets,
    ...nets,
    ...(shell ? [shell] : []),
    ...(fsWrite ? [fsWrite] : []),
    ...others,
  ];
}

/** ¿el plugin declara algún grant sensible (BYOK/red/shell/archivos)? */
export function requiresGrant(permissions: string[]): boolean {
  return grantBadges(permissions).length > 0;
}

/** Resultado de evaluar si un plugin puede activarse en un perfil. */
export interface Activatability {
  /** ¿se puede togglear/activar en este perfil? */
  activatable: boolean;
  /** razón legible (español) cuando NO se puede activar (para mostrar deshabilitado). */
  reason: string | null;
}

/**
 * Un plugin se puede activar por perfil sólo si está GLOBALMENTE habilitado (P0a) y su
 * firma es válida (fail-closed). Respeta el gobierno global: no se puede "saltar" el
 * disable global desde el editor de perfil.
 */
export function canActivate(plugin: Pick<InstalledPlugin, "enabled" | "verified">): Activatability {
  if (!plugin.verified) {
    return { activatable: false, reason: "Sin firma válida — el motor rehúsa ejecutarlo." };
  }
  if (!plugin.enabled) {
    return { activatable: false, reason: "Desactivado globalmente — activalo en Plugins primero." };
  }
  return { activatable: true, reason: null };
}

/** ¿el plugin está activo en este perfil? (presente en el array de slugs). */
export function isPluginActive(profilePlugins: string[] | undefined | null, name: string): boolean {
  return (profilePlugins ?? []).includes(name);
}

/**
 * Togglea el `name` dentro del array de plugins del perfil de forma INMUTABLE y
 * deduplicada. Si está → lo saca; si no está → lo agrega (sin duplicar). Conserva el
 * orden relativo del resto y agrega al final. Nunca muta el array de entrada.
 */
export function togglePlugin(profilePlugins: string[] | undefined | null, name: string): string[] {
  const current = profilePlugins ?? [];
  if (current.includes(name)) {
    return current.filter((p) => p !== name);
  }
  // dedup defensivo (por si el array de entrada ya traía duplicados) + append.
  return [...new Set([...current, name])];
}

/**
 * Filtra el array de plugins a persistir dejando SÓLO los que existen en disco
 * (`installedNames`). Defensivo: nunca escribimos en `agent_profile.plugins` un nombre
 * arbitrario que no esté en `plugins_list` (disco = SoT). Preserva orden y dedup.
 * Si `installedNames` está vacío y `next` no, NO se filtra a [] silenciosamente: se
 * devuelve la intersección (que será [] sólo si de verdad ninguno existe).
 */
export function validatePlugins(next: string[], installedNames: Iterable<string>): string[] {
  const known = new Set(installedNames);
  const seen = new Set<string>();
  const out: string[] = [];
  for (const name of next) {
    if (known.has(name) && !seen.has(name)) {
      seen.add(name);
      out.push(name);
    }
  }
  return out;
}

/** Helper interno: si `s` empieza con `prefix`, devuelve el resto; si no, null. */
function matchPrefix(s: string, prefix: string): string | null {
  if (s.startsWith(prefix)) return s.slice(prefix.length);
  return null;
}

/**
 * Sanitiza un string de permiso crudo para mostrarlo SIN romper la UI: quita caracteres
 * de control, colapsa espacios y trunca a 80 chars. No intenta validar el permiso —sólo
 * lo hace seguro de renderizar como texto (React ya escapa HTML, esto evita ruido visual).
 */
function sanitizePerm(s: string): string {
  // eslint-disable-next-line no-control-regex
  const cleaned = s.replace(/[\x00-]+/g, " ").replace(/\s+/g, " ").trim();
  return cleaned.length > 80 ? `${cleaned.slice(0, 79)}…` : cleaned;
}
