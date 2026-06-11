// web/src/lib/responsive.ts — 015 T023 · viewport/breakpoints como función PURA (testeable sin
// browser). El "multi-viewport" (incl. móvil como 2º viewport) se testea sobre esta lógica pura,
// no sobre render real (no hay browser-runner). El Shell puede consumir `layoutForWidth` para
// decidir clases/columnas según el ancho.

export type Viewport = "mobile" | "tablet" | "desktop";

export interface Layout {
  viewport: Viewport;
  /// ¿sidebar visible por defecto? (en móvil se colapsa).
  sidebarVisible: boolean;
  /// columnas sugeridas para la grilla de paneles.
  paneColumns: number;
}

/// Umbrales (ancho mínimo de cada viewport).
export const BREAKPOINTS = { tablet: 768, desktop: 1200 } as const;

/// Mapea un ancho (px) a un layout. Puro y total (cualquier número → un Layout válido).
export function layoutForWidth(width: number): Layout {
  if (width < BREAKPOINTS.tablet) return { viewport: "mobile", sidebarVisible: false, paneColumns: 1 };
  if (width < BREAKPOINTS.desktop) return { viewport: "tablet", sidebarVisible: true, paneColumns: 1 };
  return { viewport: "desktop", sidebarVisible: true, paneColumns: 2 };
}
