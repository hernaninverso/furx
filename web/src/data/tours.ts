// web/src/data/tours.ts — 016 US4 (T040) · pasos del tour "primeros pasos" (DATOS).
//
// Recorre los dominios de navGroups resaltando regiones reales por `data-tour="<targetId>"`. Cada paso
// puede deep-linkear su vista antes de resaltar. El copy va por i18n (titleKey/bodyKey). Los targets
// `data-tour` deben existir en el DOM del Shell; si un paso no encuentra su target (vista no montada),
// la FSM lo saltea con fallback (no se cuelga). Los pasos cuyo `requiresFlag` esté OFF se filtran.

import type { TourStep } from "../lib/tour.ts";

export const FIRST_RUN_TOUR: TourStep[] = [
  {
    id: "welcome",
    targetId: "sidebar",
    domain: "Trabajo",
    titleKey: "tour.offer.title",
    bodyKey: "tour.offer.body",
  },
  {
    id: "commands",
    targetId: "topbar",
    domain: "Sistema",
    titleKey: "topbar.commands",
    bodyKey: "tour.offer.body",
  },
  {
    id: "help",
    targetId: "topbar-help",
    domain: "Sistema",
    deeplink: "furx://settings/advanced",
    titleKey: "topbar.help",
    bodyKey: "tour.offer.body",
    requiresFlag: "helpCenter",
  },
];
