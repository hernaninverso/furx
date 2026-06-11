"use client";

import { useState } from "react";

const API = process.env.NEXT_PUBLIC_API_URL || "https://api.furx.cloud";

/**
 * Funnel email capture. Posts to furx-api /v1/leads with UTM + referrer.
 * Idempotent server-side (no enumeration); we always show success on 2xx.
 */
export default function LeadCapture({ source = "landing-cta" }: { source?: string }) {
  const [email, setEmail] = useState("");
  const [state, setState] = useState<"idle" | "loading" | "done" | "error">("idle");

  async function submit(e: React.FormEvent) {
    e.preventDefault();
    const value = email.trim();
    if (!/^[^\s@]+@[^\s@]+\.[^\s@]{2,}$/.test(value)) {
      setState("error");
      return;
    }
    setState("loading");
    try {
      const params = new URLSearchParams(window.location.search);
      const res = await fetch(`${API}/v1/leads`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({
          email: value,
          source,
          utm: {
            source: params.get("utm_source") ?? undefined,
            medium: params.get("utm_medium") ?? undefined,
            campaign: params.get("utm_campaign") ?? undefined,
          },
          referrer: document.referrer || undefined,
        }),
      });
      setState(res.ok ? "done" : "error");
    } catch {
      setState("error");
    }
  }

  if (state === "done") {
    return (
      <p className="text-sm text-accent font-mono mt-2">
        ✓ You&apos;re on the list — we&apos;ll email you when the public build drops.
      </p>
    );
  }

  return (
    <form onSubmit={submit} className="flex flex-wrap gap-2 justify-center items-center mt-2">
      <input
        type="email"
        required
        value={email}
        onChange={(e) => { setEmail(e.target.value); if (state === "error") setState("idle"); }}
        placeholder="you@company.com"
        aria-label="Email for launch updates"
        className="bg-panel border border-rule rounded-lg px-4 py-2.5 text-sm text-ink min-w-[16rem] focus:border-accent outline-none"
      />
      <button type="submit" disabled={state === "loading"} className="btn-primary">
        {state === "loading" ? "…" : "Notify me at launch"}
      </button>
      {state === "error" && (
        <span className="text-xs text-err w-full text-center">Enter a valid email and try again.</span>
      )}
    </form>
  );
}
