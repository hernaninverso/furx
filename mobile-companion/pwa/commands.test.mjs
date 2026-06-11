// 017 T045/T041 — command surface: filter/pagination + approval classification.
// Pure logic only (no DOM). Run: `node commands.test.mjs`.
import { setCatalog, catalogSize, visibleCommands, needsApproval, __reset } from "./commands.js";

let pass = 0, fail = 0;
const ok = (c, n) => (c ? pass++ : (fail++, console.log(`FAIL ${n}`)));

__reset();
const cat = [
  { id: "list_monitors", label: "List monitors", category: "state", risk: "safe" },
  { id: "reset_furx", label: "Reset furx", category: "system", risk: "destructive" },
  { id: "mobile_secret_get", label: "Mobile secret get", category: "mobile", risk: "credential" },
  { id: "check_updates", label: "Check updates", category: "system", risk: "external" },
];
setCatalog(cat);
ok(catalogSize() === 4, "catalog size 4");

// 1) empty query → all, capped by limit.
let r = visibleCommands(cat, "", 100);
ok(r.total === 4 && r.rows.length === 4, "no filter → all 4");

// 2) pagination: limit slices but total counts the full match set.
r = visibleCommands(cat, "", 2);
ok(r.total === 4 && r.rows.length === 2, "limit 2 → 2 rows, total 4 (edge: don't render all)");

// 3) filter by label/id/category (case-insensitive).
ok(visibleCommands(cat, "reset", 100).rows.some((c) => c.id === "reset_furx"), "filter by label");
ok(visibleCommands(cat, "MOBILE", 100).rows.some((c) => c.id === "mobile_secret_get"), "filter by category, case-insensitive");
ok(visibleCommands(cat, "check_updates", 100).rows.length === 1, "filter by id");
ok(visibleCommands(cat, "zzzznope", 100).total === 0, "no match → 0");

// 4) approval classification: destructive + credential need approval; safe/external don't.
ok(needsApproval({ risk: "destructive" }) === true, "destructive needs approval");
ok(needsApproval({ risk: "credential" }) === true, "credential needs approval");
ok(needsApproval({ risk: "safe" }) === false, "safe no approval");
ok(needsApproval({ risk: "external" }) === false, "external no approval");

console.log(`commands: ${pass} passed, ${fail} failed`);
process.exit(fail === 0 ? 0 : 1);
