"use client";
import Link from "next/link";
import { useEffect, useState } from "react";

// V3 dark/light toggle. Flips `.dark` on <html>, persists to localStorage; the
// anti-FOUC script in layout applies it before paint on the next load.
function ThemeToggle({ className = "" }: { className?: string }) {
  const [dark, setDark] = useState(false);
  useEffect(() => { setDark(document.documentElement.classList.contains("dark")); }, []);
  function toggle() {
    const next = !dark;
    setDark(next);
    document.documentElement.classList.toggle("dark", next);
    try { localStorage.setItem("furx-theme", next ? "dark" : "light"); } catch { /* ignore */ }
  }
  return (
    <button
      type="button"
      onClick={toggle}
      className={`inline-flex items-center gap-1.5 font-mono text-xs uppercase tracking-wider text-ink-3 hover:text-ink transition-colors ${className}`}
      title={dark ? "Switch to light" : "Switch to dark"}
      aria-label="Toggle theme"
    >
      <span className="inline-block w-3.5 text-center text-sm">{dark ? "◐" : "◑"}</span>
      <span>{dark ? "dark" : "light"}</span>
    </button>
  );
}

const APP_URL = process.env.NEXT_PUBLIC_APP_URL || "https://app.furx.cloud";
const GH_REPO = process.env.NEXT_PUBLIC_GH_REPO || "https://github.com/hernaninverso/furx";

const LINKS = [
  { href: "/council-mode/", label: "Council Mode" },
  { href: "/providers/", label: "Providers" },
  { href: "/pricing/", label: "Pricing" },
  { href: "/docs/", label: "Docs" },
  { href: "/changelog/", label: "Changelog" },
  { href: "/download/", label: "Download" },
];

interface Props { locale?: "en" | "es" }

export default function Navbar({ locale = "en" }: Props) {
  void locale;
  const [open, setOpen] = useState(false);

  return (
    <nav className="sticky top-0 z-40 bg-bg-blur backdrop-blur border-b border-rule" aria-label="Main">
      <div className="max-w-wide mx-auto px-6 h-16 flex items-center justify-between">
        <Link href="/" className="flex items-center gap-2.5 font-display text-base text-ink" aria-label="Furx home">
          <span className="brand-mark">F</span>
          <span>Furx</span>
        </Link>
        <button
          type="button"
          onClick={() => setOpen(!open)}
          className="md:hidden text-ink-2 hover:text-ink p-1"
          aria-label="Toggle navigation"
          aria-expanded={open}
        >
          <svg width="22" height="22" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
            <line x1="3" y1="6" x2="21" y2="6" />
            <line x1="3" y1="12" x2="21" y2="12" />
            <line x1="3" y1="18" x2="21" y2="18" />
          </svg>
        </button>
        <div className="hidden md:flex items-center gap-6 text-sm">
          {LINKS.map((l) => (
            <Link key={l.href} href={l.href} className="text-ink-2 hover:text-ink transition-colors font-medium">
              {l.label}
            </Link>
          ))}
          <a href={GH_REPO} target="_blank" rel="noopener noreferrer" className="text-ink-2 hover:text-ink font-medium">
            GitHub
          </a>
          <a href={APP_URL} className="text-ink-2 hover:text-ink font-medium">Sign in</a>
          <ThemeToggle />
          <Link href="/download/" className="btn-primary text-sm py-2">
            Download <span className="kbd">⌘D</span>
          </Link>
        </div>
      </div>
      {open && (
        <div className="md:hidden border-t border-rule bg-panel px-6 py-4 flex flex-col gap-3 text-sm">
          {LINKS.map((l) => (
            <Link key={l.href} href={l.href} className="text-ink-2 hover:text-ink font-medium" onClick={() => setOpen(false)}>
              {l.label}
            </Link>
          ))}
          <a href={GH_REPO} target="_blank" rel="noopener noreferrer" className="text-ink-2 hover:text-ink font-medium">GitHub</a>
          <a href={APP_URL} className="text-ink-2 hover:text-ink font-medium">Sign in</a>
          <ThemeToggle className="py-1" />
          <Link href="/download/" className="btn-primary text-sm py-2 self-start">Download <span className="kbd">⌘D</span></Link>
        </div>
      )}
    </nav>
  );
}
