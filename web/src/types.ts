// Shared types across components & views.

export interface Card {
  id: string; created_at: string; project: string; source: string; title: string;
  severity: "info" | "warning" | "critical"; status: "open" | "closed";
  // 022 P1 · US6 — campos del inbox accionable (opcionales para compat con backends viejos).
  cause?: string | null;
  /** timestamp ISO hasta el que la card queda pospuesta; null/ausente = no snoozeada. */
  snooze_until?: string | null;
  /** marcada leída (mark-read); null = no leída. */
  read_at?: string | null;
  /** descartada del inbox sin decisión (dismiss); null = activa. */
  dismissed_at?: string | null;
  /** última actividad conocida de la fuente (para auto-unsnooze). */
  last_activity_at?: string | null;
  /** true si fue auto-reabierta por nueva actividad mientras estaba snoozeada (badge "Reabierto"). */
  reopened?: boolean;
}
export interface MonitorTarget { id: string; label: string; kind: string; addr: string; interval_s?: number; }
export interface MonitorResult { id: string; up: boolean; latency_ms: number | null; error: string | null; checked_at: string; }
export interface MonitorSnapshot { target: MonitorTarget; last: MonitorResult | null; }
export interface AuditEvent {
  id: string; at: string; kind: string; actor: string;
  // 047 FR-004 — campos opcionales (el backend los expone desde la tabla events). Se usan para
  // agrupar por sesión (pane_id) y linkear una card de incidente a su evento (card_id).
  pane_id?: string | null;
  card_id?: string | null;
  correlation_id?: string | null;
}

export interface UsageSummary {
  source_files: number;
  total_tokens: number;
  burn_24h_tokens: number;
  burn_7d_tokens: number;
  by_model: { model: string; input_tokens: number; output_tokens: number }[];
  by_session: { session_id: string; input_tokens: number; output_tokens: number; model: string | null; updated_at: string | null }[];
}

// 035-ai-visibility-evidence — espejo TS de `claude_usage::PaneUsage` (comando `claude_usage_for_cwd`).
// Uso de tokens MEDIDO de la sesión Claude del cwd. NULL ⇒ sin sesión registrada (badge "no medido").
export interface PaneUsage {
  session_id: string;
  input_tokens: number;
  output_tokens: number;
  total_tokens: number;
  model: string | null;
  updated_at: string | null;
}

export interface AieState {
  enabled: boolean;
  shadow_mode: boolean;
  total_providers: number;
  healthy_providers: string[];
  blocked_providers: string[];
}

export interface Suggestion { kind: string; label: string; hint: string; }

export interface Project {
  path: string; name: string;
  branch: string | null; last_commit: string | null; last_commit_at: string | null;
  dirty: boolean; scanned_at: string;
}

export interface SshHost { name: string; hostname: string | null; port: number; user: string | null; }
export interface SshHostPing { host: SshHost; up: boolean; latency_ms: number | null; error: string | null; }

export interface PasteClassification { kind: string; bytes: number; lines: number; preview: string; action_hint: string; }

export interface McpServerHealth {
  name: string;
  transport: string;
  healthy: boolean;
  latency_ms: number | null;
  error: string | null;
  /** BLOQUE F · F16 — tool count from `tools/list`; null until handshake completes. */
  tools_count?: number | null;
  /** First few tool names (cap 8) for the tooltip preview. */
  tools_sample?: string[] | null;
  /** 045 FR-002 — estado del toggle del usuario (DB override sobre ~/.claude.json). Default true. */
  enabled: boolean;
}
export interface McpHealthReport { config_path: string | null; servers: McpServerHealth[]; elapsed_ms: number; }
/** 045 FR-002 — binario `mcp-*` hallado en $PATH (sugerencia; NO instalado). */
export interface DiscoveredMcp { binary: string; path: string; already_configured: boolean; }

export interface HeatmapCell { day: string; hour: number; count: number; }
export interface HeatmapData { cells: HeatmapCell[]; max_count: number; total: number; days: number; }

