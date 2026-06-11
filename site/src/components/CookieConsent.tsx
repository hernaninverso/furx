"use client";

import { useEffect, useState } from "react";
import Link from "next/link";

const STORAGE_KEY = "furx_consent_v1";

export default function CookieConsent() {
  const [visible, setVisible] = useState(false);

  useEffect(() => {
    try {
      const saved = localStorage.getItem(STORAGE_KEY);
      if (!saved) setVisible(true);
    } catch {
      setVisible(true);
    }
  }, []);

  function decide(granted: boolean) {
    try {
      localStorage.setItem(STORAGE_KEY, granted ? "granted" : "denied");
    } catch {}
    setVisible(false);
  }

  if (!visible) return null;

  return (
    <div
      role="dialog"
      aria-label="Cookie consent"
      className="fixed bottom-4 left-4 right-4 md:left-auto md:right-4 md:max-w-md z-50 bg-panel border border-rule-2 rounded-lg p-4 shadow-card text-sm"
    >
      <p className="text-ink-2 mb-3 leading-relaxed">
        Furx uses <strong className="text-ink">only essential cookies</strong> by default. Measurement
        (Plausible, self-hosted, no fingerprinting) is opt-in. See{" "}
        <Link href="/cookies/" className="text-accent hover:underline">Cookie Policy</Link>.
      </p>
      <div className="flex gap-2 justify-end">
        <button type="button" onClick={() => decide(false)} className="btn-secondary text-xs px-3 py-1.5">
          Essentials only
        </button>
        <button type="button" onClick={() => decide(true)} className="btn-primary text-xs px-3 py-1.5">
          Allow measurement
        </button>
      </div>
    </div>
  );
}
