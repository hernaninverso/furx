// 022 P1 · US6 — tests de la lógica PURA del inbox de incidentes. `node --experimental-strip-types`.
// Cubre: estado de inbox derivado, filtro accionable, snooze_until (1h/4h/mañana), auto-unsnooze
// (vía reopened), agrupación y mapeo fuente→destino para "ir al origen".
import {
  inboxState,
  isActionable,
  isSnoozed,
  inboxCards,
  computeSnoozeUntil,
  groupIncidents,
  sourceTarget,
  hasNavigableSource,
  SNOOZE_OPTIONS,
  groupHasCritical,
  initialCollapsedState,
  loadCollapsedState,
  saveCollapsedState,
  INCIDENT_GROUPS_COLLAPSED_KEY,
  INCIDENT_GROUP_INITIAL_VISIBLE,
  INCIDENT_GROUP_DOM_CAP,
  // 050 FR-004 — modo compacto persistido.
  loadCompactIncidents,
  saveCompactIncidents,
  INCIDENT_COMPACT_KEY,
} from "../incidents.ts";
import type { Card } from "../../types.ts";

// 044 FR-002 — el node-runner L1 no trae `localStorage`; `boot.ts` lo usa en load/save de colapso.
// Instalamos un shim en memoria (boot.ts sólo lo TOCA al llamarse, no al importar → seguro acá).
{
  const store = new Map<string, string>();
  (globalThis as { localStorage?: unknown }).localStorage = {
    get length() { return store.size; },
    clear() { store.clear(); },
    getItem(k: string) { return store.has(k) ? store.get(k)! : null; },
    key(i: number) { return Array.from(store.keys())[i] ?? null; },
    removeItem(k: string) { store.delete(k); },
    setItem(k: string, v: string) { store.set(k, String(v)); },
  };
}

let pass = 0, fail = 0;
function ok(cond: boolean, name: string) { if (cond) pass++; else { fail++; console.log(`FAIL ${name}`); } }

const NOW = "2026-06-01 12:00:00";
function card(p: Partial<Card>): Card {
  return {
    id: p.id ?? "c1",
    created_at: p.created_at ?? "2026-06-01 10:00:00",
    project: p.project ?? "furx",
    source: p.source ?? "monitor",
    title: p.title ?? "Algo pasó",
    severity: p.severity ?? "warning",
    status: p.status ?? "open",
    cause: p.cause,
    snooze_until: p.snooze_until,
    read_at: p.read_at,
    dismissed_at: p.dismissed_at,
    last_activity_at: p.last_activity_at,
    reopened: p.reopened,
  };
}

// ── inboxState ─────────────────────────────────────────────────────────────────────────────────
ok(inboxState(card({}), NOW) === "actionable", "open sin flags → actionable");
ok(inboxState(card({ status: "closed" }), NOW) === "closed", "status closed → closed");
ok(inboxState(card({ dismissed_at: "2026-06-01 11:00:00" }), NOW) === "dismissed", "dismissed_at → dismissed");
ok(inboxState(card({ snooze_until: "2026-06-01 18:00:00" }), NOW) === "snoozed", "snooze futuro → snoozed");
ok(inboxState(card({ snooze_until: "2026-06-01 09:00:00" }), NOW) === "actionable", "snooze pasado → actionable (expiró)");
ok(inboxState(card({ read_at: "2026-06-01 11:30:00" }), NOW) === "read", "read_at → read");

// auto-unsnooze por nueva actividad: reopened=1 anula el snooze futuro.
ok(
  inboxState(card({ snooze_until: "2026-06-01 18:00:00", reopened: true }), NOW) === "actionable",
  "reopened anula snooze futuro → actionable (auto-unsnooze)",
);

// ── isActionable / isSnoozed ─────────────────────────────────────────────────────────────────────
ok(isActionable(card({}), NOW), "actionable cuenta como accionable");
ok(isActionable(card({ read_at: "2026-06-01 11:30:00" }), NOW), "read cuenta como accionable (visible)");
ok(!isActionable(card({ dismissed_at: "2026-06-01 11:00:00" }), NOW), "dismissed NO es accionable");
ok(isSnoozed(card({ snooze_until: "2026-06-01 18:00:00" }), NOW), "snooze futuro → isSnoozed");
ok(!isSnoozed(card({ snooze_until: "2026-06-01 18:00:00", reopened: true }), NOW), "reabierta NO está snoozeada");

