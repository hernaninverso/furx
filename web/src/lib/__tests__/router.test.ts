// 015 T013 (US9) — tests del parser del router interno. Runnable con `node` (type-stripping
// nativo de Node 23+): `npm run test:router` (o `node src/lib/__tests__/router.test.ts`).
// Está en __tests__/ (excluido de `tsc -b`/vite build), así que no entra al bundle.
import { parseRoute, parseModalRoute, parseHelpRoute, isWhatsNewRoute, buildRoute, buildHelpRoute, navTargets, VIEWS, SETTINGS_SECTIONS, MODALS } from "../router.ts";

let pass = 0, fail = 0;
function eq(actual: unknown, expected: unknown, name: string) {
  const a = JSON.stringify(actual), e = JSON.stringify(expected);
  if (a === e) pass++;
  else { fail++; console.log(`FAIL ${name}: got ${a} want ${e}`); }
}

// Vista top-level.
eq(parseRoute("furx://queue"), { view: "queue" }, "view queue");
eq(parseRoute("furx://settings"), { view: "settings" }, "settings top");
// Sub de Settings válido.
eq(parseRoute("furx://settings/mobile"), { view: "settings", section: "mobile" }, "settings/mobile");
// Sub de Settings inválido → FAIL-SOFT (invalidSection, NO null): no varar al usuario.
eq(parseRoute("furx://settings/audio"), { view: "settings", invalidSection: "audio" }, "settings/audio fail-soft");
// Vista desconocida → null (error de programación).
eq(parseRoute("furx://nope"), null, "unknown view null");
// No-furx / vacío / sólo esquema / no-string → null.
eq(parseRoute("https://x"), null, "non-furx null");
eq(parseRoute(""), null, "empty null");
eq(parseRoute("furx://"), null, "scheme only null");
eq(parseRoute(123), null, "non-string null");
// Sub en vista no-settings → fail-soft.
eq(parseRoute("furx://queue/foo"), { view: "queue", invalidSection: "foo" }, "non-settings sub fail-soft");
// Trailing slashes tolerados.
eq(parseRoute("furx://settings/mobile/"), { view: "settings", section: "mobile" }, "trailing slash");
// buildRoute + roundtrip.
eq(buildRoute("queue"), "furx://queue", "buildRoute view");
eq(buildRoute("settings", "mobile"), "furx://settings/mobile", "buildRoute section");
eq(parseRoute(buildRoute("memory")), { view: "memory" }, "roundtrip");
// 015 T031 — modal routes: furx://modal/<name> parsea como modal (no como vista).
eq(parseModalRoute("furx://modal/council"), "council", "modal council");
eq(parseModalRoute("furx://modal/nope"), null, "modal desconocido null");
eq(parseModalRoute("furx://settings"), null, "no-modal route null");
eq(parseRoute("furx://modal/council"), null, "parseRoute NO matchea un modal (vista-only)");

// 016 US2/US3 — rutas de Help y What's New (overlays, NO vistas).
eq(parseHelpRoute("furx://help"), {}, "help top");
eq(parseHelpRoute("furx://help/"), {}, "help top trailing slash");
eq(parseHelpRoute("furx://help/audio"), { section: "audio" }, "help/audio sección");
eq(parseHelpRoute("furx://help/Inteligencia"), { section: "Inteligencia" }, "help/dominio sección");
eq(parseHelpRoute("furx://settings"), null, "no-help route null");
eq(parseHelpRoute(123), null, "help non-string null");
eq(parseRoute("furx://help/audio"), null, "parseRoute NO matchea help (overlay-only)");
eq(buildHelpRoute(), "furx://help", "buildHelpRoute top");
eq(buildHelpRoute("audio"), "furx://help/audio", "buildHelpRoute sección");
eq(isWhatsNewRoute("furx://whatsnew"), true, "whatsnew route");
eq(isWhatsNewRoute("furx://whatsnew/"), true, "whatsnew trailing slash");
eq(isWhatsNewRoute("furx://whatsnew/x"), false, "whatsnew con sub → false");
eq(isWhatsNewRoute("furx://settings"), false, "no-whatsnew route false");
eq(parseRoute("furx://whatsnew"), null, "parseRoute NO matchea whatsnew (overlay-only)");

// navTargets: vistas + secciones + modales + Help + What's New, todas parseables.
const targets = navTargets();
eq(targets.length, VIEWS.length + SETTINGS_SECTIONS.length + MODALS.length + 2, "navTargets count (+help +whatsnew)");
eq(
  targets.every(
    (t) =>
      parseRoute(t.route) !== null ||
      parseModalRoute(t.route) !== null ||
      parseHelpRoute(t.route) !== null ||
      isWhatsNewRoute(t.route),
  ),
  true,
  "all navTargets parse (view, modal, help o whatsnew)",
);

console.log(`router: ${pass} passed, ${fail} failed`);
process.exit(fail === 0 ? 0 : 1);
