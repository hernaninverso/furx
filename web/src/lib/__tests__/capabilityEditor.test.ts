// 022 US11 / FR-016 — tests de la lógica pura del editor de capabilities por perfil.
// Invariantes: toggle inmutable+dedup; badges de grant derivados de los permisos
// declarados (BYOK/red/shell), sin inventar; codebase-memory clasificada aparte; un
// plugin global-disabled o sin firma NO es activable. `node --experimental-strip-types`.
import {
  pluginKind,
  pluginKindLabel,
  grantBadges,
  requiresGrant,
  canActivate,
  isPluginActive,
  togglePlugin,
  validatePlugins,
  CODEBASE_MEMORY_PLUGIN,
} from "../capabilityEditor.ts";

let pass = 0, fail = 0;
function ok(cond: boolean, name: string) { if (cond) pass++; else { fail++; console.log(`FAIL ${name}`); } }

// ── pluginKind ────────────────────────────────────────────────────────────────
ok(pluginKind({ name: "codebase-memory", permissions: ["mcp"] }) === "codebase-memory", "kind: codebase-memory por nombre");
ok(CODEBASE_MEMORY_PLUGIN === "codebase-memory", "constante codebase-memory");
ok(pluginKind({ name: "codanna", permissions: ["mcp", "net: ninguno"] }) === "mcp", "kind: mcp por permiso");
ok(pluginKind({ name: "word-count", permissions: ["net: ninguno"] }) === "tool", "kind: tool sin mcp");
ok(pluginKind({ name: "x", permissions: [] }) === "tool", "kind: tool sin permisos");

// labels
ok(pluginKindLabel("codebase-memory") === "Memoria de código", "label codebase-memory");
ok(pluginKindLabel("mcp") === "Servidor MCP", "label mcp");
ok(pluginKindLabel("tool") === "Herramienta", "label tool");

