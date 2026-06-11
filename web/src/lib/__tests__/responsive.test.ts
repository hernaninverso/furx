// 015 T023 — multi-viewport como lógica pura. `npm test` (vía test-all.mjs).
import { layoutForWidth, BREAKPOINTS } from "../responsive.ts";

let pass = 0, fail = 0;
function ok(c: boolean, n: string) { if (c) pass++; else { fail++; console.log(`FAIL ${n}`); } }

// móvil (2º viewport): sidebar colapsada, 1 columna.
const m = layoutForWidth(375);
ok(m.viewport === "mobile" && !m.sidebarVisible && m.paneColumns === 1, "375 → mobile");
// tablet: sidebar visible, 1 columna.
const t = layoutForWidth(900);
ok(t.viewport === "tablet" && t.sidebarVisible && t.paneColumns === 1, "900 → tablet");
// desktop: 2 columnas.
const d = layoutForWidth(1440);
ok(d.viewport === "desktop" && d.paneColumns === 2, "1440 → desktop");
// bordes de breakpoint exactos.
ok(layoutForWidth(BREAKPOINTS.tablet).viewport === "tablet", "768 = tablet (borde)");
ok(layoutForWidth(BREAKPOINTS.desktop).viewport === "desktop", "1200 = desktop (borde)");
ok(layoutForWidth(0).viewport === "mobile", "0 → mobile");

console.log(`responsive: ${pass} passed, ${fail} failed`);
process.exit(fail === 0 ? 0 : 1);
