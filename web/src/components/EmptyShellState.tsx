// Empty state del shell (sin panes). 2026-06-09 brand wave 4: copy vía i18n (keys empty.*).
import type { PaneMode } from "../types";
import { useT } from "../lib/i18n";

interface Props {
  onOpenPane: (mode: PaneMode) => void;
  onOpenWizard: () => void;
  hasClaudeAccount: boolean;
  tmuxAvailable: boolean;
  /** 022 LOW — modo Claude REAL de la primera cuenta configurada (ej. "claude-trabajo").
   *  Sin hardcodear "claude-A". Solo se usa si hasClaudeAccount. */
  claudeMode?: PaneMode | null;
  /** Label legible de esa cuenta (derivado), para el texto del botón sin "A/B". */
  claudeLabel?: string | null;
}

export function EmptyShellState({ onOpenPane, onOpenWizard, hasClaudeAccount, tmuxAvailable, claudeMode, claudeLabel }: Props) {
  const t = useT();
  return (
    <div className="empty-shell">
      <div className="empty-welcome">
        <span className="hex hex-lg" aria-hidden="true" />
        <h1 id="welcome-title">{t("empty.welcomeTitle")}</h1>
        <p className="muted" id="welcome-sub">
          {t("empty.subtitle")}
        </p>
      </div>
      <div className="empty-actions-grid" aria-labelledby="welcome-title">
        {hasClaudeAccount && claudeMode && (
          <button
            type="button"
            className="empty-action-card"
            onClick={() => onOpenPane(claudeMode)}
            aria-describedby="hint-claude"
          >
            <span className="empty-action-icon" aria-hidden="true">C</span>
            <span className="empty-action-body">
              <span className="empty-action-title">{t("empty.openClaude")}</span>
              <span className="muted small" id="hint-claude">
                {claudeLabel ? t("empty.hintClaude", { label: claudeLabel }) : t("empty.hintClaudeUnset")}
              </span>
            </span>
            <span className="empty-action-arrow" aria-hidden="true">→</span>
          </button>
        )}
        <button
          type="button"
          className="empty-action-card"
          onClick={() => onOpenPane("zsh" as PaneMode)}
          aria-describedby="hint-zsh"
        >
          <span className="empty-action-icon" aria-hidden="true">&gt;_</span>
          <span className="empty-action-body">
            <span className="empty-action-title">{t("empty.openZsh")}</span>
            <span className="muted small" id="hint-zsh">{t("empty.hintZsh")}</span>
          </span>
          <span className="empty-action-arrow" aria-hidden="true">→</span>
        </button>
        {!hasClaudeAccount && (
          <button
            type="button"
            className="empty-action-card"
            onClick={onOpenWizard}
            aria-describedby="hint-wizard"
          >
            <span className="empty-action-icon" aria-hidden="true">✱</span>
            <span className="empty-action-body">
              <span className="empty-action-title">{t("empty.wizardTitle")}</span>
              <span className="muted small" id="hint-wizard">{t("empty.hintWizard")}</span>
            </span>
            <span className="empty-action-arrow" aria-hidden="true">→</span>
          </button>
        )}
        <button
          type="button"
          className="empty-action-card"
          onClick={onOpenWizard}
          aria-describedby="hint-providers"
        >
          <span className="empty-action-icon" aria-hidden="true">⚙</span>
          <span className="empty-action-body">
            <span className="empty-action-title">{t("empty.providersTitle")}</span>
            <span className="muted small" id="hint-providers">{t("empty.hintProviders")}</span>
          </span>
          <span className="empty-action-arrow" aria-hidden="true">→</span>
        </button>
      </div>
      <div className="empty-shortcuts muted small" aria-hidden="false">
        <kbd>⌘N</kbd> {t("empty.scNew")} · <kbd>⌘K</kbd> {t("empty.scActions")} · <kbd>⌘/</kbd> {t("empty.scAll")}
      </div>
      {!tmuxAvailable && (
        <div className="empty-tmux-hint card-block info" role="status">
          <strong>{t("empty.tmuxMissing")}</strong>{" "}
          {t("empty.tmuxBody")}
          <br />
          <code className="muted small">brew install tmux</code> {t("empty.tmuxReopen")}
        </div>
      )}
    </div>
  );
}