// B9 — PaneMode ahora acepta cualquier claude-<slug> dinámico además de los built-ins.
// El backend pty.rs valida el slug antes de spawnear (~/bin/claude-as-<slug>).
export type PaneMode = "zsh" | "codex" | "gemini" | "aider" | "grok" | `claude-${string}`;
export interface PaneCfg {
  id: string;
  mode: PaneMode;
  title: string;
  /** BLOQUE B · F2/F3 — sticky cwd hint persisted in layout.
   *  Set by Card→Claude flow when a worktree is created so reload restores it. */
  cwd?: string;
  /** BLOQUE B · F2 — last bundle context attached to this pane (for reuse). */
  bundle_path?: string;
  /** spec 004 F0 — pane kind. Absent or "terminal" = a CLI terminal pane (default, so
   *  pre-existing layouts stay terminal). "data" = JSON/CSV viewer; "compare" = side-by-side. */
  kind?: "terminal" | "data" | "compare" | "web" | "context";
  /** spec 004 F3 — read-only content shown in a data viewer pane (persisted in layout). */
  data_content?: string;
  /** spec 004 F4 — the two responses compared side-by-side (persisted in layout). */
  compare_left?: string;
  compare_right?: string;
  /** spec 004 F1 — pinned URL for a web (context-viewer) pane. Rendered in a sandboxed
   *  iframe (no Tauri IPC access → BYOK F-I preserved). No free address bar. */
  web_url?: string;
  /** spec 004 F2 — project-context pane: repo dir for the Git surface + the file/glob
   *  paths selected as context for the Council (persisted in layout). */
  context_repo?: string;
  context_paths?: string;
  /** 006 agent-profiles — si está seteado, ESTE agente maneja el runtime del pane
   *  (override del `mode` legacy). El backend resuelve cli/cuenta/modelo/prompt/cwd. */
  agent_profile_id?: string;
  /** 008 orchestration — sesión tmux única por tarea (FURX_<orch_session>) para que N tareas
   *  del mismo agente no compartan sesión y persistan al desmontar/remontar el pane. */
  orch_session?: string;
}

// 008 parallel-orchestration — una tarea autónoma en su worktree con su agente.
export interface OrchTask {
  id: string;
  batch_id: string;
  title: string;
  objective: string;
  agent_profile_id?: string | null;
  mode?: string | null;
  repo_path: string;
  branch: string;
  worktree_path?: string | null;
  pane_id?: string | null;
  state: "pending" | "running" | "awaiting_review" | "done" | "failed" | "canceled";
  exit_code?: number | null;
  result_summary?: string | null;
  // 012-pty-done-detection — sub-estado + flags del poller (0/1).
  needs_input?: number;
  auto_confirm?: number;
  cli_kind?: string | null;
  // 014-orchestration-ux — best-of-N: grupo de variantes (NULL = tarea normal).
  group_id?: string | null;
  variant_index?: number | null;
  // 019 F3 (T030) — pausa: ISO de cuándo se pausó el attempt (NULL = corriendo normal).
  paused_at?: string | null;
  // 038 Goose-C P1 — DAG de pipelines. Campos DERIVADOS (single-task: depends_on=[], pipeline_run_id
  // null, dag_blocked=0 → idéntico a hoy). depends_on = ids de tareas de las que ésta depende;
  // dag_blocked=1 → esperando sus deps (no lanzable); pipeline_run_id = run al que pertenece.
  depends_on?: string[];
  pipeline_run_id?: string | null;
  dag_blocked?: number;
  created_at: string;
  updated_at: string;
}

// 038 F1.5 — superficie derivada por run (FR-009): un run `running` sin ninguna tarea corriendo pero
// con algo en review = "esperando tu review/aprobación" (indistinguible de un hang en un pipeline
// lineal sin esta señal). El backend lo deriva; el board muestra el advisory "esperando review hace Nm".
export interface PipelineWaiting {
  run_id: string;
  waiting_minutes: number;
}

// 019 F3 (T030) — ETA: estimación de tiempo restante de un batch/grupo (cálculo puro en Rust).
export interface EtaEstimate {
  avg_terminal_secs: number;
  running: number;
  finished: number;
  eta_secs: number;
}

// 014-orchestration-ux — best-of-N: grupo de variantes de un objetivo.
export interface OrchTaskGroup {
  id: string;
  batch_id: string;
  objective: string;
  n: number;
  chosen_task_id?: string | null;
  created_at: string;
  updated_at: string;
}

// 014-orchestration-ux — una variante en la comparación N-way.
export interface OrchVariantDiff {
  task_id: string;
  variant_index: number | null;
  title: string;
  repo_path: string;
  branch: string;
  state: OrchTask["state"];
  diff_stat: string;
  risky_paths: string[];
}

// ── 024-quality-gate — evidencia objetiva por variante (espejo del payload Rust) ──
// CONTRATO FAIL-SAFE: "no disponible" ≠ 0. `status != "ok"` ⇒ la UI muestra "no disponible",
// NUNCA "0 issues" (un 0 falso diría "limpio" cuando NO se midió).
export type LinterStatus = "ok" | "unavailable" | "timeout" | "unparsable";

export interface QgIssue {
  file: string;
  line: number;
  rule: string;
  message: string;
  severity: string; // "error" | "warning"
}

export interface LinterResult {
  tool: string;
  status: LinterStatus;
  errors: number;
  warnings: number;
  issues?: QgIssue[];
  reason?: string | null;
  raw_excerpt?: string | null;
  elapsed_ms: number;
}

