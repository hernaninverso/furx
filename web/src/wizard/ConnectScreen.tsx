// BLOQUE 2 · Wizard "Furx Connect" — 5 tabs operativos: OpenRouter / Free / Paid / Local / Proxy.
// 2026-06-09 brand wave 4: copy user-facing vía i18n (keys connect.*). Los specs a nivel módulo
// referencian KEYS (helpKey/guideKeys/labelKey) y se resuelven con t() en render — un const a nivel
// módulo no puede llamar al hook.

import { useEffect, useState } from "react";
import { invoke } from "../lib/invoke"; // 015 T015: invoke con flujo de aprobación universal
import { open as openExternal } from "@tauri-apps/plugin-shell";
import type {
  ProviderCredential,
  PingResult,
  PersistRequest,
  ProviderKind,
  LocalScan,
} from "../types";
import { ClaudeAccountsTab } from "./ClaudeAccountsTab";
import { Button } from "../components/Button";
import { useT } from "../lib/i18n";
import type { LocaleKey } from "../locales/es";

type TabKey = "openrouter" | "free" | "paid" | "local" | "proxy" | "claude-accounts";

interface Props {
  onDone: () => void;
  // 022 US7 / FR-012 — abre el wizard directo en una pestaña (ej. Ajustes → Cuentas abre
  // "claude-accounts" sin pasar por el onboarding/ProGate). Sin esta prop conserva el arranque
  // original ("openrouter"), así el Wizard y el ProGate siguen funcionando igual.
  initialTab?: TabKey;
}

interface ProviderSpec {
  kind: ProviderKind;
  // label fijo (nombre de producto) O labelKey traducible — uno de los dos.
  label?: string;
  labelKey?: LocaleKey;
  defaultAlias: string;
  keyPlaceholder: string;
  keyPrefixHint?: string;
  signupUrl: string;
  helpKey: LocaleKey;
  // 060 v1 — instructivo paso a paso de "cómo sacar la key gratis" (colapsable). El último paso es
  // siempre "pegala arriba". La key la pega el HUMANO → va directo al Keychain (BYOK intacto).
  guideKeys?: LocaleKey[];
}

const FREE_TIERS: ProviderSpec[] = [
  {
    kind: "cerebras", label: "Cerebras", defaultAlias: "cerebras-main",
    keyPlaceholder: "csk-...", keyPrefixHint: "csk-",
    signupUrl: "https://cloud.cerebras.ai/",
    helpKey: "connect.help.cerebras",
    guideKeys: ["connect.guide.cerebras.1", "connect.guide.cerebras.2", "connect.guide.cerebras.3"],
  },
  {
    kind: "groq", label: "Groq", defaultAlias: "groq-main",
    keyPlaceholder: "gsk_...", keyPrefixHint: "gsk_",
    signupUrl: "https://console.groq.com/keys",
    helpKey: "connect.help.groq",
    guideKeys: ["connect.guide.groq.1", "connect.guide.groq.2", "connect.guide.groq.3"],
  },
  {
    kind: "mistral", label: "Mistral", defaultAlias: "mistral-main",
    keyPlaceholder: "...",
    signupUrl: "https://console.mistral.ai/api-keys/",
    helpKey: "connect.help.mistral",
    guideKeys: ["connect.guide.mistral.1", "connect.guide.mistral.2", "connect.guide.mistral.3"],
  },
  {
    kind: "sambanova", label: "SambaNova", defaultAlias: "sambanova-main",
    keyPlaceholder: "...",
    signupUrl: "https://cloud.sambanova.ai/apis",
    helpKey: "connect.help.sambanova",
    guideKeys: ["connect.guide.sambanova.1", "connect.guide.sambanova.2", "connect.guide.sambanova.3"],
  },
  {
    kind: "gemini_studio", labelKey: "connect.label.geminiStudio", defaultAlias: "gemini-studio-main",
    keyPlaceholder: "AIza...", keyPrefixHint: "AIza",
    signupUrl: "https://aistudio.google.com/apikey",
    helpKey: "connect.help.gemini",
    guideKeys: ["connect.guide.gemini.1", "connect.guide.gemini.2", "connect.guide.gemini.3"],
  },
];

