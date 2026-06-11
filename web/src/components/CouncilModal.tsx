// Council on demand (⌘J) — BLOQUE 2: BYOK multi-provider dispatch.
// Reemplaza el adapter al CLI ~/council/ original con council_run_multi (dispatch directo
// a los providers conectados por el user via Furx Connect).

import { useEffect, useState, useMemo, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
// 019 F3 (T031): las MUTACIONES (custom-voices add/remove/toggle, history clear) pasan por el
// gate universal → usamos el invoke envuelto. Las lecturas usan el invoke crudo (no gateado).
import { invoke as gatedInvoke } from "../lib/invoke";
import type {
  CouncilResult, CouncilPreset, ProviderCredential, CouncilRunRecord, CustomVoice,
} from "../types";
import { Modal } from "./Modal";
import { Button } from "./Button";

// BLOQUE J ext · Council Templates by workflow phase (migrations/018_council_templates.sql).
// Templates apply ON TOP of presets: preset narrows by provider type, template narrows by model substring.
// Free + Pro: both ship with all 5 built-in templates.
type CouncilTemplate = {
  name: string;
  display_name: string;
  description: string;
  model_filter: string;
  max_voices: number;
  sort_order: number;
  built_in: boolean;
};

// BLOQUE 3 · Cost estimator simple — Council EDGE_3 fallback (no tiktoken-rs).
// Approximate $0 for free tiers; published per-MTok pricing for paid.
// Token estimate: chars/4 (industry rule of thumb for English/Spanish).
const PRICING_USD_PER_MTOK: Record<string, [number, number] | "free"> = {
  // [input $/MTok, output $/MTok]
  "openrouter":       [3, 15],    // worst case (Claude Sonnet 4.6 via OR with markup)
  "anthropic":        [3, 15],
  "openai":           [5, 30],
  "gemini_paid":      [1.25, 5],
  // Free tiers — cost $0 to the user
  "cerebras":         "free",
  "groq":             "free",
  "mistral":          "free",
  "sambanova":        "free",
  "gemini_studio":    "free",
  "ollama":           "free",
  "lmstudio":         "free",
  "llamacpp":         "free",
  "vllm":             "free",
  "litellm":          "free",
  "custom":           "free",
};

function estimateCost(prompt: string, voiceCount: number, providers: string[]): { usd: number, isApprox: boolean } {
  const inputTokens = Math.ceil(prompt.length / 4);
  const outputBudget = 512; // matches council_multi max_tokens
  let total = 0;
  for (const p of providers.slice(0, voiceCount)) {
    const px = PRICING_USD_PER_MTOK[p];
    if (px === "free" || px === undefined) continue;
    const [pIn, pOut] = px;
    total += (inputTokens / 1_000_000) * pIn + (outputBudget / 1_000_000) * pOut;
  }
  return { usd: total, isApprox: true };
}

import { useT } from "../lib/i18n";
import type { LocaleKey } from "../locales/es";

interface Props { onClose: () => void; }

// descKey en vez de texto: un const a nivel módulo no puede llamar al hook (mismo patrón
// que ConnectScreen, brand wave 4c).
const PRESETS: { value: CouncilPreset; label: string; descKey: LocaleKey }[] = [
  { value: "mix", label: "Mix (default)", descKey: "council.preset.mixDesc" },
  { value: "quick", label: "Quick", descKey: "council.preset.quickDesc" },
  { value: "cheapo", label: "Cheapo", descKey: "council.preset.cheapoDesc" },
  { value: "frontier", label: "Frontier", descKey: "council.preset.frontierDesc" },
  { value: "local", label: "Local", descKey: "council.preset.localDesc" },
];

export function CouncilModal({ onClose }: Props) {
  const t = useT();
  const [query, setQuery] = useState("");
  const [preset, setPreset] = useState<CouncilPreset>("mix");
  const [templateName, setTemplateName] = useState<string>(""); // "" = no template filter
  const [templates, setTemplates] = useState<CouncilTemplate[]>([]);
  const [running, setRunning] = useState(false);
  const [result, setResult] = useState<CouncilResult | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const [creds, setCreds] = useState<ProviderCredential[]>([]);
  // 019 F3 (T031) — tab: convocar | history | voces custom.
  const [tab, setTab] = useState<"run" | "history" | "voices">("run");
  const [history, setHistory] = useState<CouncilRunRecord[]>([]);
  const [customVoices, setCustomVoices] = useState<CustomVoice[]>([]);
  // formulario de alta de voz custom.
  const [newVoiceAlias, setNewVoiceAlias] = useState("");
  const [newVoiceModel, setNewVoiceModel] = useState("");

  const reloadHistory = useCallback(() => {
    invoke<CouncilRunRecord[]>("council_history_list", { limit: 50 })
      .then(setHistory).catch(() => setHistory([]));
  }, []);
  const reloadVoices = useCallback(() => {
    invoke<CustomVoice[]>("council_custom_voices_list")
      .then(setCustomVoices).catch(() => setCustomVoices([]));
  }, []);

  useEffect(() => {
    invoke<ProviderCredential[]>("provider_list")
      .then(setCreds)
      .catch((e) => { console.warn("provider_list failed; cost estimator will show $0", e); });
    invoke<CouncilTemplate[]>("council_templates_list")
      .then(setTemplates)
      .catch((e) => { console.warn("council_templates_list failed", e); setTemplates([]); });
    reloadHistory();
    reloadVoices();
  }, [reloadHistory, reloadVoices]);

  // 019 F3 (T031) — custom-voices: alta (gated + audit). F-II: NUNCA se gatea por tier.
  const addVoice = async () => {
    if (!newVoiceAlias.trim()) return;
    try {
      await gatedInvoke("council_custom_voice_add", {
        providerAlias: newVoiceAlias.trim(),
        model: newVoiceModel.trim() || null,
      });
      setNewVoiceAlias(""); setNewVoiceModel(""); reloadVoices();
    } catch (e) { setErr(String(e)); }
  };
  const toggleVoice = async (v: CustomVoice) => {
    try { await gatedInvoke("council_custom_voice_set_enabled", { id: v.id, enabled: !v.enabled }); reloadVoices(); }
    catch (e) { setErr(String(e)); }
  };
  const removeVoice = async (v: CustomVoice) => {
    try { await gatedInvoke("council_custom_voice_remove", { id: v.id }); reloadVoices(); }
    catch (e) { setErr(String(e)); }
  };
  const clearHistory = async () => {
    try { await gatedInvoke("council_history_clear"); reloadHistory(); }
    catch (e) { setErr(String(e)); }
  };

  const activeTemplate = useMemo(
    () => (templateName ? templates.find((t) => t.name === templateName) ?? null : null),
    [templateName, templates]
  );

  const submit = async () => {
    if (!query.trim() || running) return;
    setRunning(true); setErr(null); setResult(null);
    try {
      const maxV = activeTemplate ? activeTemplate.max_voices : 6;
      const r = await invoke<CouncilResult>("council_run_multi", {
        req: {
          prompt: query,
          preset,
          max_voices: maxV,
          template: templateName || null,
        }
      });
      setResult(r);
      reloadHistory(); // 019 F3 (T031): el run quedó en el history.
    } catch (e) {
      setErr(String(e));
    } finally {
      setRunning(false);
    }
  };

  const healthyCount = creds.filter((c) => c.status === "healthy").length;

  // Cost estimate (only paid providers matter)
  const costEstimate = useMemo(() => {
    if (!query.trim()) return null;
    const healthy = creds.filter((c) => c.status === "healthy");
    let providers: string[];
    if (preset === "quick") {
      // Quick = 6 voces vía OpenRouter (cada una un modelo distinto)
      providers = healthy.some((c) => c.provider === "openrouter")
        ? Array(6).fill("openrouter")
        : [];
    } else if (preset === "cheapo") {
      providers = healthy.filter((c) =>
        ["cerebras", "groq", "mistral", "sambanova", "gemini_studio", "openrouter"].includes(c.provider)
      ).map((c) => c.provider);
    } else if (preset === "frontier") {
      providers = healthy.filter((c) =>
        ["anthropic", "openai", "gemini_paid"].includes(c.provider)
      ).map((c) => c.provider);
    } else if (preset === "local") {
      providers = healthy.filter((c) =>
        ["ollama", "lmstudio", "llamacpp", "vllm"].includes(c.provider)
      ).map((c) => c.provider);
    } else {
      providers = healthy.map((c) => c.provider);
    }
    return estimateCost(query, 6, providers);
  }, [query, preset, creds]);

  return (
    <Modal
      title="Council Mode ⌘J"
      subtitle={`${healthyCount} provider(s) verde · BYOK universal · ⌘↩ submit · Esc cerrar`}
      maxWidth={900}
      onClose={onClose}
      onSubmit={submit}
      canSubmit={tab === "run" && !!query.trim() && !running}
    >
          {/* 019 F3 (T031) — tabs: convocar | history | voces custom. El council es free para
              TODOS los tiers (F-II): ni history ni custom-voices están detrás de un paywall. */}
          <div style={{ display: "flex", gap: 6, marginBottom: 12, borderBottom: "1px solid var(--line)" }}>
            {([["run", t("council.tab.run")], ["history", `${t("council.tab.history")}${history.length ? ` · ${history.length}` : ""}`], ["voices", `${t("council.tab.voices")}${customVoices.length ? ` · ${customVoices.length}` : ""}`]] as const).map(([k, lbl]) => (
              <button key={k} onClick={() => setTab(k)}
                style={{ background: "none", border: "none", borderBottom: tab === k ? "2px solid var(--accent)" : "2px solid transparent",
                         color: tab === k ? "var(--text)" : "var(--muted, #888)", fontWeight: tab === k ? 600 : 400,
                         padding: "4px 10px", cursor: "pointer", fontSize: 13 }}>
                {lbl}
              </button>
            ))}
          </div>

          {tab === "run" && <>
          <div className="form-row">
            <label>{t("council.preset")}</label>
            <div className="form-input">
              <select
                value={preset}
                onChange={(e) => setPreset(e.target.value as CouncilPreset)}
                disabled={running}
                style={{ flex: 1 }}
              >
                {PRESETS.map((p) => (
                  <option key={p.value} value={p.value}>
                    {p.label} — {t(p.descKey)}
                  </option>
                ))}
              </select>
            </div>
          </div>

          <div className="form-row">
            <label>{t("council.template")} <span className="muted small">{t("council.templateOptional")}</span></label>
            <div className="form-input">
              <select
                value={templateName}
                onChange={(e) => setTemplateName(e.target.value)}
                disabled={running || templates.length === 0}
                style={{ flex: 1 }}
                aria-label="Council Template by phase"
              >
                <option value="">— no template (use full preset) —</option>
                {templates.map((t) => (
                  <option key={t.name} value={t.name}>
                    {t.display_name} · ≤{t.max_voices} voices
                  </option>
                ))}
              </select>
            </div>
          </div>
          {activeTemplate && (
            <div className="muted small" style={{ marginTop: -4, marginBottom: 6 }}>
              {activeTemplate.description}
            </div>
          )}

          <textarea
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            disabled={running}
            placeholder={t("council.placeholder")}
            rows={6}
            autoFocus
            aria-label={t("council.queryAria")}
            style={{ width: "100%", background: "var(--bg2)", border: "1px solid var(--line)",
                     borderRadius: 6, padding: 10, fontFamily: "var(--mono)", fontSize: 12,
                     color: "var(--text)", outline: "none", marginTop: 10 }}
          />

          {costEstimate && costEstimate.usd > 0 && (
            <div className="muted small" style={{ marginTop: 8 }}>
              {t("council.costEstimate", { usd: costEstimate.usd.toFixed(4), tokens: Math.ceil(query.length / 4) })}
            </div>
          )}
          {costEstimate && costEstimate.usd === 0 && (
            <div className="muted small" style={{ marginTop: 8 }}>{t("council.costZero")}</div>
          )}
          {err && <div className="card-block warn" style={{ marginTop: 10 }}>error: {err}</div>}
          {running && (
            <div className="muted" style={{ marginTop: 12 }}>
              {t("council.running", { preset })}
            </div>
          )}
          {result && (
            <>
              <div className="muted small" style={{ marginTop: 12 }}>
                {t("council.resultMeta", { ok: result.voices_succeeded, total: result.voices_attempted })} ·{" "}
                {result.elapsed_ms} ms · preset <code>{result.preset}</code>
              </div>
              <div className="voices-list" style={{ marginTop: 8 }}>
                {result.voices.map((v, i) => (
                  <span key={i} className={`voice-pill ${v.ok ? "ok" : "fail"}`}
                        title={v.error ?? `${v.latency_ms} ms`}>
                    {v.ok ? "✓" : "✗"} <code>{v.model.slice(0, 40)}</code>
                  </span>
                ))}
              </div>
              <pre style={{ background: "var(--bg2)", border: "1px solid var(--line)",
                            borderRadius: 6, padding: 12, marginTop: 12, maxHeight: 420,
                            overflowY: "auto", fontSize: 11, color: "var(--text)",
                            whiteSpace: "pre-wrap", fontFamily: "var(--mono)" }}>
                {result.synth}
              </pre>
            </>
          )}
          <div className="wizard-actions">
            <button onClick={onClose}>{result ? t("council.close") : t("common.cancel")}</button>
            <Button variant="primary" onClick={submit} disabled={!query.trim() || running}>
              {running ? t("council.convening") : t("council.convene")} <kbd style={{ marginLeft: 6 }}>⌘↩</kbd>
            </Button>
          </div>
          </>}

          {/* 019 F3 (T031) — Historial de councils corridos (consulta; revertir = correr de nuevo). */}
          {tab === "history" && (
            <div>
              {history.length === 0 ? (
                <div className="muted" style={{ padding: 12 }}>{t("council.historyEmpty")}</div>
              ) : (
                <div style={{ display: "flex", flexDirection: "column", gap: 8, maxHeight: 480, overflowY: "auto" }}>
                  {history.map((h) => (
                    <div key={h.id} style={{ border: "1px solid var(--line)", borderRadius: 6, padding: "8px 10px" }}>
                      <div className="muted small" style={{ display: "flex", gap: 8, flexWrap: "wrap" }}>
                        <span>{h.ran_at}</span>
                        <span>preset <code>{h.preset}</code></span>
                        {h.template && <span>template <code>{h.template}</code></span>}
                        <span>{h.voices_succeeded}/{h.voices_attempted} voces · {h.elapsed_ms} ms</span>
                      </div>
                      {h.prompt && <div style={{ fontSize: 13, marginTop: 4, color: "var(--text)" }}>{h.prompt.slice(0, 240)}{h.prompt.length > 240 ? "…" : ""}</div>}
                      {h.synth && (
                        <pre style={{ background: "var(--bg2)", border: "1px solid var(--line)", borderRadius: 6, padding: 8, marginTop: 6,
                                      maxHeight: 160, overflowY: "auto", fontSize: 11, color: "var(--text)", whiteSpace: "pre-wrap", fontFamily: "var(--mono)" }}>
                          {h.synth}
                        </pre>
                      )}
                    </div>
                  ))}
                </div>
              )}
              {history.length > 0 && (
                <div className="wizard-actions" style={{ marginTop: 12 }}>
                  <button onClick={clearHistory}>{t("council.historyClear")}</button>
                </div>
              )}
            </div>
          )}

          {/* 019 F3 (T031) — Voces custom: SIEMPRE participan del council (config, NO un tier-gate). */}
          {tab === "voices" && (
            <div>
              <div className="muted small" style={{ marginBottom: 10 }}>
                Pineá voces que SIEMPRE participan del council, por encima del preset. Referencian un provider que ya conectaste
                (la key vive en tu Keychain, nunca acá). El council es free para todos los planes.
              </div>
              <div style={{ display: "flex", gap: 8, marginBottom: 12, flexWrap: "wrap", alignItems: "flex-end" }}>
                <div style={{ flex: 2, minWidth: 160 }}>
                  <label className="muted small">{t("council.voiceProvider")}</label>
                  <select value={newVoiceAlias} onChange={(e) => setNewVoiceAlias(e.target.value)}
                    style={{ width: "100%", background: "var(--bg2)", border: "1px solid var(--line)", borderRadius: 6, padding: 8, color: "var(--text)" }}>
                    <option value="">{t("council.voicePickProvider")}</option>
                    {creds.map((c) => <option key={c.alias} value={c.alias}>{c.alias} ({c.provider})</option>)}
                  </select>
                </div>
                <div style={{ flex: 2, minWidth: 160 }}>
                  <label className="muted small">{t("council.voiceModel")}</label>
                  <input value={newVoiceModel} onChange={(e) => setNewVoiceModel(e.target.value)} placeholder={t("council.voiceModelDefault")}
                    style={{ width: "100%", background: "var(--bg2)", border: "1px solid var(--line)", borderRadius: 6, padding: 8, color: "var(--text)", fontFamily: "var(--mono)", fontSize: 12 }} />
                </div>
                <Button variant="primary" onClick={addVoice} disabled={!newVoiceAlias.trim()}>{t("council.voiceAdd")}</Button>
              </div>
              {customVoices.length === 0 ? (
                <div className="muted" style={{ padding: 12 }}>{t("council.voicesEmpty")}</div>
              ) : (
                <div style={{ display: "flex", flexDirection: "column", gap: 6 }}>
                  {customVoices.map((v) => (
                    <div key={v.id} style={{ display: "flex", justifyContent: "space-between", alignItems: "center", gap: 10,
                                             border: "1px solid var(--line)", borderRadius: 6, padding: "6px 10px", opacity: v.enabled ? 1 : 0.55 }}>
                      <div style={{ fontFamily: "var(--mono)", fontSize: 12, color: "var(--text)" }}>
                        {v.provider_alias}{v.model ? ` · ${v.model}` : " · (default)"}
                      </div>
                      <div style={{ display: "flex", gap: 6 }}>
                        <button onClick={() => toggleVoice(v)}>{v.enabled ? t("council.voiceDisable") : t("council.voiceEnable")}</button>
                        <button onClick={() => removeVoice(v)}>{t("council.voiceRemove")}</button>
                      </div>
                    </div>
                  ))}
                </div>
              )}
            </div>
          )}
    </Modal>
  );
}
