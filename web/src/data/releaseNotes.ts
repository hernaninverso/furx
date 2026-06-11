// web/src/data/releaseNotes.ts — 016 US3 (T030) · release notes versionadas (DATOS).
//
// Cada entrada: {version, title, description, deeplink?, date}. El parser semver real de whatsNew.ts
// ordena/filtra estas por versión (NO lexicográfico). Mantener ordenado conceptualmente por versión;
// el filtro/orden lo hace whatsNew.ts. Los `deeplink` usan el router interno (`furx://…`).
//
// El copy cumple el constraint F-III (palabra prohibida). title/description son curados (no i18n por ahora — son
// datos editoriales por release; el chrome de la UI sí va por t()).

export interface ReleaseNote {
  /// versión SemVer en la que apareció el cambio (ej "0.2.0", "1.0.0-beta.1").
  version: string;
  title: string;
  description: string;
  /// deeplink opcional a la feature (furx://<view> | furx://settings/<sec> | furx://modal/<name>).
  deeplink?: string;
  /// fecha ISO (informativa).
  date: string;
}

export const RELEASE_NOTES: ReleaseNote[] = [
  {
    version: "0.1.0",
    title: "Primer arranque de Furx",
    description: "Paneles de terminal, BYOK (tus claves nunca salen del equipo) y el Council multi-modelo.",
    date: "2026-05-28",
  },
  {
    version: "0.2.0",
    title: "Navegación por dominios + paleta de comandos",
    description: "La barra lateral agrupa las vistas en 6 dominios y ⌘K abre la paleta universal de comandos.",
    deeplink: "furx://settings/advanced",
    date: "2026-05-30",
  },
  {
    version: "0.3.0",
    title: "Centro de ayuda, novedades, tours e idioma",
    description: "Ayuda contextual buscable, este panel de novedades, un tour de primeros pasos y selector de idioma (ES/EN).",
    deeplink: "furx://help",
    date: "2026-05-30",
  },
];