const PAID_APIS: ProviderSpec[] = [
  {
    kind: "anthropic", labelKey: "connect.label.anthropic", defaultAlias: "anthropic-main",
    keyPlaceholder: "sk-ant-...", keyPrefixHint: "sk-ant-",
    signupUrl: "https://console.anthropic.com/settings/keys",
    helpKey: "connect.help.anthropic",
  },
  {
    kind: "openai", labelKey: "connect.label.openai", defaultAlias: "openai-main",
    keyPlaceholder: "sk-...", keyPrefixHint: "sk-",
    signupUrl: "https://platform.openai.com/api-keys",
    helpKey: "connect.help.openai",
  },
  {
    kind: "gemini_paid", labelKey: "connect.label.geminiPaid", defaultAlias: "gemini-paid-main",
    keyPlaceholder: "AIza...", keyPrefixHint: "AIza",
    signupUrl: "https://aistudio.google.com/apikey",
    helpKey: "connect.help.geminiPaid",
  },
];

const OPENROUTER_SPEC: ProviderSpec = {
  kind: "openrouter", labelKey: "connect.openrouter.label",
  defaultAlias: "openrouter-main",
  keyPlaceholder: "sk-or-v1-...", keyPrefixHint: "sk-or-",
  signupUrl: "https://openrouter.ai/keys",
  helpKey: "connect.openrouter.help",
};

export function ConnectScreen({ onDone, initialTab }: Props) {
  const t = useT();
  const [tab, setTab] = useState<TabKey>(initialTab ?? "openrouter");
  const [creds, setCreds] = useState<ProviderCredential[]>([]);

  const refresh = async () => {
    try {
      const list = await invoke<ProviderCredential[]>("provider_list");
      setCreds(list);
    } catch (e) {
      console.error(e);
    }
  };

  useEffect(() => { refresh(); }, []);

  return (
    <div className="wizard-backdrop">
      <div className="wizard wizard-wide">
        <header className="wizard-header">
          <span className="hex" />
          <div>
            <h2>Furx Connect</h2>
            <div className="muted">{t("connect.subtitle")}</div>
          </div>
          <Button variant="ghost" onClick={onDone}>{t("connect.close")}</Button>
        </header>

        <nav className="wizard-tabs" role="tablist">
          <Tab active={tab === "openrouter"} onClick={() => setTab("openrouter")}>Quick Start</Tab>
          <Tab active={tab === "claude-accounts"} onClick={() => setTab("claude-accounts")}>{t("connect.tab.accounts")}</Tab>
          <Tab active={tab === "free"} onClick={() => setTab("free")}>Free Tiers</Tab>
          <Tab active={tab === "paid"} onClick={() => setTab("paid")}>{t("connect.tab.paid")}</Tab>
          <Tab active={tab === "local"} onClick={() => setTab("local")}>Local</Tab>
          <Tab active={tab === "proxy"} onClick={() => setTab("proxy")}>Proxy</Tab>
        </nav>

        <main className="wizard-body">
          {tab === "openrouter" && (
            <SingleProviderPane
              spec={OPENROUTER_SPEC}
              existing={creds.find((c) => c.alias === "openrouter-main")}
              presetMember="quick,mix"
              onSaved={refresh}
            />
          )}

          {tab === "free" && (
            <MultiProviderPane
              title={t("connect.free.title")}
              subtitle={t("connect.free.subtitle")}
              specs={FREE_TIERS}
              creds={creds}
              presetMember="cheapo,mix"
              onSaved={refresh}
            />
          )}

          {tab === "paid" && (
            <MultiProviderPane
              title={t("connect.paid.title")}
              subtitle={t("connect.paid.subtitle")}
              specs={PAID_APIS}
              creds={creds}
              presetMember="frontier,mix"
              onSaved={refresh}
            />
          )}

          {tab === "local" && (
            <LocalPane creds={creds} onSaved={refresh} />
          )}

          {tab === "proxy" && (
            <ProxyPane creds={creds} onSaved={refresh} />
          )}

          {tab === "claude-accounts" && (
            <ClaudeAccountsTab onChanged={refresh} />
          )}
        </main>

        <footer className="wizard-footer">
          <div className="muted small">
            {t("connect.footer.status", {
              healthy: creds.filter((c) => c.status === "healthy").length,
              issues: creds.filter((c) => c.status === "amber" || c.status === "red").length,
            })}
          </div>
          <Button variant="primary" onClick={onDone}>{t("connect.footer.done")}</Button>
        </footer>
      </div>
    </div>
  );
}

function Tab({
  active, onClick, children,
}: {
  active: boolean; onClick: () => void; children: React.ReactNode;
}) {
  return (
    <button
      className={`wizard-tab ${active ? "active" : ""}`}
      onClick={onClick}
      role="tab"
      aria-selected={active}
    >
      {children}
    </button>
  );
}

interface SingleProviderProps {
  spec: ProviderSpec;
  existing?: ProviderCredential;
  presetMember: string;
  onSaved: () => void;
  endpointOverride?: string;
}

