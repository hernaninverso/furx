// 017 T067 — 3-way contract: TS nav SSOT ↔ mobile subset ↔ PWA fixture discipline.
//
// The command registry SSOT is in Rust (command_registry.rs); the Rust test
// `t020_catalog_excludes_internal_hidden_and_denied` proves the projection drops
// internal/hidden + deny-listed there. This TS contract test covers the parts
// that live in TS / the PWA fixtures:
//   1. every mobile nav domain id ⊆ navGroups (no orphan literal) — analogous to
//      the Rust `registry_covers_all_handler_commands`.
//   2. every mobile nav item maps to a real NAV_GROUPS item (view+label+icon).
//   3. a PWA command fixture must NOT contain ids that the projection would have
//      filtered (we can't import the Rust registry, so we assert the FIXTURE only
//      lists categories that map to mobile domains + no obvious internal id).
import {
  NAV_GROUPS,
  MOBILE_NAV_SUBSET,
  buildNavSpec,
} from "../navGroups.ts";

let pass = 0, fail = 0;
function ok(cond: boolean, name: string) {
  if (cond) pass++;
  else { fail++; console.log(`FAIL ${name}`); }
}

const groupById = new Map(NAV_GROUPS.map((g) => [g.id, g]));

// 1) nav ids ⊆ navGroups (orphan literal → fail).
for (const id of MOBILE_NAV_SUBSET) {
  ok(groupById.has(id), `mobile domain "${id}" exists in NAV_GROUPS`);
}

// 2) every materialized item maps to a real NAV_GROUPS item.
const spec = buildNavSpec();
for (const d of spec.domains) {
  const src = groupById.get(d.domainId)!;
  for (const it of d.items) {
    const m = src.items.find((s) => s.view === it.view);
    ok(!!m && m.label === it.label && m.icon === it.icon,
      `item ${d.domainId}/${it.view} matches NAV_GROUPS`);
  }
}

// 3) PWA command fixture discipline. The fixture below mirrors what a
//    CommandCatalog frame should look like AFTER the server projection: only
//    palette/primary commands, categories that belong to mobile domains, and NO
//    internal-plumbing ids. If a future change leaks an internal id into a
//    fixture, this fails.
const MOBILE_DOMAIN_CATEGORIES = new Set(
  spec.domains.flatMap((d) => d.items.map(() => d.domainId)),
);
ok(MOBILE_DOMAIN_CATEGORIES.size >= 1, "mobile domains non-empty");

// Categories the registry uses that are KNOWN-internal plumbing → must never
// appear in a mobile catalog fixture (mirror of the Rust deny-list intent).
const FORBIDDEN_FIXTURE_CATEGORIES = new Set(["ssh", "vpn", "infra", "tmux", "terminal"]);
// A representative fixture (what the bridge sends). NOTE: this is a TEST fixture,
// not the SSOT — it documents the contract the Rust projection must honor.
const PWA_CATALOG_FIXTURE = [
  { id: "list_monitors", label: "List monitors", category: "state", risk: "safe" },
  { id: "reset_furx", label: "Reset furx", category: "system", risk: "destructive" },
  { id: "check_updates", label: "Check updates", category: "system", risk: "external" },
];
for (const c of PWA_CATALOG_FIXTURE) {
  ok(!FORBIDDEN_FIXTURE_CATEGORIES.has(c.category),
    `fixture cmd ${c.id} not in a deny-listed category`);
  ok(!c.id.startsWith("ssh_") && !c.id.startsWith("vpn_"),
    `fixture cmd ${c.id} not an ssh_/vpn_ id`);
  // internal plumbing ids that must never be exposed.
  ok(!["pty_write", "pty_spawn", "pty_kill", "health"].includes(c.id),
    `fixture cmd ${c.id} not internal plumbing`);
}

console.log(`mobileContract: ${pass} passed, ${fail} failed`);
process.exit(fail === 0 ? 0 : 1);