// ── grantBadges ─────────────────────────────────────────────────────────────
{
  // "mcp" y "net: ninguno" NO son grants → no badges.
  const none = grantBadges(["mcp", "net: ninguno"]);
  ok(none.length === 0, "grants: mcp/net-ninguno no son grants");
  ok(requiresGrant(["mcp", "net: ninguno"]) === false, "requiresGrant false sin sensibles");
}
{
  // byok → badge danger con 🔑 y el nombre del secreto.
  const b = grantBadges(["byok: openai-api-key"]);
  ok(b.length === 1 && b[0].icon === "🔑" && b[0].tone === "danger", "grants: byok → 🔑 danger");
  ok(b[0].label.includes("Keychain") && b[0].label.includes("openai-api-key"), "grants: byok label con secreto");
  ok(requiresGrant(["byok: x"]) === true, "requiresGrant true con byok");
}
{
  // net: host → badge warn 🌐.
  const b = grantBadges(["net: api.github.com"]);
  ok(b.length === 1 && b[0].icon === "🌐" && b[0].tone === "warn", "grants: net → 🌐 warn");
  ok(b[0].label.includes("api.github.com"), "grants: net label con host");
}
{
  // shell → badge danger 📟.
  const b = grantBadges(["shell"]);
  ok(b.length === 1 && b[0].icon === "📟" && b[0].tone === "danger", "grants: shell → 📟 danger");
}
{
  // fs_write legacy → badge 📁 warn.
  const b = grantBadges(["fs_write"]);
  ok(b.length === 1 && b[0].icon === "📁" && b[0].tone === "warn", "grants: fs_write → 📁");
}
{
  // MED 1 — shell y fs_write son grants SEPARADOS: declarar ambos produce AMBOS badges
  // (uno no se come al otro). Antes compartían la misma variable y uno quedaba oculto.
  const b = grantBadges(["shell", "fs_write"]);
  ok(b.length === 2, "grants: shell+fs_write → 2 badges (no se ocultan)");
  ok(b.some((x) => x.key === "shell" && x.icon === "📟"), "grants: shell+fs_write incluye badge shell 📟");
  ok(b.some((x) => x.key === "fs-write" && x.icon === "📁"), "grants: shell+fs_write incluye badge fs-write 📁");
  // orden: shell antes que fs_write.
  ok(b[0].key === "shell" && b[1].key === "fs-write", "grants: orden shell→fs-write");
}
{
  // y combinado con secrets/red: orden global secrets→red→shell→fs-write, los 4 presentes.
  const b = grantBadges(["fs_write", "shell", "net: x.io", "byok: k"]);
  ok(b.length === 4, "grants: byok+net+shell+fs_write → 4 badges");
  ok(b[0].key.startsWith("byok:") && b[1].key.startsWith("net:") && b[2].key === "shell" && b[3].key === "fs-write",
    "grants: orden secrets→red→shell→fs-write");
}
{
  // orden estable: secrets primero, luego red, luego shell.
  const b = grantBadges(["shell", "net: x.io", "byok: k", "mcp", "net: ninguno"]);
  ok(b.length === 3, "grants: 3 badges (mcp/net-ninguno excluidos)");
  ok(b[0].key.startsWith("byok:") && b[1].key.startsWith("net:") && b[2].key === "shell", "grants: orden secrets→red→shell");
}
{
  // multiples hosts net → un badge por host.
  const b = grantBadges(["net: a.io", "net: b.io"]);
  ok(b.length === 2 && b.every((x) => x.icon === "🌐"), "grants: un badge por host de red");
}
{
  // HIGH fix — CATCH-ALL: un permiso desconocido NO se descarta; produce badge warn con
  // el string crudo. Antes se ignoraba en silencio (falso negativo).
  const b = grantBadges(["filesystem"]);
  ok(b.length === 1 && b[0].tone === "warn" && b[0].icon === "⚠️", "grants: permiso desconocido → 1 badge warn");
  ok(b[0].label.includes("filesystem"), "grants: catch-all muestra el string crudo (filesystem)");
}
{
  // otro permiso desconocido cualquiera (exec) → también badge warn con su string.
  const b = grantBadges(["exec"]);
  ok(b.length === 1 && b[0].tone === "warn" && b[0].label.includes("exec"), "grants: exec desconocido → badge warn con string");
}
{
  // fs_write (conocido) + exec (desconocido) → DOS badges: el de fs + el catch-all de exec.
  const b = grantBadges(["fs_write", "exec"]);
  ok(b.length === 2, "grants: fs_write+exec → 2 badges (conocido + catch-all)");
  ok(b.some((x) => x.key === "fs-write" && x.icon === "📁"), "grants: fs_write+exec incluye badge fs 📁");
  ok(b.some((x) => x.key === "other:exec" && x.icon === "⚠️" && x.label.includes("exec")), "grants: fs_write+exec incluye catch-all exec ⚠️");
  // orden: el conocido (fs-write) antes que el catch-all.
  ok(b[0].key === "fs-write" && b[1].key === "other:exec", "grants: orden conocido→catch-all");
}
{
  // ["mcp"] solo → SIN badges (es marcador, no grant real).
  const b = grantBadges(["mcp"]);
  ok(b.length === 0, "grants: ['mcp'] solo → sin badges (marcador)");
  ok(requiresGrant(["mcp"]) === false, "requiresGrant: ['mcp'] solo → false");
}
{
  // marcadores no-grant adicionales (net: none EN inglés) tampoco producen badge.
  const b = grantBadges(["net: none", "net: ninguno"]);
  ok(b.length === 0, "grants: net: none / net: ninguno → sin badges (marcadores)");
}
{
  // dedup del catch-all: el mismo permiso desconocido repetido → un solo badge.
  const b = grantBadges(["db", "db"]);
  ok(b.length === 1 && b[0].key === "other:db", "grants: catch-all dedup por valor crudo");
}
{
  // orden global con catch-all: secrets→red→shell→fs-write→otros.
  const b = grantBadges(["env", "fs_write", "shell", "net: x.io", "byok: k"]);
  ok(b.length === 5, "grants: byok+net+shell+fs+env → 5 badges");
  ok(b[0].key.startsWith("byok:") && b[1].key.startsWith("net:") && b[2].key === "shell"
    && b[3].key === "fs-write" && b[4].key === "other:env",
    "grants: orden secrets→red→shell→fs→catch-all");
}
{
  // catch-all sanitiza control chars (tab/newline) del string crudo: no rompe la UI.
  const b = grantBadges(["weird\tperm\n"]);
  ok(b.length === 1, "grants: catch-all 1 badge pese a control chars");
  // sin tabs/newlines crudos en el label (colapsados a espacio).
  // eslint-disable-next-line no-control-regex
  ok(!/[\u0000-\u001f\u007f]/.test(b[0].label), "grants: catch-all sanitiza control chars");
  ok(b[0].label.includes("weird perm"), "grants: catch-all conserva el texto visible");
}

// ── canActivate ─────────────────────────────────────────────────────────────
ok(canActivate({ enabled: true, verified: true }).activatable === true, "canActivate: habilitado+firmado → ok");
{
  const r = canActivate({ enabled: false, verified: true });
  ok(r.activatable === false && !!r.reason && r.reason.includes("globalmente"), "canActivate: global-disabled bloquea");
}
{
  const r = canActivate({ enabled: true, verified: false });
  ok(r.activatable === false && !!r.reason && r.reason.includes("firma"), "canActivate: sin firma bloquea (fail-closed)");
}
{
  // sin firma tiene prioridad sobre disabled.
  const r = canActivate({ enabled: false, verified: false });
  ok(r.activatable === false && !!r.reason && r.reason.includes("firma"), "canActivate: sin firma precede a disabled");
}

// ── isPluginActive ──────────────────────────────────────────────────────────
ok(isPluginActive(["codanna"], "codanna") === true, "isActive: presente");
ok(isPluginActive(["codanna"], "word-count") === false, "isActive: ausente");
ok(isPluginActive(null, "x") === false, "isActive: null array");
ok(isPluginActive(undefined, "x") === false, "isActive: undefined array");