export interface VariantEvidence {
  task_id: string;
  total_errors: number;
  total_warnings: number;
  by_tool: LinterResult[];
  unavailable_tools: string[];
  /** `false` ⇒ NADA se pudo medir (la UI NO debe mostrar "0 limpio"). */
  any_measured: boolean;
}

// ── 026-preference-loop — prior local explicable + ranking advisory enriquecido ──
// Espejo de los structs Rust (`preference_signal::*`, `commands::Prior*`). Local-first,
// SIEMPRE advisory: el ranking sigue siendo sugerencia; la explicación NUNCA es opaca.

/** Un factor de la explicación de una sugerencia (FR-023): feature, dirección y contribución. */
export interface ExplanationFactor {
  feature_key: string;
  /** "menos es mejor" | "más es mejor" | "neutro". */
  direction: string;
  /** Contribución signed de este feature al score de la variante. */
  contribution: number;
  /** Peso aprendido del feature ∈ [-1,1]. */
  weight: number;
}

/** La explicación legible de UNA variante en el ranking enriquecido. */
export interface VariantExplanation {
  task_id: string;
  combined_score: number;
  /** Score del ranking de 020 (AIE/heurística), normalizado [0,1]. */
  base_score: number;
  /** Contribución del prior [0,1] (0.5 neutro). */
  prior_score: number;
  factors: ExplanationFactor[];
}

/** El ranking advisory enriquecido: orden + explicación por variante + flags de gobierno. */
export interface RankingExplanation {
  /** Orden advisory (índices de variantes, mejor→peor). */
  order: number[];
  variants: VariantExplanation[];
  /** El prior está en cold-start (aún aprendiendo) → no se inyectó. */
  still_learning: boolean;
  /** La inyección está desactivada por setting (`preference.inject` OFF) → no se inyectó. */
  inject_disabled: boolean;
}

/** Un feature del prior inspeccionado (peso/dirección + evidencia Beta que lo respalda). */
export interface PriorFeatureView {
  feature_key: string;
  weight: number;
  direction: string;
  alpha: number;
  beta: number;
  distinct_obs: number;
}

/** La vista inspeccionable del prior de un contexto (FR-030). */
export interface PriorView {
  repo_key: string;
  task_type: string;
  sample_count: number;
  /** ¿Superó el cold-start (≥15 muestras + diversidad)? */
  is_warm: boolean;
  features: PriorFeatureView[];
}

// 014-orchestration-ux — un snapshot del log-history de una tarea.
export interface OrchLogEntry {
  id: string;
  task_id: string;
  captured_at: string;
  source: string;
  content: string;
}

// 006 agent-profiles — el agente como entidad de primera clase. Config NO-secreta;
// el token vive en Keychain y se referencia vía account_slug (NUNCA el token).
export interface AgentProfile {
  id: string;
  name: string;
  description: string;
  cli_kind: string; // zsh|claude|codex|gemini|aider|openai-api|custom
  account_slug?: string | null;
  model?: string | null;
  system_prompt: string;
  default_cwd?: string | null;
  council_enabled: boolean;
  council_preset?: string | null;
  shell_enabled: boolean;
  icon?: string | null;
  color?: string | null;
  is_builtin: boolean;
  /** 006 ext — motor: 'cli' (CLI en pane) | 'aie' (REPL HTTP, diferido). MVP: 'cli'. */
  engine_kind: string;
  /** 006 ext — categoría para agrupar presets/roles ('soporte'|'ventas'|'qa'|...). */
  category?: string | null;
  plugins: string[];
  created_at: string;
  updated_at: string;
}

// BLOQUE 1 — Wizard Furx Connect (BYOK universal)
export type ProviderKind =
  | "openrouter" | "cerebras" | "groq" | "mistral" | "sambanova"
  | "gemini_studio" | "anthropic" | "openai" | "gemini_paid"
  | "ollama" | "lmstudio" | "llamacpp" | "vllm" | "litellm" | "custom";

export type ProviderStatus = "healthy" | "amber" | "red" | "unconfigured";

export interface ProviderCredential {
  alias: string;
  provider: ProviderKind;
  key_ref: string | null;
  endpoint_url: string | null;
  status: ProviderStatus;
  last_ping_ms: number | null;
  last_ping_at: string | null;
  last_error_msg: string | null;
  scope_workspace: string | null;
  preset_member: string | null;
  created_at: string;
  updated_at: string;
}

export interface PingResult {
  ok: boolean;
  latency_ms: number;
  model: string | null;
  error: string | null;
}

export interface PersistRequest {
  alias: string;
  provider: ProviderKind;
  key: string | null;
  endpoint_url: string | null;
  scope_workspace: string | null;
  preset_member: string | null;
}

