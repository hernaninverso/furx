import { useEffect, useState } from "react";
import { UsageSummary, AieState, formatTok } from "../types";
import { Pill } from "./Pill";
import { useT } from "../lib/i18n";

interface Props {
  usage: UsageSummary | null; usageStaleAt: number;
  aieState: AieState | null; aieStaleAt: number;
  auditDrawerOpen: boolean;
  onToggleAudit: () => void;
  onOpenSmartPaste: () => void;
  onOpenStandup?: () => void;
  onOpenPr?: () => void;
  onOpenDisagree?: () => void;
  /** 016 US2 — abrir el Help Center (sólo si el flag helpCenter está ON). */
  onOpenHelp?: () => void;
  /** 022 P0b · REFORMA 3 — drill-down del stat de tokens a su detalle de uso/origen. */
  onOpenUsage?: () => void;
}

const STALE_AFTER_MS = 60_000;

export function TopBar({ usage, usageStaleAt, aieState, aieStaleAt, auditDrawerOpen, onToggleAudit, onOpenSmartPaste, onOpenStandup, onOpenPr, onOpenDisagree, onOpenHelp, onOpenUsage }: Props) {
  const t = useT(); // 022 P0c · US5 — copy de la topbar vía catálogo i18n (sentence-case).
  // BLOQUE A · F26 — force re-eval of stale flags every 5s even when no new
  // data arrives. Without this, a poll that quietly fails (e.g. AIE down)
  // leaves `staleAt` frozen and the UI never flips to `stale` styling.
  // Implementation choice: lightweight ticker over StaleWatch wrapper here
  // because the TopBar already owns the boolean computation; the dedicated
  // StaleWatch component is reserved for greenfield call-sites.
  const [now, setNow] = useState<number>(Date.now());
  useEffect(() => {
    const id = window.setInterval(() => setNow(Date.now()), 5000);
    return () => window.clearInterval(id);
  }, []);
  const usageStale = usageStaleAt > 0 && now - usageStaleAt > STALE_AFTER_MS;
  const aieStale = aieStaleAt > 0 && now - aieStaleAt > STALE_AFTER_MS;
  const totalTok = usage ? formatTok(usage.total_tokens) : "—";
  const tokenTitle = usage
    ? t("chrome.topbar.tokensTitle", { tokens: usage.total_tokens.toLocaleString(), sessions: usage.source_files })
    : t("chrome.topbar.tokensNoData");
  const tokenDetailTitle = usage
    ? t("chrome.topbar.tokensDetailTitle", { tokens: usage.total_tokens.toLocaleString(), sessions: usage.source_files })
    : t("chrome.topbar.tokensNoData");
  return (
    <div className="topbar" data-tour="topbar">
      {/* 022 P0b · REFORMA 3 — el stat de tokens es una PUERTA a su detalle de uso
          (AIE providers / consumo), no sólo un hover. Tooltip rico + destino real. */}
      {onOpenUsage
        ? <Pill label={t("chrome.topbar.tokens")} value={totalTok} stale={usageStale} title={tokenDetailTitle} onClick={onOpenUsage} ariaLabel={t("chrome.topbar.tokensDetailAria", { detail: tokenTitle })} />
        : <Pill label={t("chrome.topbar.tokens")} value={totalTok} stale={usageStale} title={tokenTitle} />}
      <AiePill aieState={aieState} stale={aieStale} />
      <BurnPill usage={usage} />
      <span className="topbar-spacer" />
      {onOpenHelp && (
        <span data-tour="topbar-help">
          <Pill value={`? ${t("chrome.topbar.help")}`} onClick={onOpenHelp} title={t("chrome.topbar.helpTitle")} ariaLabel={t("chrome.topbar.helpAria")} />
        </span>
      )}
      <ThemeTogglePill />
      {onOpenStandup && (
        <Pill value={`✱ ${t("chrome.topbar.standup")}`} onClick={onOpenStandup} title={t("chrome.topbar.standupTitle")} ariaLabel={t("chrome.topbar.standupAria")} />
      )}
      {onOpenPr && (
        <Pill value={`⇪ ${t("chrome.topbar.pr")}`} onClick={onOpenPr} title={t("chrome.topbar.prTitle")} ariaLabel={t("chrome.topbar.prAria")} />
      )}
      {onOpenDisagree && (
        <Pill value={`⚖ ${t("chrome.topbar.disagree")}`} onClick={onOpenDisagree} title={t("chrome.topbar.disagreeTitle")} ariaLabel={t("chrome.topbar.disagreeAria")} />
      )}
      <Pill value={`📋 ${t("chrome.topbar.paste")}`} onClick={onOpenSmartPaste} title={t("chrome.topbar.pasteTitle")} ariaLabel={t("chrome.topbar.pasteAria")} />
      <Pill
        value={`≡ ${t("chrome.topbar.audit")}`}
        onClick={onToggleAudit}
        active={auditDrawerOpen}
        title={t("chrome.topbar.auditTitle")}
        ariaLabel={t("chrome.topbar.auditAria")}
        ariaPressed={auditDrawerOpen}
      />
    </div>
  );
}

// V3 dark/light toggle. Flips `.dark` on <html> and persists to localStorage;
// the anti-FOUC script in index.html applies it before paint on next launch.
function ThemeTogglePill() {
  const t = useT();
  const [dark, setDark] = useState<boolean>(
    typeof document !== "undefined" && document.documentElement.classList.contains("dark"),
  );
  function toggle() {
    const next = !dark;
    setDark(next);
    document.documentElement.classList.toggle("dark", next);
    try { localStorage.setItem("furx-theme", next ? "dark" : "light"); } catch { /* ignore */ }
  }
  return (
    <Pill
      value={dark ? `◐ ${t("chrome.topbar.themeDark")}` : `◑ ${t("chrome.topbar.themeLight")}`}
      onClick={toggle}
      title={dark ? t("chrome.topbar.themeToLight") : t("chrome.topbar.themeToDark")}
      ariaLabel={t("chrome.topbar.themeAria")}
    />
  );
}

function BurnPill({ usage }: { usage: UsageSummary | null }) {
  const t = useT();
  if (!usage || usage.burn_24h_tokens === 0) return null;
  const burn = usage.burn_24h_tokens;
  const fmt = burn < 1000 ? String(burn) : burn < 1_000_000 ? `${(burn/1000).toFixed(1)}k` : `${(burn/1_000_000).toFixed(2)}M`;
  return (
    <Pill
      label={t("chrome.topbar.burn")}
      value={fmt}
      title={t("chrome.topbar.burnTitle", { burn: burn.toLocaleString(), week: usage.burn_7d_tokens.toLocaleString() })}
    />
  );
}

function AiePill({ aieState, stale }: { aieState: AieState | null; stale: boolean }) {
  const t = useT();
  if (!aieState) return <Pill label={t("chrome.topbar.aie")} value="—" stale={stale} />;
  const hp = aieState.healthy_providers;
  const blocked = aieState.blocked_providers.length;
  const tone = blocked > 0 ? (hp.length === 0 ? "danger" : "warning") : "default";
  const top = hp[0] ?? "?";
  const title = t("chrome.topbar.aieHealthyTitle", {
    healthy: hp.join(", ") || "—",
    blocked: aieState.blocked_providers.join(", ") || "—",
  });
  return (
    <Pill
      label={t("chrome.topbar.aie")}
      value={
        <>
          {top}
          {blocked > 0 && <span className="pill-extra"> · {t("chrome.topbar.aieBlocked", { count: blocked })}</span>}
        </>
      }
      tone={tone}
      stale={stale}
      title={title}
    />
  );
}
