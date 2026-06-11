// web/src/lib/navIcons.tsx — 055 wedge · iconos SVG (lucide-react) para el sidebar.
//
// Decisión de diseño (consultada a codex): opción híbrida con `lucide-react` — iconos SVG + texto en
// el sidebar (trazos consistentes, nítidos, escalables), barra superior textual. Reemplaza los glifos
// unicode (▤ ⊞ ⌬ ▦…) que codex marcó como ambiguos/inconsistentes entre sí. El glifo `icon` de
// NAV_GROUPS queda como FALLBACK estructural para la serialización móvil (string); el desktop usa esto.
import {
  PanelsTopLeft,
  ListOrdered,
  Brain,
  Search,
  Activity,
  Settings,
  Circle,
  type LucideIcon,
} from "lucide-react";
import type { View } from "./router";

/// Mapa view → icono lucide. Metáforas recomendadas por codex para los 6 ítems de la espina. Las
/// vistas fuera de la espina (modo flat de rollback) caen al `Circle` neutro.
const ICONS: Partial<Record<View, LucideIcon>> = {
  panes: PanelsTopLeft, // Sesiones
  queue: ListOrdered, // Cola
  memory: Brain, // Memoria (diferencial)
  search: Search, // Buscar
  activity: Activity, // 058 (ultrareview fix): la espina usa `activity` (Action Center), no `monitors`
  monitors: Activity, // vista de detalle detrás de "Actividad" (deep-link / ⌘K)
  settings: Settings, // Ajustes
};

/// Icono SVG del nav para una vista. Acepta `string` (SidebarGroups es genérico sobre el tipo de
/// vista); una vista desconocida cae a `Circle`. `aria-hidden` porque siempre va con el label de
/// texto. 16px + stroke 1.75 alineado con la tipografía del sidebar.
export function NavIcon({ view }: { view: string }) {
  const Icon = ICONS[view as View] ?? Circle;
  return <Icon size={16} strokeWidth={1.75} aria-hidden="true" />;
}