export type LicenseState =
  | { kind: "valid"; tier: string; until: string }
  | { kind: "trial"; until: string }
  | { kind: "expired" }
  | { kind: "offline"; last_check: string; cached_state: string | null };

// BLOQUE 2 — Local provider scan + Council multi
export interface LocalProviderInfo {
  kind: string;
  alive: boolean;
  endpoint: string;
  models: string[];
  latency_ms: number;
  error: string | null;
}

export interface LocalScan {
  ollama: LocalProviderInfo;
  lmstudio: LocalProviderInfo;
  llamacpp: LocalProviderInfo;
  scanned_at: string;
}

export type CouncilPreset = "quick" | "cheapo" | "frontier" | "local" | "mix";

export interface CouncilRequest {
  prompt: string;
  preset?: CouncilPreset;
  max_voices?: number;
}

export interface VoiceResult {
  provider: string;
  alias: string;
  model: string;
  ok: boolean;
  content: string;
  latency_ms: number;
  error: string | null;
}

export interface CouncilResult {
  voices: VoiceResult[];
  synth: string;
  elapsed_ms: number;
  preset: string;
  voices_attempted: number;
  voices_succeeded: number;
}

// 019 F3 (T031) — council history: un run registrado (consulta). voices_json = metadata NO-secreta.
export interface CouncilRunRecord {
  id: string;
  ran_at: string;
  preset: string;
  template: string | null;
  prompt: string;
  synth: string;
  voices_attempted: number;
  voices_succeeded: number;
  elapsed_ms: number;
  voices_json: string;
}

// 019 F3 (T031) — council custom-voice (config del user, NUNCA un tier-gate; F-II).
export interface CustomVoice {
  id: string;
  provider_alias: string;
  model: string | null;
  enabled: boolean;
  created_at: string;
}

// B9 + B9.1 — Universal CLI Accounts (Claude / Codex / Gemini / Aider / openai-api / custom)
export type CliKind = "claude" | "codex" | "gemini" | "aider" | "grok" | "openai-api" | "custom";

export interface ClaudeAccount {
  cli_kind: CliKind;
  slug: string;
  label: string;
  browser: string | null;
  status: "verified" | "unverified" | "missing_token";
  env_var: string | null;
  keychain_service: string | null;
  last_verified_at: string | null;
  last_used_at: string | null;
  created_at: string;
  updated_at: string;
}

export interface ClaudeAccountAddRequest {
  slug: string;
  label: string;
  cli_kind: CliKind;
  browser: string | null;
  env_var?: string | null;
  keychain_service?: string | null;
}

export interface ClaudeAccountVerifyResult {
  slug: string;
  cli_kind: CliKind;
  ok: boolean;
  status: string;
  message: string;
}

// Paleta de CATEGORÍA (data-viz, no marca): cada CLI tiene un hue distinto para
// distinguir panes/cuentas en la UI (como la leyenda de un chart). Son theme-invariantes
// por diseño — NO usan el accent coral de marca (eso los volvería indistinguibles entre sí
// y de la identidad). El accent de marca es var(--accent); estos son ortogonales.
export const CLI_KIND_META: Record<CliKind, { label: string; color: string; envHint: string }> = {
  claude:       { label: "Claude Code",   color: "#b86bc4", envHint: "CLAUDE_CODE_OAUTH_TOKEN" },
  codex:        { label: "Codex CLI",     color: "#f4b860", envHint: "OPENAI_API_KEY" },
  gemini:       { label: "Gemini CLI",    color: "#6bd97a", envHint: "GEMINI_API_KEY" },
  aider:        { label: "Aider",         color: "#ff6b6b", envHint: "ANTHROPIC_API_KEY" },
  grok:         { label: "Grok CLI",      color: "#3bc9db", envHint: "XAI_API_KEY" },
  "openai-api": { label: "OpenAI API",    color: "#7c7cff", envHint: "OPENAI_API_KEY" },
  custom:       { label: "Custom",        color: "#a0b1c8", envHint: "API_KEY" },
};

export function formatTok(n: number): string {
  if (n < 1000) return String(n);
  if (n < 1_000_000) return `${(n / 1000).toFixed(1)}k`;
  return `${(n / 1_000_000).toFixed(2)}M`;
}

export function fmtTime(iso: string): string {
  const d = new Date(iso.includes("T") ? iso : iso.replace(" ", "T") + "Z");
  return d.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit", second: "2-digit" });
}

export function fmtWhen(iso: string): string {
  const d = new Date(iso.includes("T") ? iso : iso.replace(" ", "T") + "Z");
  const m = Math.floor((Date.now() - d.getTime()) / 60000);
  if (m < 1) return "now";
  if (m < 60) return `${m}m ago`;
  if (m < 1440) return `${Math.floor(m / 60)}h ago`;
  return d.toLocaleDateString();
}