// ── inboxCards (filtro) ──────────────────────────────────────────────────────────────────────────
const set: Card[] = [
  card({ id: "a" }),                                                  // actionable
  card({ id: "b", read_at: "2026-06-01 11:30:00" }),                 // read
  card({ id: "c", snooze_until: "2026-06-01 18:00:00" }),            // snoozed
  card({ id: "d", dismissed_at: "2026-06-01 11:00:00" }),            // dismissed
  card({ id: "e", status: "closed" }),                                // closed
  card({ id: "f", snooze_until: "2026-06-01 18:00:00", reopened: true }), // reabierta
];
const visible = inboxCards(set, NOW);
ok(visible.map((c) => c.id).sort().join() === "a,b,f", "inbox visible = actionable+read+reabierta (sin snoozed/dismissed/closed)");
const strict = inboxCards(set, NOW, true);
ok(strict.map((c) => c.id).sort().join() === "a,f", "onlyActionable excluye las 'read'");

// ── computeSnoozeUntil ───────────────────────────────────────────────────────────────────────────
const base = Date.UTC(2026, 5, 1, 12, 0, 0); // 2026-06-01T12:00:00Z
ok(computeSnoozeUntil("1h", base) === "2026-06-01 13:00:00", "snooze 1h = +1 hora UTC");
ok(computeSnoozeUntil("4h", base) === "2026-06-01 16:00:00", "snooze 4h = +4 horas UTC");
ok(SNOOZE_OPTIONS.length === 3, "3 opciones de snooze (1h/4h/tomorrow), no fijo");
// tomorrow: 09:00 local del día siguiente, almacenado en UTC (comparable con datetime('now')).
// La hora UTC depende del offset local de la máquina, así que NO pineamos la hora: validamos formato
// SQLite, que es estrictamente futuro, y que cae dentro de las próximas ~48h (mañana, no lejano).
const tomorrow = computeSnoozeUntil("tomorrow", base);
ok(/^\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}$/.test(tomorrow), "tomorrow tiene formato SQLite (YYYY-MM-DD HH:MM:SS)");
ok(tomorrow > "2026-06-01 12:00:00", "tomorrow es estrictamente futuro");
ok(tomorrow < "2026-06-03 12:00:00", "tomorrow cae dentro de las próximas ~48h (mañana, no lejano)");

// ── groupIncidents ───────────────────────────────────────────────────────────────────────────────
const toGroup: Card[] = [
  card({ id: "g1", project: "furx", severity: "critical" }),
  card({ id: "g2", project: "furx", severity: "info" }),
  card({ id: "g3", project: "toga", severity: "warning" }),
];
const byProject = groupIncidents(toGroup, "project", NOW);
ok(byProject.length === 2, "agrupa por proyecto → 2 grupos");
ok(byProject[0].key === "furx" && byProject[0].cards.length === 2, "grupo furx con 2 (más accionables primero)");
ok(byProject[0].cards[0].severity === "critical", "dentro del grupo, critical antes que info");
const bySev = groupIncidents(toGroup, "severity", NOW);
ok(bySev.map((g) => g.key).join() === "critical,warning,info", "grupos por severidad ordenados crit→warn→info");
ok(bySev[0].actionableCount === 1, "actionableCount del grupo critical = 1");

// ── sourceTarget / hasNavigableSource ───────────────────────────────────────────────────────────
ok(sourceTarget(card({ source: "monitor" })).view === "monitors", "monitor → vista monitors");
ok(sourceTarget(card({ source: "monitor" })).drilldown === "monitors-down", "monitor → drilldown caídos");
ok(sourceTarget(card({ source: "worktree" })).view === "panes", "worktree → vista panes");
ok(sourceTarget(card({ source: "merge" })).view === "panes", "merge → vista panes");
ok(sourceTarget(card({ source: "ci" })).view === null, "ci (sin vista canónica) → null (slide-over)");
ok(hasNavigableSource(card({ source: "monitor" })), "monitor tiene origen navegable");
ok(!hasNavigableSource(card({ source: "doctrine" })), "doctrine NO tiene origen navegable → slide-over");

