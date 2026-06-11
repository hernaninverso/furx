// 019 F2 T020 — tests de la lógica PURA de filtrado del kit (`lib/kit/filter.ts`).
// Convención del repo: Node `--experimental-strip-types` (no vitest), assert a mano, `npm test`.
import {
  matchesQuery, matchesFacets, applyFilter, facetCounts, emptyFilter,
} from "../kit/filter.ts";

let pass = 0, fail = 0;
function ok(cond: boolean, name: string) { if (cond) pass++; else { fail++; console.log(`FAIL ${name}`); } }

// matchesQuery: AND de tokens, OR de campos, case-insensitive, trim.
ok(matchesQuery("", ["lo que sea"]), "query vacía matchea todo");
ok(matchesQuery("  ", ["x"]), "query whitespace matchea todo");
ok(matchesQuery("foo", ["the FOO bar"]), "case-insensitive");
ok(matchesQuery("foo bar", ["foo here", "bar there"]), "tokens AND across fields");
ok(!matchesQuery("foo zzz", ["foo here", "bar there"]), "token ausente → no matchea");
ok(!matchesQuery("x", [null, undefined, ""]), "campos vacíos no matchean texto");

// matchesFacets: faceta null = todos.
ok(matchesFacets({ state: null }, () => "running"), "faceta null pasa cualquiera");
ok(matchesFacets({ state: "running" }, () => "running"), "faceta igual pasa");
ok(!matchesFacets({ state: "failed" }, () => "running"), "faceta distinta no pasa");
ok(matchesFacets({}, () => undefined), "sin facetas pasa");
ok(!matchesFacets({ state: "x" }, () => undefined), "faceta pedida vs item sin valor → no pasa");

// applyFilter end-to-end.
type Row = { id: string; title: string; state: string };
const rows: Row[] = [
  { id: "1", title: "fix login", state: "failed" },
  { id: "2", title: "add logout", state: "done" },
  { id: "3", title: "refactor auth", state: "failed" },
];
const acc = {
  text: (r: Row) => [r.title],
  facet: (r: Row, f: string) => (f === "state" ? r.state : undefined),
};
ok(applyFilter(rows, emptyFilter(), acc).length === 3, "filtro vacío devuelve todo");
ok(applyFilter(rows, { query: "log", facets: {} }, acc).map((r) => r.id).join() === "1,2", "texto 'log' → login+logout");
ok(applyFilter(rows, { query: "", facets: { state: "failed" } }, acc).map((r) => r.id).join() === "1,3", "faceta failed → 1,3");
ok(applyFilter(rows, { query: "auth", facets: { state: "failed" } }, acc).map((r) => r.id).join() === "3", "texto+faceta combinados");
// inmutabilidad: no muta la entrada.
const before = rows.slice();
applyFilter(rows, { query: "x", facets: {} }, acc);
ok(JSON.stringify(before) === JSON.stringify(rows), "applyFilter no muta la lista original");

// facetCounts.
const counts = facetCounts(rows, "state", acc.facet);
ok(counts.failed === 2 && counts.done === 1, "facetCounts agrupa bien");

console.log(`kitFilter: ${pass} passed, ${fail} failed`);
process.exit(fail === 0 ? 0 : 1);
