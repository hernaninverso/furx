import { useCallback, useEffect, useLayoutEffect, useRef, useState, ReactNode } from "react";
import { useT } from "../lib/i18n";

export type CardsRailMode = "auto" | "collapsed" | "expanded";

const STORAGE_KEY = "furx.cards-rail.v1";
const VALID_MODES: CardsRailMode[] = ["auto", "collapsed", "expanded"];

function readMode(): { mode: CardsRailMode; raw: string | null } {
  try {
    if (typeof localStorage === "undefined") return { mode: "auto", raw: null };
    const raw = localStorage.getItem(STORAGE_KEY);
    if (raw && (VALID_MODES as string[]).includes(raw)) return { mode: raw as CardsRailMode, raw };
    return { mode: "auto", raw };
  } catch {
    return { mode: "auto", raw: null };
  }
}

function writeMode(mode: CardsRailMode): void {
  try {
    if (typeof localStorage === "undefined") return;
    localStorage.setItem(STORAGE_KEY, mode);
  } catch {
    // silent
  }
}

function clampCount(n: number): string {
  if (!Number.isFinite(n) || n <= 0) return "0";
  if (n > 99) return "99+";
  return String(n);
}

export interface CardsRailProps {
  openCount: number;
  children: ReactNode;
  onCollapsedChange?: (collapsed: boolean) => void;
}

export function CardsRail({ openCount, children, onCollapsedChange }: CardsRailProps) {
  const t = useT(); // 022 P0c · US5 — copy de la chrome del rail vía catálogo i18n.
  const initial = useRef(readMode());
  const [mode, setMode] = useState<CardsRailMode>(initial.current.mode);
  const stripRef = useRef<HTMLButtonElement | null>(null);
  const contentRef = useRef<HTMLDivElement | null>(null);
  const lastAnnouncedCount = useRef<number>(openCount);
  const [announceText, setAnnounceText] = useState("");

  // Codex MED: if storage held an invalid value, normalize it once on mount.
  useEffect(() => {
    const { raw } = initial.current;
    if (raw !== null && !(VALID_MODES as string[]).includes(raw)) {
      writeMode("auto");
    }
  }, []);

  // Cross-tab sync.
  useEffect(() => {
    if (typeof window === "undefined") return;
    const handler = (e: StorageEvent) => {
      if (e.key !== STORAGE_KEY) return;
      setMode(readMode().mode);
    };
    window.addEventListener("storage", handler);
    return () => window.removeEventListener("storage", handler);
  }, []);

  // Pure state→storage sync.
  const prevModeRef = useRef<CardsRailMode | null>(null);
  useEffect(() => {
    const prev = prevModeRef.current;
    prevModeRef.current = mode;
    if (prev === null) return;
    if (prev !== mode) writeMode(mode);
  }, [mode]);

  const isCollapsed =
    mode === "collapsed" || (mode === "auto" && openCount === 0);

  // Lift state up so parent can set a class/attr on .shell (`:has()` fallback).
  // Codex LOW v2: useLayoutEffect so the shell attribute is committed before paint
  // — avoids a first-frame flash where shell=false but rail rendered collapsed.
  const lastReportedCollapsed = useRef<boolean | null>(null);
  useLayoutEffect(() => {
    if (lastReportedCollapsed.current === isCollapsed) return;
    lastReportedCollapsed.current = isCollapsed;
    onCollapsedChange?.(isCollapsed);
  }, [isCollapsed, onCollapsedChange]);

  // Focus return when collapsing while focus is inside content.
  useEffect(() => {
    if (!isCollapsed) return;
    const active = typeof document !== "undefined" ? document.activeElement : null;
    if (active && contentRef.current && contentRef.current.contains(active)) {
      stripRef.current?.focus();
    }
  }, [isCollapsed]);

  // Announce incident count changes while collapsed.
  useEffect(() => {
    if (!isCollapsed) {
      lastAnnouncedCount.current = openCount;
      return;
    }
    if (openCount !== lastAnnouncedCount.current && openCount > 0) {
      setAnnounceText(t("chrome.rail.heading", { count: openCount }));
    }
    lastAnnouncedCount.current = openCount;
  }, [openCount, isCollapsed]);

  const toggle = useCallback(() => {
    setMode((prev) => (prev === "collapsed" || (prev === "auto" && openCount === 0) ? "expanded" : "collapsed"));
  }, [openCount]);

  const countLabel = clampCount(openCount);
  const contentId = "cards-rail-content";

  return (
    <aside
      className={`cards-rail ${isCollapsed ? "cards-rail--collapsed" : "cards-rail--expanded"}`}
      data-mode={mode}
    >
      <div className="cards-rail-strip" aria-hidden={!isCollapsed} {...(isCollapsed ? {} : { inert: true })}>
        <button
          ref={stripRef}
          type="button"
          className="rail-strip-btn"
          onClick={toggle}
          aria-label={openCount > 0 ? t("chrome.rail.expandAriaCount", { count: openCount }) : t("chrome.rail.expandAria")}
          aria-expanded={false}
          aria-controls={contentId}
          title={openCount > 0 ? t("chrome.rail.expandTitleCount", { count: openCount }) : t("chrome.rail.expandTitle")}
        >
          <span className="rail-strip-glyph" aria-hidden="true">◇</span>
          {openCount > 0 && <span className="rail-strip-badge">{countLabel}</span>}
        </button>
      </div>
      <div
        ref={contentRef}
        id={contentId}
        className="cards-rail-content"
        aria-hidden={isCollapsed}
        inert={isCollapsed}
      >
        <div className="rail-header">
          <span>{t("chrome.rail.heading", { count: openCount })}</span>
          <button
            type="button"
            className="rail-collapse-btn"
            onClick={toggle}
            aria-label={t("chrome.rail.collapseAria")}
            aria-expanded={true}
            aria-controls={contentId}
            title={t("chrome.rail.collapseTitle")}
          >
            ›
          </button>
        </div>
        {children}
      </div>
      <div className="sr-only" aria-live="polite" aria-atomic="true">{announceText}</div>
    </aside>
  );
}

export const __test__ = {
  STORAGE_KEY,
  VALID_MODES,
  readMode,
  writeMode,
  clampCount,
};