function SingleProviderPane({ spec, existing, presetMember, onSaved, endpointOverride }: SingleProviderProps) {
  const t = useT();
  const [key, setKey] = useState("");
  const [busy, setBusy] = useState(false);
  const [ping, setPing] = useState<PingResult | null>(null);
  const [err, setErr] = useState<string | null>(null);

  const label = spec.labelKey ? t(spec.labelKey) : (spec.label ?? spec.kind);
  const helpLine = t(spec.helpKey);

  const openSignup = async () => {
    try { await openExternal(spec.signupUrl); }
    catch (e) { setErr(t("connect.err.browser", { error: String(e) })); }
  };

  const handleConnect = async () => {
    if (busy) return;
    setErr(null); setPing(null);
    if (!key.trim()) {
      setErr(t("connect.err.pasteKey"));
      return;
    }
    if (spec.keyPrefixHint && !key.trim().startsWith(spec.keyPrefixHint)) {
      setErr(t("connect.err.keyPrefix", { label, prefix: spec.keyPrefixHint }));
      return;
    }
    setBusy(true);
    try {
      const req: PersistRequest = {
        alias: spec.defaultAlias,
        provider: spec.kind,
        key: key.trim(),
        endpoint_url: endpointOverride ?? null,
        scope_workspace: null,
        preset_member: presetMember,
      };
      await invoke<ProviderCredential>("provider_persist", { req });
      const result = await invoke<PingResult>("provider_test", { alias: spec.defaultAlias });
      setPing(result);
      if (!result.ok) {
        setErr(t("connect.err.savedPingFailed", { error: result.error ?? t("connect.err.noDetail") }));
      } else {
        setKey(""); // limpia input solo si pingó bien
      }
      onSaved();
    } catch (e) {
      setErr(t("connect.err.connect", { error: String(e) }));
    } finally {
      setBusy(false);
    }
  };

  const handleDelete = async () => {
    if (!confirm(t("connect.deleteConfirm", { label }))) return;
    setBusy(true);
    try {
      await invoke<boolean>("provider_delete", { alias: spec.defaultAlias });
      onSaved();
      setPing(null);
    } catch (e) {
      setErr(String(e));
    } finally {
      setBusy(false);
    }
  };

  const handleRetest = async () => {
    if (busy || !existing) return;
    setBusy(true); setErr(null);
    try {
      const result = await invoke<PingResult>("provider_test", { alias: existing.alias });
      setPing(result);
      if (!result.ok) setErr(t("connect.err.pingFailed", { error: result.error ?? "" }));
      onSaved();
    } catch (e) { setErr(String(e)); }
    finally { setBusy(false); }
  };

  return (
    <div className="connect-pane">
      <h3>{label}</h3>
      <p className="muted small">{helpLine}</p>

      {existing && (
        <div className="key-row">
          <StatusDot status={existing.status} />
          <code>{existing.alias}</code>
          <span className="muted small">
            {existing.status} {existing.last_ping_ms !== null && `· ${existing.last_ping_ms} ms`}
          </span>
          <span className="actions">
            <button onClick={handleRetest} disabled={busy}>Re-test</button>
            <Button variant="danger" onClick={handleDelete} disabled={busy}>{t("connect.delete")}</Button>
          </span>
        </div>
      )}

      {/* 060 v1 — instructivo paso a paso (colapsable), sólo si el spec lo trae y aún no hay key. */}
      {spec.guideKeys && !existing && (
        <details className="connect-guide">
          <summary>{t("connect.guide.summary")} <span className="cg-time">{helpLine.split("·").pop()?.trim()}</span></summary>
          <ol>
            {spec.guideKeys.map((k) => <li key={k}>{t(k)}</li>)}
          </ol>
          <button className="cg-signup" onClick={openSignup}>{t("connect.guide.openSignup", { label })}</button>
        </details>
      )}

      <div className="form-row">
        <label>{existing ? t("connect.replaceKey") : t("connect.apiKey")}</label>
        <div className="form-input">
          <input
            type="password"
            placeholder={spec.keyPlaceholder}
            value={key}
            onChange={(e) => setKey(e.target.value)}
            autoComplete="off"
            spellCheck={false}
            disabled={busy}
          />
          <Button variant="primary" onClick={handleConnect} disabled={busy || !key.trim()}>
            {busy ? t("connect.connecting") : t("connect.connect")}
          </Button>
        </div>
      </div>

      {ping?.ok && (
        <div className="card-block info">
          {t("connect.testPassed", { ms: ping.latency_ms })} <code>{ping.model ?? "n/a"}</code>
        </div>
      )}
      {err && <div className="card-block warn">{err}</div>}

      {/* 060 v1 — el "Abrir signup" de abajo SÓLO si NO se muestra el instructivo (que ya trae el suyo),
          para no duplicar el CTA (audit codex: redundancia visual). */}
      {!(spec.guideKeys && !existing) && (
        <div className="wizard-actions">
          <button onClick={openSignup}>{t("connect.openSignup")}</button>
        </div>
      )}
    </div>
  );
}

