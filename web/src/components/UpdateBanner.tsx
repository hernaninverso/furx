import { useCallback, useEffect, useRef, useState } from "react";
import { check, Update } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";

type Phase = "idle" | "checking" | "available" | "downloading" | "downloaded" | "installing" | "ready" | "error" | "skipped";

interface State {
  phase: Phase;
  update: Update | null;
  progress: { contentLength: number | null; downloaded: number };
  error: string | null;
}

interface SkipRecord {
  skippedVersion: string;
  currentVersion: string;
  skippedAt: string; // ISO
}

const SKIPPED_KEY = "furx.update.skip.v2";
// Skip records age out after 14 days so a re-published same-version update can re-surface.
const SKIP_TTL_MS = 14 * 24 * 60 * 60 * 1000;

function readSkipped(): SkipRecord | null {
  try {
    if (typeof localStorage === "undefined") return null;
    const raw = localStorage.getItem(SKIPPED_KEY);
    if (!raw) return null;
    const parsed = JSON.parse(raw);
    if (typeof parsed !== "object" || parsed === null) return null;
    if (typeof parsed.skippedVersion !== "string"
      || typeof parsed.currentVersion !== "string"
      || typeof parsed.skippedAt !== "string") return null;
    const ts = Date.parse(parsed.skippedAt);
    if (!Number.isFinite(ts)) return null;
    if (Date.now() - ts > SKIP_TTL_MS) {
      localStorage.removeItem(SKIPPED_KEY);
      return null;
    }
    return parsed as SkipRecord;
  } catch {
    return null;
  }
}

function writeSkipped(record: SkipRecord | null) {
  try {
    if (typeof localStorage === "undefined") return;
    if (record === null) localStorage.removeItem(SKIPPED_KEY);
    else localStorage.setItem(SKIPPED_KEY, JSON.stringify(record));
  } catch {
    /* silent */
  }
}

// Plain helper so it can be invoked from anywhere in the file (including
// runCheck's auto-skip path) without React closure ordering issues.
function tryCloseUpdate(u: Update | null) {
  if (!u) return;
  try {
    const maybeClose = (u as unknown as { close?: () => Promise<void> | void }).close;
    if (typeof maybeClose === "function") {
      void Promise.resolve(maybeClose.call(u)).catch(() => { /* ok-to-fail: Tauri Update resource may already be disposed */ });
    }
  } catch {
    /* best-effort */
  }
}

interface Props {
  /** "auto" = check 3s after mount; "manual" = no auto check, only via checkNow. */
  mode?: "auto" | "manual";
  /**
   * Optional gate. If returns true, restart is deferred until user confirms in a
   * caller-provided dialog (Shell can plug active-PTY detection here). When omitted,
   * a confirm() prompt is used.
   */
  confirmRestart?: () => Promise<boolean> | boolean;
}

