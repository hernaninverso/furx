// 019 F2 T020 — kit de controles reusables, DESTILADO del flujo F1 (no especulativo).
//
// Regla de oro aplicada: un componente entra al kit SÓLO si su patrón ya aparece en ≥2-3 superficies
// del front (≥80% reutilizable). Lo que no llega, queda documentado en backlog — NO se inventa.
//
// INCLUIDOS (evidencia de reuso real):
//   • FilterBar      — AuditDrawer (filtra kind/actor), CommandPalette015 (texto+categoría),
//                      CouncilModal/AgentGallery (subconjuntos por campo).            → ≥3 lugares.
//   • BulkActionBar  — BroadcastModal (Set<string>+aplicar a todos), OrchestrationBoard
//                      (descartar no-elegidas), BestOfNCompare (descartar variantes). → ≥3 lugares.
//   • InlineEdit     — OrchestrationBoard (title/objective), AgentGallery (drafts),
//                      SettingsRegistryPanel (valores).                                → ≥3 lugares.
//   • DrillDown      — DataViewer (árbol JSON <details>), OrchestrationBoard (log-history),
//                      AuditDrawer (row→detalle).                                      → ≥3 lugares.
//   • FormFromSchema — contrato T021 (anti command-injection: rendering/validation/policy
//                      separados). Aunque hoy hay ~1-2 forms schema-driven, es requisito de
//                      seguridad explícito, NO especulación → se construye.
//
// BACKLOG (NO construidos por YAGNI — < ≥80% de reuso hoy):
//   • Sparkline      — CERO usos en el front actual (0 <svg>/polyline/chart en components/). Ningún
//                      patrón real que destilar. Se difiere hasta que UsageSummary/telemetry o
//                      SignalsPanel pidan una serie temporal mini. Cuando exista ≥2 call-sites,
//                      extraer una `<Sparkline data:number[]>` con lógica pura de escalado en
//                      `lib/kit/sparkline.ts` (min/max/normalize). Mientras tanto, NO se incluye.
//   • DateRangePicker / Pagination — tampoco aparecen ≥2 veces hoy; mismo criterio, backlog.
//
// ─────────────────────────────────────────────────────────────────────────────────────────────────
// INVARIANTE DE GOBIERNO (audit ronda 2 H3 — LEER ANTES DE CABLEAR EL KIT)
// ─────────────────────────────────────────────────────────────────────────────────────────────────
// Los componentes de este kit son BUILDING BLOCKS DE PRESENTACIÓN puros y reusables. Exponen
// callbacks genéricos (BulkActionBar.onRun, InlineEdit.onCommit, etc.) a propósito — la reusabilidad
// es deliberada. PERO eso traslada una responsabilidad CRÍTICA al consumidor:
//
//   NINGUNA acción de MUTACIÓN debe ejecutarse desde un callback del kit por fuera del `invoke`
//   gobernado (`web/src/lib/invoke.ts` → pending_approval → approvalBus → audit). El kit NO ejecuta
//   ni gobierna nada: sólo dispara INTENCIÓN. La garantía de que las mutaciones pasan por el gate
//   universal vive en el WIRING del consumidor, no en el kit.
//
// Patrón de referencia: `web/src/components/QueuePanel.tsx` — cada handler de mutación (bulk-cancel,
// bulk-retry, inline-rename) va por `invoke(...)` gateado; el inline-rename incluso queda
// DESHABILITADO si el Shell no aporta un command id REAL del registry, para nunca exponer una
// mutación no-gobernada.
//
// Regla al agregar/cablear un componente del kit: si su callback dispara una mutación, cablealo a
// `invoke` gobernado (NUNCA a un fetch/Tauri-invoke/red directo que bypasee el gate + audit).

export { FilterBar } from "./FilterBar";
export { BulkActionBar, type BulkAction } from "./BulkActionBar";
export { InlineEdit } from "./InlineEdit";
export { DrillDown, type DrillRow } from "./DrillDown";
export { FormFromSchema } from "./FormFromSchema";
