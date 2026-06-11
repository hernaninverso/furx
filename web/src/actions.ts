// Registry de acciones para ⌘K palette de acciones.
// Council 5/5 consenso 2026-05-26: opción B + ⌘⇧K como respaldo project.
// Filtro `available()` para esconder acciones sin sentido en el contexto actual.
// `proGated` muestra greyed + click → ProGateModal (no oculta — discoverability).

import type { PaneCfg } from "./types";

export type ActionGroup = "Pane" | "Modal" | "View" | "System";

export interface ActionEntry {
  id: string;
  label: string;
  hint?: string;
  group: ActionGroup;
  shortcut?: string;
  proGated?: boolean;
  available?: () => boolean;
  run: () => void;
}

const VIEW_LABELS: Record<string, { label: string; hint: string }> = {
  panes: { label: "Panes", hint: "grid de terminales" },
  incidents: { label: "Incidents", hint: "cards abiertas" },
  monitors: { label: "Monitors", hint: "health checks" },
  audit: { label: "Audit log", hint: "events append-only" },
  saas: { label: "SaaS", hint: "tenants + billing" },
  health: { label: "Health", hint: "MCP + services" },
  heatmap: { label: "Heatmap", hint: "actividad por pane" },
  grafana: { label: "Grafana", hint: "dashboards embebidos" },
  ssh: { label: "SSH targets", hint: "saved hosts" },
  vpn: { label: "VPN status", hint: "Tailscale + WG" },
  latency: { label: "Latency", hint: "AIE providers" },
  search: { label: "Search view", hint: "code · memories · git" },
  eval: { label: "Eval", hint: "twin metrics" },
  queue: { label: "Queue", hint: "background jobs" },
  router: { label: "Router", hint: "AIE routing" },
  replay: { label: "Replay", hint: "session scrubber" },
  tools: { label: "Tools", hint: "skills + integrations" },
  plugins: { label: "Plugins", hint: "extensiones MCP firmadas" },
  memory: { label: "Memory", hint: "cross-CLI memory hub" },
  settings: { label: "Settings", hint: "preferences" },
  // 058 (ultrareview fix) — la reestructura 055 sacó estas vistas del sidebar y las marcó como "demotadas
  // a ⌘K", pero NO estaban en este catálogo → quedaban INALCANZABLES (ni sidebar ni paleta). Las agrego
  // para que la invariante "nada se elimina, sólo se demota a la paleta" sea real. `activity` (la nueva
  // espina) suma paridad con el resto de la espina (todas alcanzables por ⌘K).
  activity: { label: "Activity", hint: "centro de excepciones" },
  github: { label: "GitHub", hint: "PRs + repos" },
  reliability: { label: "Reliability", hint: "SLOs + uptime" },
  savings: { label: "Savings", hint: "ahorro del routing" },
  crashlog: { label: "Crash logs", hint: "fallos registrados" },
  extensions: { label: "Extensions", hint: "plugins + skills" },
  policy: { label: "Policy", hint: "reglas + guardrails" },
  presets: { label: "Presets", hint: "configs guardadas" },
};

const VIEW_IDS = Object.keys(VIEW_LABELS);

export interface BuildActionsDeps {
  panes: PaneCfg[];
  focusedPane: string | null;
  proActive: boolean | null;
  addPane: () => void;
  removePane: (id: string) => void;
  cyclePaneMode: (id: string) => void;
  setFocusedPane: (id: string | null) => void;
  setView: (v: string) => void;
  openBroadcast: () => void;
  openVoice: () => void;
  openSmartPaste: () => void;
  openStandup: () => void;
  openPr: () => void;
  openDisagree: () => void;
  openCouncil: () => void;
  setAuditDrawerOpen: (v: boolean | ((prev: boolean) => boolean)) => void;
  snapshotManual: () => void;
  // spec 001 US1 — read the focused pane's last output aloud (local OS TTS).
  readAloud: () => void;
  stopReadAloud: () => void;
  ttsAvailable: boolean;
  // spec 001 T032 — toggle auto-read-on-idle for the focused pane.
  toggleAutoRead: () => void;
  autoReadOn: boolean;
}

