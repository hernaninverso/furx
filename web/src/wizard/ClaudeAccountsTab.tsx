// B9 + B9.1 · Universal CLI Accounts management tab inside ConnectScreen.
// Lista cuentas conectadas (Claude/Codex/Gemini/Aider/etc.) + Add/Verify/Delete +
// abre setup-script en Terminal.
// 2026-06-09 brand wave 4: copy user-facing vía i18n (keys accounts.*).

import { useCallback, useEffect, useState } from "react";
import { invoke } from "../lib/invoke"; // 015 T015: invoke con flujo de aprobación universal
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
  ClaudeAccount,
  ClaudeAccountAddRequest,
  ClaudeAccountVerifyResult,
  CliKind,
} from "../types";
import { CLI_KIND_META } from "../types";
import { Button } from "../components/Button";
import { t as tGlobal, useT } from "../lib/i18n";

interface Props { onChanged?: () => void; }

const BROWSERS = ["Chrome", "Brave", "Arc", "Edge", "Firefox", "Safari"] as const;
const CLI_KINDS_ORDERED: CliKind[] = ["claude", "codex", "gemini", "aider", "openai-api", "custom"];

export function ClaudeAccountsTab({ onChanged }: Props) {
  const t = useT();
  const [accounts, setAccounts] = useState<ClaudeAccount[]>([]);
  const [adding, setAdding] = useState(false);
  const [newSlug, setNewSlug] = useState("");
  const [newBrowser, setNewBrowser] = useState<string>("");
  const [newCliKind, setNewCliKind] = useState<CliKind>("claude");
  const [busyKey, setBusyKey] = useState<string | null>(null);
  const [msg, setMsg] = useState<string | null>(null);
  const [err, setErr] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    try {
      const list = await invoke<ClaudeAccount[]>("claude_accounts_list");
      setAccounts(list);
    } catch (e) {
      setErr(String(e));
    }
  }, []);

  useEffect(() => {
    let mounted = true;
    refresh();
    let unlisten: UnlistenFn | undefined;
    (async () => {
      try {
        unlisten = await listen("claude-accounts:changed", () => { if (mounted) refresh(); });
      } catch { /* ignore */ }
    })();
    return () => { mounted = false; if (unlisten) unlisten(); };
  }, [refresh]);

  const validateSlug = (s: string) => /^[A-Za-z0-9_-]{1,32}$/.test(s);

  const keyOf = (a: ClaudeAccount) => `${a.cli_kind}:${a.slug}`;

  const handleAdd = async () => {
    setErr(null); setMsg(null);
    if (!validateSlug(newSlug)) {
      setErr(t("accounts.err.slug"));
      return;
    }
    // 062 — el nombre (slug) ES el display. Guardamos label=slug por compat de columna (no se muestra
    // un label arbitrario aparte); si en el futuro un CLI expone identidad real (whoami), se enchufa acá.
    const req: ClaudeAccountAddRequest = {
      slug: newSlug,
      label: newSlug,
      cli_kind: newCliKind,
      browser: newBrowser || null,
    };
    try {
      await invoke<ClaudeAccount>("claude_account_add", { req });
      setMsg(t("accounts.added", { label: CLI_KIND_META[newCliKind].label, slug: newSlug }));
      try {
        await invoke<string>("claude_account_run_setup", {
          cliKind: newCliKind,
          slug: newSlug,
        });
      } catch (e) {
        setErr(t("accounts.err.terminal", { error: String(e) }));
      }
      setNewSlug(""); setNewBrowser("");
      setAdding(false);
      await refresh();
      onChanged?.();
    } catch (e) {
      setErr(String(e));
    }
  };

  const handleVerify = async (acc: ClaudeAccount) => {
    const k = keyOf(acc);
    setBusyKey(k); setErr(null); setMsg(null);
    try {
      const res = await invoke<ClaudeAccountVerifyResult>("claude_account_verify", {
        cliKind: acc.cli_kind, slug: acc.slug,
      });
      setMsg(`${acc.cli_kind}/${acc.slug}: ${res.ok ? "✓" : "✗"} ${res.message}`);
      await refresh();
    } catch (e) {
      setErr(String(e));
    } finally {
      setBusyKey(null);
    }
  };

  const handleDelete = async (acc: ClaudeAccount) => {
    if (!confirm(t("accounts.deleteConfirm", { label: CLI_KIND_META[acc.cli_kind].label, slug: acc.slug }))) return;
    const k = keyOf(acc);
    setBusyKey(k); setErr(null);
    try {
      await invoke<boolean>("claude_account_delete", {
        cliKind: acc.cli_kind, slug: acc.slug,
      });
      setMsg(t("accounts.deleted", { slug: acc.slug }));
      await refresh();
      onChanged?.();
    } catch (e) {
      setErr(String(e));
    } finally {
      setBusyKey(null);
    }
  };

  const handleRunSetup = async (acc: ClaudeAccount) => {
    const k = keyOf(acc);
    setBusyKey(k); setErr(null);
    try {
      await invoke<string>("claude_account_run_setup", {
        cliKind: acc.cli_kind, slug: acc.slug,
      });
      setMsg(t("accounts.setupOpened", { kind: acc.cli_kind, slug: acc.slug }));
    } catch (e) {
      setErr(String(e));
    } finally {
      setBusyKey(null);
    }
  };

  // Group accounts by cli_kind
  const grouped: Record<CliKind, ClaudeAccount[]> = {} as Record<CliKind, ClaudeAccount[]>;
  for (const a of accounts) {
    (grouped[a.cli_kind] ||= []).push(a);
  }

  return (
    <div className="connect-pane">
      <h3>{t("accounts.title")}</h3>
      <p className="muted small">{t("accounts.subtitle")}</p>

      {err && <div className="card-block warn" style={{ marginTop: 8 }}>{err}</div>}
      {msg && <div className="card-block info" style={{ marginTop: 8 }}>{msg}</div>}

      <div className="claude-accounts-list">
        {accounts.length === 0 && (
          <div className="muted small" style={{ padding: 14 }}>
            {t("accounts.empty")}
          </div>
        )}
        {CLI_KINDS_ORDERED.map((kind) => {
          const list = grouped[kind] ?? [];
          if (list.length === 0) return null;
          const meta = CLI_KIND_META[kind];
          return (
            <div key={kind}>
              <div className="cli-kind-header">
                <span className="cli-kind-pill" style={{ background: `${meta.color}22`, color: meta.color, borderColor: `${meta.color}55` }}>
                  {meta.label}
                </span>
                <span className="muted small">{list.length} · env <code>{meta.envHint}</code></span>
              </div>
              {list.map((acc) => {
                const k = keyOf(acc);
                return (
                  <div key={k} className="claude-account-row">
                    <span className={`status-dot ${dotClass(acc.status)}`} />
                    <div style={{ flex: 1, minWidth: 0 }}>
                      <div>
                        {/* 062 — el nombre visible es el SLUG (lo que el user puso al agregar la cuenta),
                            no un `label` de texto libre aparte (defaulteaba a "Cuenta 1/2", arbitrario). */}
                        <strong>{acc.slug}</strong>
                        {acc.browser && <span className="muted small" style={{ marginLeft: 8 }}>· {acc.browser}</span>}
                      </div>
                      <div className="muted small">
                        {acc.status === "verified" && acc.last_verified_at ? t("accounts.verified", { rel: formatRel(acc.last_verified_at) }) : null}
                        {acc.status === "unverified" ? t("accounts.unverified") : null}
                        {acc.status === "missing_token" ? t("accounts.missingToken") : null}
                        {acc.last_used_at && t("accounts.usedAgo", { rel: formatRel(acc.last_used_at) })}
                      </div>
                    </div>
                    <span className="actions">
                      {acc.status === "missing_token" ? (
                        <button onClick={() => handleRunSetup(acc)} disabled={busyKey === k}>
                          Setup
                        </button>
                      ) : (
                        <button onClick={() => handleVerify(acc)} disabled={busyKey === k}>
                          Verify
                        </button>
                      )}
                      <button onClick={() => handleRunSetup(acc)} disabled={busyKey === k} title="Re-run setup (overwrite token)">
                        ↻
                      </button>
                      <Button variant="danger" onClick={() => handleDelete(acc)} disabled={busyKey === k}>
                        {t("connect.delete")}
                      </Button>
                    </span>
                  </div>
                );
              })}
            </div>
          );
        })}
      </div>

      {!adding ? (
        <div className="wizard-actions" style={{ marginTop: 14 }}>
          <Button variant="primary" onClick={() => setAdding(true)}>{t("accounts.add")}</Button>
        </div>
      ) : (
        <div className="claude-account-add-form">
          <h4 style={{ marginBottom: 12, fontSize: 13 }}>{t("accounts.new")}</h4>
          <div className="form-row">
            <label>{t("accounts.cliLabel")}</label>
            <div className="form-input">
              <select
                value={newCliKind}
                onChange={(e) => setNewCliKind(e.target.value as CliKind)}
                style={{ flex: 1 }}
              >
                {CLI_KINDS_ORDERED.map((k) => (
                  <option key={k} value={k}>
                    {CLI_KIND_META[k].label} · env {CLI_KIND_META[k].envHint}
                  </option>
                ))}
              </select>
            </div>
          </div>
          <div className="form-row">
            <label>{t("accounts.nameLabel")}</label>
            <div className="form-input">
              <input
                placeholder={t("accounts.namePlaceholder")}
                value={newSlug}
                onChange={(e) => setNewSlug(e.target.value)}
                maxLength={32}
              />
            </div>
          </div>
          <div className="form-row">
            <label>{t("accounts.browserLabel")}</label>
            <div className="form-input">
              <select value={newBrowser} onChange={(e) => setNewBrowser(e.target.value)} style={{ flex: 1 }}>
                <option value="">{t("accounts.autoDetect")}</option>
                {BROWSERS.map((b) => <option key={b} value={b}>{b}</option>)}
              </select>
            </div>
          </div>
          <p className="muted small">
            {t("accounts.setupHelpPre")}{" "}
            <code>
              {newCliKind === "claude"
                ? `~/bin/setup-max-account.sh ${newSlug || "<slug>"}`
                : `~/bin/setup-account.sh ${newSlug || "<slug>"} --cli ${newCliKind}`}
            </code>
            {t("accounts.setupHelpMid")} <code>{CLI_KIND_META[newCliKind].envHint}</code>.
          </p>
          <div className="wizard-actions">
            <button onClick={() => { setAdding(false); setErr(null); }}>{t("common.cancel")}</button>
            <Button variant="primary" onClick={handleAdd} disabled={!newSlug.trim()}>
              {t("accounts.addOpen")}
            </Button>
          </div>
        </div>
      )}
    </div>
  );
}

function dotClass(status: string): string {
  switch (status) {
    case "verified": return "green";
    case "unverified": return "amber";
    case "missing_token": return "red";
    default: return "gray";
  }
}

// Helper fuera del componente → usa el `t` standalone (idioma activo del módulo i18n).
function formatRel(iso: string): string {
  try {
    const d = new Date(iso);
    const m = Math.floor((Date.now() - d.getTime()) / 60000);
    if (m < 1) return tGlobal("accounts.rel.now");
    if (m < 60) return tGlobal("accounts.rel.m", { m });
    if (m < 1440) return tGlobal("accounts.rel.h", { h: Math.floor(m / 60) });
    return d.toLocaleDateString();
  } catch {
    return iso;
  }
}