function MultiProviderPane({
  title, subtitle, specs, creds, presetMember, onSaved,
}: {
  title: string; subtitle: string;
  specs: ProviderSpec[];
  creds: ProviderCredential[];
  presetMember: string;
  onSaved: () => void;
}) {
  return (
    <div className="connect-pane">
      <h3>{title}</h3>
      <p className="muted small">{subtitle}</p>
      <div className="provider-grid">
        {specs.map((spec) => (
          <SingleProviderPane
            key={spec.kind}
            spec={spec}
            existing={creds.find((c) => c.alias === spec.defaultAlias)}
            presetMember={presetMember}
            onSaved={onSaved}
          />
        ))}
      </div>
    </div>
  );
}

function LocalPane({ creds, onSaved }: { creds: ProviderCredential[]; onSaved: () => void }) {
  const t = useT();
  const [scan, setScan] = useState<LocalScan | null>(null);
  const [scanning, setScanning] = useState(false);
  const [err, setErr] = useState<string | null>(null);

  const runScan = async () => {
    if (scanning) return;
    setScanning(true); setErr(null);
    try {
      const result = await invoke<LocalScan>("provider_local_scan");
      setScan(result);
    } catch (e) {
      setErr(String(e));
    } finally {
      setScanning(false);
    }
  };

  useEffect(() => {
    // Council EDGE_1: scan on tab open (not boot).
    runScan();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const persistLocal = async (alias: string, providerKind: ProviderKind, endpoint: string) => {
    try {
      const req: PersistRequest = {
        alias,
        provider: providerKind,
        key: null,
        endpoint_url: endpoint,
        scope_workspace: null,
        preset_member: "local,mix",
      };
      await invoke<ProviderCredential>("provider_persist", { req });
      await invoke<PingResult>("provider_test", { alias });
      onSaved();
    } catch (e) { setErr(String(e)); }
  };

  return (
    <div className="connect-pane">
      <h3>{t("connect.local.title")}</h3>
      <p className="muted small">{t("connect.local.subtitle")}</p>

      <div className="wizard-actions" style={{ justifyContent: "flex-start" }}>
        <button onClick={runScan} disabled={scanning}>
          {scanning ? t("connect.local.scanning") : t("connect.local.scan")}
        </button>
      </div>

      {err && <div className="card-block warn">{err}</div>}

      {scan && (
        <div className="local-grid">
          <LocalCard
            title="Ollama"
            info={scan.ollama}
            url="https://ollama.com/download"
            existing={creds.find((c) => c.alias === "ollama-local")}
            onConnect={() => persistLocal("ollama-local", "ollama", scan.ollama.endpoint)}
          />
          <LocalCard
            title="LM Studio"
            info={scan.lmstudio}
            url="https://lmstudio.ai/"
            existing={creds.find((c) => c.alias === "lmstudio-local")}
            onConnect={() => persistLocal("lmstudio-local", "lmstudio", scan.lmstudio.endpoint)}
          />
          <LocalCard
            title="llama.cpp server"
            info={scan.llamacpp}
            url="https://github.com/ggerganov/llama.cpp"
            existing={creds.find((c) => c.alias === "llamacpp-local")}
            onConnect={() => persistLocal("llamacpp-local", "llamacpp", scan.llamacpp.endpoint)}
          />
        </div>
      )}

      {scan && !scan.ollama.alive && !scan.lmstudio.alive && !scan.llamacpp.alive && (
        <p className="muted small">
          {t("connect.local.nonePre")} <a onClick={() => openExternal("https://ollama.com/download")}>Ollama</a>{" "}
          {t("connect.local.noneOr")} <a onClick={() => openExternal("https://lmstudio.ai/")}>LM Studio</a>{" "}
          {t("connect.local.nonePost")}
        </p>
      )}
    </div>
  );
}

function LocalCard({
  title, info, url, existing, onConnect,
}: {
  title: string;
  info: { kind: string; alive: boolean; endpoint: string; models: string[]; latency_ms: number; error: string | null };
  url: string;
  existing?: ProviderCredential;
  onConnect: () => void;
}) {
  const t = useT();
  return (
    <div className="local-card">
      <div className="local-card-head">
        <strong>{title}</strong>
        <StatusDot status={info.alive ? "healthy" : "unconfigured"} />
      </div>
      <div className="muted small"><code>{info.endpoint}</code></div>
      {info.alive ? (
        <>
          <p className="muted small">{t("connect.local.models", { ms: info.latency_ms, count: info.models.length })}</p>
          {info.models.length > 0 && (
            <ul className="models-list">
              {info.models.slice(0, 5).map((m) => (
                <li key={m}><code>{m}</code></li>
              ))}
              {info.models.length > 5 && (
                <li className="muted">{t("connect.local.more", { count: info.models.length - 5 })}</li>
              )}
            </ul>
          )}
          {existing ? (
            <div className="card-block info small">{t("connect.local.connectedAs")} <code>{existing.alias}</code></div>
          ) : (
            <Button variant="primary" onClick={onConnect}>{t("connect.connect")}</Button>
          )}
        </>
      ) : (
        <>
          <p className="muted small">{info.error ?? t("connect.local.notDetected")}</p>
          <a onClick={() => openExternal(url)}>{t("connect.local.install")}</a>
        </>
      )}
    </div>
  );
}

function ProxyPane({ creds, onSaved }: { creds: ProviderCredential[]; onSaved: () => void }) {
  const t = useT();
  const [url, setUrl] = useState("");
  const [key, setKey] = useState("");
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<string | null>(null);

  const existing = creds.find((c) => c.alias === "litellm-proxy" || c.alias === "custom-proxy");

  const handleConnect = async () => {
    if (busy) return;
    setErr(null);
    if (!url.trim()) { setErr(t("connect.err.pasteUrl")); return; }
    // HIGH fix (Gemini B2 audit): use URL parser to validate host.
    // `startsWith("http://localhost")` was bypassable with `http://localhost.attacker.com`.
    let parsed: URL;
    try { parsed = new URL(url.trim()); }
    catch { setErr(t("connect.err.invalidUrl")); return; }
    const httpsOk = parsed.protocol === "https:";
    const loopback = (parsed.protocol === "http:") &&
      (parsed.hostname === "localhost" || parsed.hostname === "127.0.0.1" || parsed.hostname === "[::1]");
    if (!httpsOk && !loopback) {
      setErr(t("connect.err.urlScheme"));
      return;
    }
    setBusy(true);
    try {
      const req: PersistRequest = {
        alias: "litellm-proxy",
        provider: "litellm",
        key: key.trim() || null,
        endpoint_url: url.trim(),
        scope_workspace: null,
        preset_member: "mix",
      };
      await invoke<ProviderCredential>("provider_persist", { req });
      const result = await invoke<PingResult>("provider_test", { alias: "litellm-proxy" });
      if (!result.ok) setErr(t("connect.err.savedPingFailed", { error: result.error ?? "" }));
      onSaved();
    } catch (e) { setErr(String(e)); }
    finally { setBusy(false); }
  };

  return (
    <div className="connect-pane">
      <h3>{t("connect.proxy.title")}</h3>
      <p className="muted small">{t("connect.proxy.subtitle")}</p>

      {existing && (
        <div className="card-block info">
          {t("connect.proxy.already")} <code>{existing.endpoint_url}</code>
        </div>
      )}

      <div className="form-row">
        <label>{t("connect.proxy.urlLabel")}</label>
        <div className="form-input">
          <input
            type="url"
            placeholder="https://litellm.your-org.com/v1"
            value={url}
            onChange={(e) => setUrl(e.target.value)}
            disabled={busy}
          />
        </div>
      </div>
      <div className="form-row">
        <label>{t("connect.proxy.keyLabel")}</label>
        <div className="form-input">
          <input
            type="password"
            placeholder="sk-..."
            value={key}
            onChange={(e) => setKey(e.target.value)}
            disabled={busy}
            autoComplete="off"
          />
          <Button variant="primary" onClick={handleConnect} disabled={busy || !url.trim()}>
            {busy ? t("connect.connecting") : t("connect.connect")}
          </Button>
        </div>
      </div>
      {err && <div className="card-block warn">{err}</div>}
    </div>
  );
}

function StatusDot({ status }: { status: string }) {
  const cls = status === "healthy" ? "green"
    : status === "amber" ? "amber"
    : status === "red" ? "red" : "gray";
  return <span className={`status-dot ${cls}`} title={status} />;
}