export function UpdateBanner({ mode = "auto", confirmRestart }: Props) {
  const [state, setState] = useState<State>({
    phase: "idle",
    update: null,
    progress: { contentLength: null, downloaded: 0 },
    error: null,
  });
  const alive = useRef(true);
  const inFlight = useRef(false);

  const safeSetState = useCallback((updater: (prev: State) => State) => {
    if (!alive.current) return;
    setState(updater);
  }, []);

  const runCheck = useCallback(async () => {
    if (!alive.current || inFlight.current) return;
    inFlight.current = true;
    safeSetState((s) => ({ ...s, phase: "checking", error: null }));
    try {
      const update = await check();
      if (!alive.current) { inFlight.current = false; return; }
      if (!update) {
        safeSetState(() => ({ phase: "idle", update: null, progress: { contentLength: null, downloaded: 0 }, error: null }));
        inFlight.current = false;
        return;
      }
      const skipped = readSkipped();
      if (skipped && skipped.skippedVersion === update.version && skipped.currentVersion === update.currentVersion) {
        // Codex LOW v12: close the auto-skipped update before discarding so the
        // Tauri resource doesn't linger for the app lifetime.
        tryCloseUpdate(update);
        safeSetState(() => ({ phase: "skipped", update: null, progress: { contentLength: null, downloaded: 0 }, error: null }));
        inFlight.current = false;
        return;
      }
      safeSetState(() => ({ phase: "available", update, progress: { contentLength: null, downloaded: 0 }, error: null }));
    } catch (e) {
      if (!alive.current) { inFlight.current = false; return; }
      const msg = e instanceof Error ? e.message : String(e);
      // Sprint #5 — when the updater is intentionally disabled (no releases yet,
      // active: false in tauri.conf.json) the plugin throws a "no valid release JSON"
      // error on every check. Treat it as idle, not as a user-visible failure.
      const isDisabledOrNotReleased =
        msg.includes("Could not fetch a valid release JSON") ||
        msg.includes("not a valid release") ||
        msg.includes("update endpoint did not respond") ||
        msg.toLowerCase().includes("404");
      if (isDisabledOrNotReleased) {
        safeSetState(() => ({ phase: "idle", update: null, progress: { contentLength: null, downloaded: 0 }, error: null }));
      } else {
        safeSetState(() => ({
          phase: "error",
          update: null,
          progress: { contentLength: null, downloaded: 0 },
          error: msg,
        }));
      }
    } finally {
      inFlight.current = false;
    }
  }, [safeSetState]);

  useEffect(() => {
    alive.current = true;
    if (mode === "auto") {
      const t = window.setTimeout(() => { void runCheck(); }, 3000);
      return () => { alive.current = false; window.clearTimeout(t); };
    }
    return () => { alive.current = false; };
  }, [mode, runCheck]);

  // Codex MED v9: split download from install. Windows' `downloadAndInstall` quits
  // the app automatically BEFORE we can show the confirm dialog, killing PTYs.
  // Two-step now: download() → confirm → install() + relaunch.
  const downloadOnly = useCallback(async () => {
    if (!state.update || inFlight.current) return;
    inFlight.current = true;
    safeSetState((s) => ({ ...s, phase: "downloading", progress: { contentLength: null, downloaded: 0 } }));
    try {
      let length: number | null = null;
      let downloaded = 0;
      await state.update.download((ev) => {
        if (!alive.current) return;
        switch (ev.event) {
          case "Started":
            length = ev.data.contentLength ?? null;
            downloaded = 0;
            safeSetState((s) => ({ ...s, progress: { contentLength: length, downloaded } }));
            break;
          case "Progress":
            downloaded += ev.data.chunkLength;
            safeSetState((s) => ({ ...s, progress: { contentLength: length, downloaded } }));
            break;
          case "Finished":
            safeSetState((s) => ({ ...s, phase: "downloaded" }));
            break;
        }
      });
      safeSetState((s) => (s.phase === "downloaded" ? s : { ...s, phase: "downloaded" }));
    } catch (e) {
      safeSetState((s) => ({
        ...s,
        phase: "error",
        error: e instanceof Error ? e.message : String(e),
      }));
    } finally {
      inFlight.current = false;
    }
  }, [state.update, safeSetState]);

  const installAndRestart = useCallback(async () => {
    if (!state.update || inFlight.current) return;
    // Confirm BEFORE install (Windows quits the app during install).
    let ok = true;
    try {
      if (confirmRestart) {
        ok = await Promise.resolve(confirmRestart());
      } else if (typeof window !== "undefined" && typeof window.confirm === "function") {
        ok = window.confirm("Install update and restart Furx now? Active terminal/agent sessions will be terminated.");
      }
    } catch (e) {
      console.error("confirmRestart threw", e);
      ok = false;
    }
    if (!ok) return;
    inFlight.current = true;
    safeSetState((s) => ({ ...s, phase: "installing" }));
    try {
      await state.update.install();
      // On Windows install() already quit; relaunch is a no-op there.
      // On macOS/Linux we still need to relaunch.
      await relaunch();
    } catch (e) {
      safeSetState((s) => ({
        ...s,
        phase: "error",
        error: e instanceof Error ? e.message : String(e),
      }));
    } finally {
      inFlight.current = false;
    }
  }, [state.update, safeSetState, confirmRestart]);

  const skip = useCallback(() => {
    if (!state.update) return;
    writeSkipped({
      skippedVersion: state.update.version,
      currentVersion: state.update.currentVersion,
      skippedAt: new Date().toISOString(),
    });
    tryCloseUpdate(state.update);
    safeSetState(() => ({ phase: "skipped", update: null, progress: { contentLength: null, downloaded: 0 }, error: null }));
  }, [state.update, safeSetState]);

  const dismiss = useCallback(() => {
    tryCloseUpdate(state.update);
    safeSetState(() => ({ phase: "idle", update: null, progress: { contentLength: null, downloaded: 0 }, error: null }));
  }, [state.update, safeSetState]);

  // Nothing to render when there's no active update lifecycle.
  if (state.phase === "idle" || state.phase === "skipped" || state.phase === "checking") return null;

  if (state.phase === "error") {
    // Sprint #5 — when the updater endpoint is intentionally unreachable
    // (no releases yet / active:false in tauri.conf.json) the plugin returns one
    // of these well-known error messages. Hide the banner instead of warning the
    // user about an issue they can't act on; the next runCheck() will move the
    // state to idle thanks to the catch-branch above. This is a belt-and-suspenders
    // so any state.error already set in-memory at upgrade time is also suppressed.
    const e = (state.error || "").toLowerCase();
    const benign = e.includes("could not fetch a valid release json")
      || e.includes("not a valid release")
      || e.includes("update endpoint did not respond")
      || e.includes("404");
    if (benign) return null;
    return (
      <div className="update-banner update-banner--error" role="status" aria-live="polite" aria-atomic="true">
        <span className="update-banner-text">Update check failed: <code>{state.error}</code></span>
        <button type="button" className="update-banner-btn" onClick={dismiss} aria-label="Dismiss">×</button>
      </div>
    );
  }

  const v = state.update?.version;
  const current = state.update?.currentVersion;
  const pct = state.progress.contentLength && state.progress.contentLength > 0
    ? Math.min(100, Math.round((state.progress.downloaded / state.progress.contentLength) * 100))
    : null;

  return (
    <div className="update-banner" role="status" aria-live="polite" aria-atomic="true">
      {state.phase === "available" && (
        <>
          <span className="update-banner-text">
            Update available: <strong>v{current}</strong> → <strong>v{v}</strong>
          </span>
          <div className="update-banner-actions">
            <button
              type="button"
              className="update-banner-btn update-banner-btn--primary"
              onClick={() => { void downloadOnly(); }}
              disabled={inFlight.current}
            >
              Download
            </button>
            <button type="button" className="update-banner-btn" onClick={skip}>Skip this version</button>
            <button type="button" className="update-banner-btn" onClick={dismiss}>Later</button>
          </div>
        </>
      )}
      {state.phase === "downloading" && (
        <>
          <span className="update-banner-text">Downloading v{v}…</span>
          <progress
            className="update-banner-progress"
            value={pct === null ? undefined : pct}
            max={100}
            aria-label="Update download progress"
            aria-live="off"
          />
          <span className="update-banner-text muted small" aria-hidden="true">{pct === null ? "…" : `${pct}%`}</span>
        </>
      )}
      {state.phase === "downloaded" && (
        <>
          <span className="update-banner-text">v{v} downloaded — restart to apply.</span>
          <div className="update-banner-actions">
            <button
              type="button"
              className="update-banner-btn update-banner-btn--primary"
              onClick={() => { void installAndRestart(); }}
              disabled={inFlight.current}
            >
              Install &amp; Restart
            </button>
            <button type="button" className="update-banner-btn" onClick={dismiss}>Later</button>
          </div>
        </>
      )}
      {state.phase === "installing" && (
        <span className="update-banner-text">Installing v{v}…</span>
      )}
      {state.phase === "ready" && (
        <>
          <span className="update-banner-text">v{v} installed — restart to apply.</span>
          <div className="update-banner-actions">
            <button
              type="button"
              className="update-banner-btn update-banner-btn--primary"
              onClick={() => { void installAndRestart(); }}
            >
              Restart now
            </button>
            <button type="button" className="update-banner-btn" onClick={dismiss}>Later</button>
          </div>
        </>
      )}
    </div>
  );
}

export const __updaterTest__ = { readSkipped, writeSkipped, SKIPPED_KEY, SKIP_TTL_MS };