// ── 044 FR-002 — colapso/expansión de grupos ─────────────────────────────────────────────────────
{
  const grps = [
    { key: "crit-grp", cards: [card({ severity: "critical" }), card({ severity: "info" })], actionableCount: 2 },
    { key: "calm-grp", cards: [card({ severity: "warning" }), card({ severity: "info" })], actionableCount: 1 },
  ];
  ok(groupHasCritical(grps[0]), "groupHasCritical: grupo con una critical → true");
  ok(!groupHasCritical(grps[1]), "groupHasCritical: grupo sin critical → false");

  // primer arranque (persisted=null): el grupo con critical arranca EXPANDIDO (collapsed=false),
  // el resto COLAPSADO (collapsed=true).
  const first = initialCollapsedState(grps, null);
  ok(first["crit-grp"] === false, "primer arranque: grupo critical expandido (collapsed=false)");
  ok(first["calm-grp"] === true, "primer arranque: grupo sin critical colapsado (collapsed=true)");

  // lo persistido por el usuario MANDA sobre el default de primer arranque.
  const restored = initialCollapsedState(grps, { "crit-grp": true, "calm-grp": false });
  ok(restored["crit-grp"] === true, "persistido manda: critical queda colapsado si el user lo colapsó");
  ok(restored["calm-grp"] === false, "persistido manda: calm queda expandido si el user lo expandió");

  // un grupo nuevo (no presente en lo persistido) cae al default de primer arranque.
  const mixed = initialCollapsedState(grps, { "crit-grp": true });
  ok(mixed["crit-grp"] === true, "grupo persistido conserva su valor");
  ok(mixed["calm-grp"] === true, "grupo nuevo (sin persistir, sin critical) → colapsado por default");

  // round-trip de persistencia.
  try { localStorage.removeItem(INCIDENT_GROUPS_COLLAPSED_KEY); } catch { /* ignore */ }
  ok(loadCollapsedState() === null, "loadCollapsedState sin nada guardado → null");
  ok(saveCollapsedState({ a: true, b: false }) === true, "saveCollapsedState persiste y verifica");
  const loaded = loadCollapsedState();
  ok(loaded !== null && loaded.a === true && loaded.b === false, "loadCollapsedState devuelve lo guardado");
  // JSON inválido / forma rara → null (nunca tira).
  try { localStorage.setItem(INCIDENT_GROUPS_COLLAPSED_KEY, "{not json"); } catch { /* ignore */ }
  ok(loadCollapsedState() === null, "JSON inválido → null (no tira)");
  try { localStorage.setItem(INCIDENT_GROUPS_COLLAPSED_KEY, JSON.stringify(["array"])); } catch { /* ignore */ }
  ok(loadCollapsedState() === null, "array (no objeto) → null");
  // valores no-boolean se descartan (sólo boolean sobrevive).
  try { localStorage.setItem(INCIDENT_GROUPS_COLLAPSED_KEY, JSON.stringify({ a: true, b: "nope", c: 1 })); } catch { /* ignore */ }
  const filtered = loadCollapsedState();
  ok(filtered !== null && filtered.a === true && !("b" in filtered) && !("c" in filtered), "descarta valores no-boolean");

  ok(INCIDENT_GROUP_INITIAL_VISIBLE === 5, "primeras 5 visibles por grupo");
  ok(INCIDENT_GROUP_DOM_CAP === 200, "cap 200 en DOM");
}

// 050 FR-004 — modo compacto de incidentes (persistencia opt-in, default OFF → cero regresión).
{
  try { localStorage.removeItem(INCIDENT_COMPACT_KEY); } catch { /* ignore */ }
  ok(loadCompactIncidents() === false, "modo compacto default OFF (sin nada guardado)");
  ok(saveCompactIncidents(true) === true, "saveCompactIncidents(true) persiste y verifica");
  ok(loadCompactIncidents() === true, "tras guardar ON, load devuelve true");
  ok(saveCompactIncidents(false) === true, "saveCompactIncidents(false) persiste");
  ok(loadCompactIncidents() === false, "tras guardar OFF, load devuelve false");
}

console.log(`incidents: ${pass} passed, ${fail} failed`);
process.exit(fail === 0 ? 0 : 1);