// ── togglePlugin ────────────────────────────────────────────────────────────
{
  const before = ["codanna"];
  const after = togglePlugin(before, "word-count");
  ok(after.length === 2 && after.includes("word-count"), "toggle: agrega");
  ok(before.length === 1, "toggle: inmutable (no muta entrada)");
}
ok(togglePlugin(["codanna", "word-count"], "codanna").length === 1, "toggle: saca");
ok(JSON.stringify(togglePlugin(["a", "b"], "a")) === JSON.stringify(["b"]), "toggle: conserva orden del resto");
ok(togglePlugin(null, "x")[0] === "x", "toggle: null array → agrega");
{
  // dedup defensivo: array con duplicados + toggle de un name nuevo → sin duplicados.
  const after = togglePlugin(["a", "a"], "b");
  ok(after.filter((x) => x === "a").length === 1 && after.includes("b"), "toggle: dedup defensivo");
}
{
  // togglear un name ya presente lo elimina TODO (filter saca todas las copias).
  const after = togglePlugin(["a", "a", "b"], "a");
  ok(!after.includes("a") && after.includes("b"), "toggle: saca todas las copias de un dup");
}

// ── validatePlugins (defensivo: sólo nombres que existen en disco) ─────────────
{
  // sólo conserva los names presentes en installedNames; descarta el arbitrario.
  const out = validatePlugins(["codanna", "arbitrario", "word-count"], ["codanna", "word-count"]);
  ok(JSON.stringify(out) === JSON.stringify(["codanna", "word-count"]), "validate: descarta name no instalado");
}
ok(validatePlugins(["x"], []).length === 0, "validate: nada instalado → []");
ok(JSON.stringify(validatePlugins(["a", "b"], ["a", "b"])) === JSON.stringify(["a", "b"]), "validate: todos instalados → intactos, orden preservado");
{
  // dedup: si next trae duplicados pero todos válidos, devuelve uno solo.
  const out = validatePlugins(["a", "a", "b"], ["a", "b"]);
  ok(JSON.stringify(out) === JSON.stringify(["a", "b"]), "validate: dedup conservando orden");
}
{
  // acepta un Set como installedNames (Iterable).
  const out = validatePlugins(["a", "z"], new Set(["a", "c"]));
  ok(JSON.stringify(out) === JSON.stringify(["a"]), "validate: acepta Set y filtra");
}

// ── persistencia save()/create() filtra contra disco (MED 022-p2) ──────────────
// El MED: `toggleCapability` filtraba con validatePlugins, pero el guardado normal
// `save()` persistía `draft.plugins ?? []` SIN validar. Si `draft.plugins` viene de un
// perfil existente/importado/clonado/stale (con nombres arbitrarios o de plugins ya
// desinstalados), save()/create() los reescribía → rompía el invariante "nunca se
// persiste un nombre de plugin que no exista en disco". Este helper modela EXACTAMENTE
// la línea de persistencia del componente (`AgentGallery.save()`):
//   plugins: validatePlugins(draft.plugins ?? [], installed.map(x => x.name))
// para confirmar que TODO camino de persistencia (toggle, save, create) filtra el disco.
function persistedPlugins(draftPlugins: string[] | undefined | null, installedNames: string[]): string[] {
  return validatePlugins(draftPlugins ?? [], installedNames);
}
{
  // save() de un perfil EXISTENTE cuyo draft trae un plugin ya desinstalado → NO se persiste.
  const installedNames = ["codanna", "word-count"];
  const draftPlugins = ["codanna", "removed-plugin"]; // 'removed-plugin' ya no está en disco
  const out = persistedPlugins(draftPlugins, installedNames);
  ok(!out.includes("removed-plugin"), "save: plugin desinstalado en draft NO se persiste");
  ok(JSON.stringify(out) === JSON.stringify(["codanna"]), "save: persiste sólo los instalados");
}
{
  // create() (perfil nuevo, editingId=null) — misma línea de persistencia: un name arbitrario
  // (de un clon/import) NO debe terminar persistido al crear el agente.
  const installedNames = ["codanna"];
  const draftPlugins = ["arbitrario", "codanna"]; // 'arbitrario' p.ej. de un import
  const out = persistedPlugins(draftPlugins, installedNames);
  ok(!out.includes("arbitrario"), "create: name arbitrario importado/clonado NO se persiste");
  ok(JSON.stringify(out) === JSON.stringify(["codanna"]), "create: persiste sólo los instalados");
}
{
  // save() cuando draft.plugins es undefined (perfil sin capabilities) → persiste [].
  ok(JSON.stringify(persistedPlugins(undefined, ["codanna"])) === JSON.stringify([]), "save: draft.plugins undefined → []");
}
{
  // save() de un draft totalmente válido (todos instalados) → intacto, orden preservado.
  const out = persistedPlugins(["codanna", "word-count"], ["codanna", "word-count"]);
  ok(JSON.stringify(out) === JSON.stringify(["codanna", "word-count"]), "save: draft válido → intacto");
}

console.log(`capabilityEditor: ${pass} passed, ${fail} failed`);
process.exit(fail === 0 ? 0 : 1);