export function buildActions(deps: BuildActionsDeps): ActionEntry[] {
  const hasFocused = () => deps.focusedPane !== null;
  const items: ActionEntry[] = [];

  // PANE
  items.push(
    { id: "pane.add", label: "Nuevo pane", hint: "agregar terminal", group: "Pane", shortcut: "⌘N", run: deps.addPane },
    {
      id: "pane.close", label: "Cerrar pane focado", hint: "remove + persist layout",
      group: "Pane", shortcut: "⌘W", available: hasFocused,
      run: () => { if (deps.focusedPane) { deps.removePane(deps.focusedPane); deps.setFocusedPane(null); } },
    },
    {
      id: "pane.cycle-mode", label: "Cambiar modo del pane", hint: "zsh → claude → codex → …",
      group: "Pane", shortcut: "⌘⇧M", available: hasFocused,
      run: () => { if (deps.focusedPane) deps.cyclePaneMode(deps.focusedPane); },
    },
    {
      id: "pane.read-aloud", label: "Read aloud — última respuesta", hint: "TTS local del OS · resumen",
      group: "Pane", available: () => hasFocused() && deps.ttsAvailable, run: deps.readAloud,
    },
    {
      id: "pane.stop-reading", label: "Stop reading", hint: "cortar el TTS",
      group: "Pane", available: () => deps.ttsAvailable, run: deps.stopReadAloud,
    },
    {
      id: "pane.auto-read", label: deps.autoReadOn ? "Auto-read: ON (apagar)" : "Auto-read on completion",
      hint: "leer el resumen cuando el pane termina", group: "Pane",
      available: () => hasFocused() && deps.ttsAvailable, run: deps.toggleAutoRead,
    },
  );
  for (let i = 0; i < 8; i++) {
    const idx = i;
    items.push({
      id: `pane.focus.${idx + 1}`,
      label: `Focar pane ${idx + 1}`,
      hint: `⌘${idx + 1}`,
      group: "Pane",
      shortcut: `⌘${idx + 1}`,
      available: () => idx < deps.panes.length,
      run: () => { const p = deps.panes[idx]; if (p) deps.setFocusedPane(p.id); },
    });
  }

  // MODAL
  items.push(
    { id: "modal.broadcast", label: "Broadcast", hint: "enviar a varios panes", group: "Modal", shortcut: "⌘B", run: deps.openBroadcast },
    {
      // Council es FREE (OPEN-CORE.md) — sin proGated ni pill PRO; abre directo.
      id: "modal.council", label: "Council Mode", hint: "multi-provider consensus",
      group: "Modal", shortcut: "⌘J",
      run: () => deps.openCouncil(),
    },
    { id: "modal.voice", label: "Voice (preview)", hint: "dictar prompt", group: "Modal", shortcut: "⌘⇧V", run: deps.openVoice },
    { id: "modal.smartpaste", label: "Smart Paste", hint: "auto-detect provider+payload", group: "Modal", run: deps.openSmartPaste },
    { id: "modal.standup", label: "Standup", hint: "resumen diario", group: "Modal", run: deps.openStandup },
    { id: "modal.pr", label: "PR Description", hint: "generar body", group: "Modal", run: deps.openPr },
    { id: "modal.disagree", label: "Disagreement", hint: "diff entre panes", group: "Modal", run: deps.openDisagree },
  );

  // VIEW
  for (const vid of VIEW_IDS) {
    const meta = VIEW_LABELS[vid];
    items.push({
      id: `view.${vid}`,
      label: `Ir a ${meta.label}`,
      hint: meta.hint,
      group: "View",
      run: () => deps.setView(vid),
    });
  }

  // SYSTEM
  items.push(
    { id: "system.snapshot", label: "Snapshot manual", hint: "DB + audit", group: "System", shortcut: "⌘⇧S", run: deps.snapshotManual },
    {
      id: "system.audit-toggle", label: "Toggle Audit drawer", hint: "panel lateral derecho",
      group: "System", run: () => deps.setAuditDrawerOpen((v) => !v),
    },
    // 059 — se removió la entrada de seed-demo de ESTA palette estática (+ su gate isDevBuild). El
    // comando sigue en el registry Rust como dev-only; la palette universal (CommandPalette015) lo
    // excluye en producción y el backend lo rechaza en release.
  );

  return items;
}

// History persistido en localStorage. Cap 10 IDs. Council EDGE_3: descartar IDs huérfanos al cargar.
const RECENT_KEY = "furx.palette.recent";
const RECENT_CAP = 10;

export function loadRecent(validIds: Set<string>): string[] {
  try {
    const raw = localStorage.getItem(RECENT_KEY);
    if (!raw) return [];
    const arr = JSON.parse(raw);
    if (!Array.isArray(arr)) return [];
    return arr.filter((x) => typeof x === "string" && validIds.has(x)).slice(0, RECENT_CAP);
  } catch {
    return [];
  }
}

export function pushRecent(id: string) {
  try {
    const raw = localStorage.getItem(RECENT_KEY);
    const parsed: unknown = raw ? JSON.parse(raw) : [];
    // Gemini MED-2: handle corruption (raw="true"/"123") — only accept arrays of strings.
    const cur: string[] = Array.isArray(parsed) ? parsed.filter((x): x is string => typeof x === "string") : [];
    const next = [id, ...cur.filter((x) => x !== id)].slice(0, RECENT_CAP);
    localStorage.setItem(RECENT_KEY, JSON.stringify(next));
  } catch {
    /* localStorage puede fallar en private mode / quota — silencioso */
  }
}
