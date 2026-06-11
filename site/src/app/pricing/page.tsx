import type { Metadata } from "next";
import Link from "next/link";

import Footer from "@/components/Footer";
import Navbar from "@/components/Navbar";

export const metadata: Metadata = {
  title: "Pricing",
  description:
    "Furx pricing: Free Apache-2.0 forever (all hotkeys, all panes, all Council voices unlimited, BYOK pure), Pro $12/mo (cloud sync, replay, cost analytics), Team $30/seat/mo, Enterprise $49/seat or $2.5k perpetual. Paddle MoR. 14-day Pro trial without a card.",
  alternates: { canonical: "https://furx.cloud/pricing/" },
};

const TIERS = [
  {
    id: "free",
    name: "Free",
    price: "$0",
    sub: "forever · Apache-2.0 core",
    cta: "Download",
    href: "/download/",
    features: [
      "Multi-pane PTY grid — any number of panes",
      "BYOK to OS Keychain (macOS · Linux · Windows)",
      "Council Mode ⌘J — unlimited voices, all 5 presets",
      "Council Templates by phase (Planning / Implementation / Review / Debug / Refactor)",
      "Memory Hub local (SQLite FTS5 + Knowledge Graph), cross-CLI",
      "Skills Registry cross-CLI (markdown skills)",
      "Voice → active pane (whisper.cpp local)",
      "Mobile bridge (WS server + mDNS, BYO companion)",
      "Append-only audit log in local SQLite",
      "Cost meter basic + latency heatmap basic",
      "⌘B broadcast, ⌘P search, ⌘K palette, ⌘⇧V smart paste, ⌘⇧S snapshot",
      "MCP server health probes",
      "Disagreement detector",
      "Crash capture + auto-update (Tauri minisign)",
    ],
  },
  {
    id: "pro",
    name: "Pro",
    price: "$12",
    sub: "/mo · USD · 14-day trial, no card",
    cta: "Start Pro trial",
    href: "/download/",
    featured: true,
    features: [
      "Everything in Free, ungated",
      "Cloud sync of .mcp.json + skills + Memory Hub backup",
      "Session replay scrubber (30 days retention)",
      "Cost Meter Pro: alerts, CSV export, history",
      "Latency heatmap with 7- and 30-day trends",
      "Encrypted daily backup of Memory Hub",
      "Premium UI themes",
      "Priority issues on private GitHub + Discord channel",
      "Plan→DAG visualizer, auto-bisect, PR description generator",
      "Background job queue (fire-and-forget agents)",
    ],
  },
  {
    id: "team",
    name: "Team",
    price: "$30",
    sub: "/seat/mo · 5–25 seats · annual −15%",
    cta: "Contact sales",
    href: "mailto:sales@furx.cloud?subject=Furx%20Team",
    features: [
      "Everything in Pro",
      "Shared Memory Hub team-wide (org-scoped knowledge graph)",
      "Shared skills registry",
      "SSO / OIDC / SAML",
      "Admin console + policy enforcement",
      "Shared cost dashboard + centralized audit",
      "Provider allow-listing",
      "TOTP 2FA mandatory for admins",
      "Email support 24h business response",
    ],
  },
  {
    id: "enterprise",
    name: "Enterprise",
    price: "$49",
    sub: "/seat/mo · or $2.5k perpetual · min 20 seats",
    cta: "Contact sales",
    href: "mailto:sales@furx.cloud?subject=Furx%20Enterprise",
    features: [
      "Self-hosted notarized build (on-prem)",
      "Data residency Argentina or EU",
      "Source-code escrow (NCC Group)",
      "White-label + custom provider integrations",
      "Dedicated SLA 99.9% on license API",
      "4h business response, Slack/Discord direct",
    ],
  },
];

const COMPLIANCE = {
  name: "Compliance Pack",
  price: "$199",
  sub: "one-time · Team+ / Enterprise add-on",
  features: [
    "GDPR / DPA template signed",
    "Encrypted audit backup (AES-256-GCM)",
    "3-year legal escrow audit log",
    "SOC2-ready evidence pack",
    "Pre-audit checklist + remediation guidance",
  ],
};

const NEVER_PAY = [
  "Number of Council voices (1, 6, 30 — same $0)",
  "Number of panes",
  "Tokens you send to providers (we never see them)",
  "Provider keys (OS keychain only)",
  "Voice transcription (local whisper.cpp)",
  "Memory Hub local",
  "Skills you write",
  "Mobile bridge over your LAN / Tailscale",
];

const COMPARE = [
  { feat: "Council voices per dispatch", free: "unlimited", pro: "unlimited", team: "unlimited", ent: "unlimited" },
  { feat: "Council Templates by phase", free: "✓ all 5", pro: "✓ + custom", team: "✓ + org-shared", ent: "✓ + custom routers" },
  { feat: "Pane count", free: "unlimited", pro: "unlimited", team: "unlimited", ent: "unlimited" },
  { feat: "Memory Hub", free: "local SQLite + FTS5", pro: "+ cloud backup", team: "+ org-shared", ent: "+ on-prem WORM" },
  { feat: "Skills cross-CLI", free: "local", pro: "+ cloud sync", team: "+ shared", ent: "+ on-prem" },
  { feat: "Voice → active pane", free: "✓ local whisper", pro: "✓", team: "✓", ent: "✓" },
  { feat: "Mobile bridge", free: "✓ self-hosted WS", pro: "✓", team: "✓", ent: "✓" },
  { feat: "Audit log", free: "local forever", pro: "+ cloud sync", team: "+ centralized", ent: "+ tamper-proof" },
  { feat: "Session replay", free: "—", pro: "30 days", team: "90 days", ent: "configurable" },
  { feat: "Devices per seat", free: "1", pro: "3", team: "unlimited", ent: "unlimited" },
  { feat: "Support", free: "GitHub issues", pro: "priority + Discord", team: "email 24h", ent: "Slack 4h" },
  { feat: "Billing", free: "—", pro: "Paddle (cards, IBAN, PayPal)", team: "Paddle / invoice", ent: "PO / wire" },
];

const FAQ_PRICING = [
  {
    q: "Why is Council Mode unlimited on Free?",
    a: "Because the voices are your provider keys. Charging you to dispatch your prompt to more of your providers would mean charging twice for the same thing. We don't.",
  },
  {
    q: "What does the $12 Pro pay for, then?",
    a: "Cloud infra we actually run on Cloudflare: sync of your .mcp.json + skills + Memory Hub backup; session replay scrubber (30 days); Cost Meter Pro with alerts and CSV export; daily encrypted backup; priority support. Things that need a server to exist.",
  },
  {
    q: "Annual billing?",
    a: "Yes. Pro annual is $99/yr (~31% off $12 × 12). Team annual is −15%. Enterprise is annual by default; perpetual one-time also available.",
  },
  {
    q: "Why no usage-based pricing on LLM tokens?",
    a: "Furx is pass-through — we never see your provider keys or token counts. The subscription pays for the orchestration layer, not your inference spend. If you spend $200/mo on Claude API, that's between you and Anthropic.",
  },
  {
    q: "Can I downgrade?",
    a: "Yes, any time. Downgrade is effective at the end of your paid period. Free features remain; sync/replay/backups pause until you upgrade again.",
  },
  {
    q: "Refunds?",
    a: "14-day no-questions refund via Paddle MoR. Compliance Pack one-time is refundable within 14 days only if not downloaded.",
  },
  {
    q: "Sanctions / OFAC?",
    a: "We do not sell to entities in countries under comprehensive sanctions (Cuba, Iran, North Korea, Syria, Crimea region). See AUP.",
  },
  {
    q: "Education / OSS discount?",
    a: "Free tier is full-featured and Apache-2.0 — that's the OSS discount. Pro Edu $6/mo with .edu address on annual plan; contact sales.",
  },
  {
    q: "How does the trial work?",
    a: "First install activates 14 days of Pro automatically. No credit card. On day 15 it auto-reverts to Free, you keep all Free features (which is, again, most things).",
  },
  {
    q: "What about VAT / sales tax?",
    a: "Paddle is the merchant of record. They handle EU VAT, US sales tax, AR Argentina, JP consumption tax automatically. The price you see is the price you pay (tax-inclusive in the EU).",
  },
];

export default function PricingPage() {
  return (
    <>
      <Navbar />
      <main id="main" className="max-w-wide mx-auto px-6 pt-16 pb-24">
        <header className="mb-12">
          <h1 className="text-4xl md:text-5xl font-semibold mb-3 text-balance text-ink tracking-[-0.025em]">
            Pricing, <span className="font-italic-serif text-accent">plain</span>.
          </h1>
          <p className="text-ink-2 text-lg max-w-[64ch] leading-relaxed">
            BYOK pass-through means we don&apos;t mark up your LLM tokens. The subscription pays for the
            cloud infrastructure behind sync, replay and analytics. 14-day Pro trial on first install, no card.
          </p>
        </header>

        {/* What you never pay for */}
        <section className="mb-12 bg-bg-soft border border-rule rounded-lg p-6 md:p-7">
          <h2 className="text-xl font-semibold mb-3 text-ink">What you <span className="font-italic-serif text-accent">never</span> pay for</h2>
          <ul className="grid md:grid-cols-2 gap-x-10 gap-y-2.5 text-sm">
            {NEVER_PAY.map((item) => (
              <li key={item} className="flex items-start gap-3 text-ink">
                <span className="text-accent font-mono mt-1 shrink-0">✓</span>
                <span>{item}</span>
              </li>
            ))}
          </ul>
        </section>

        {/* Tier cards */}
        <div className="grid md:grid-cols-2 lg:grid-cols-4 gap-5 mb-12">
          {TIERS.map((t) => (
            <div key={t.id} className={`bg-panel border rounded-lg p-6 flex flex-col gap-4 ${t.featured ? "border-accent ring-1 ring-accent" : "border-rule"}`}>
              <div>
                <div className="font-mono text-xs uppercase tracking-wider text-ink-3 mb-2">{t.name}</div>
                <div className="font-mono text-4xl font-semibold text-accent">{t.price}</div>
                <div className="text-xs text-ink-3 mt-1">{t.sub}</div>
              </div>
              <ul className="text-sm space-y-2 mt-2 text-ink">
                {t.features.map((f) => (
                  <li key={f} className="flex gap-2"><span className="text-accent mt-0.5">›</span><span>{f}</span></li>
                ))}
              </ul>
              <a href={t.href} className={`mt-4 ${t.featured ? "btn-primary" : "btn-secondary"} text-sm self-start`}>{t.cta}</a>
            </div>
          ))}
        </div>

        {/* Compliance Pack */}
        <section className="bg-panel border border-rule rounded-lg p-6 md:p-8 mb-12 flex flex-col md:flex-row gap-6 items-start">
          <div className="flex-1">
            <div className="font-mono text-xs uppercase tracking-wider text-ink-3 mb-2">Add-on</div>
            <h2 className="text-2xl font-semibold mb-2 text-ink">{COMPLIANCE.name}</h2>
            <div className="font-mono text-3xl text-accent mb-1">{COMPLIANCE.price}</div>
            <div className="text-xs text-ink-3">{COMPLIANCE.sub}</div>
          </div>
          <ul className="flex-1 text-sm text-ink space-y-2">
            {COMPLIANCE.features.map((f) => (
              <li key={f} className="flex gap-2"><span className="text-accent mt-0.5">›</span><span>{f}</span></li>
            ))}
          </ul>
          <a href="mailto:sales@furx.cloud?subject=Furx%20Compliance%20Pack" className="btn-secondary text-sm self-start">Request</a>
        </section>

        {/* Comparison table */}
        <section className="mb-16">
          <h2 className="text-2xl font-semibold mb-6 text-ink">Feature comparison</h2>
          <div className="overflow-x-auto border border-rule rounded-lg bg-panel">
            <table className="w-full text-sm">
              <thead className="bg-bg-soft border-b border-rule">
                <tr>
                  <th className="text-left px-4 py-3 font-mono text-xs uppercase tracking-wider text-ink-3 font-medium">Feature</th>
                  <th className="px-4 py-3 font-mono text-xs uppercase tracking-wider text-ink-3 font-medium">Free</th>
                  <th className="px-4 py-3 font-mono text-xs uppercase tracking-wider text-accent font-semibold">Pro</th>
                  <th className="px-4 py-3 font-mono text-xs uppercase tracking-wider text-ink-3 font-medium">Team</th>
                  <th className="px-4 py-3 font-mono text-xs uppercase tracking-wider text-ink-3 font-medium">Enterprise</th>
                </tr>
              </thead>
              <tbody>
                {COMPARE.map((row, i) => (
                  <tr key={i} className={`border-b border-rule last:border-0 ${i % 2 ? "bg-bg-2" : ""}`}>
                    <td className="px-4 py-3 text-ink-2">{row.feat}</td>
                    <td className="px-4 py-3 text-center text-ink-2 font-mono text-xs">{row.free}</td>
                    <td className="px-4 py-3 text-center text-ink font-mono text-xs font-medium">{row.pro}</td>
                    <td className="px-4 py-3 text-center text-ink-2 font-mono text-xs">{row.team}</td>
                    <td className="px-4 py-3 text-center text-ink-2 font-mono text-xs">{row.ent}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </section>

        {/* FAQ */}
        <section id="faq">
          <h2 className="text-2xl font-semibold mb-6 text-ink">Pricing FAQ</h2>
          <div className="grid md:grid-cols-2 gap-5">
            {FAQ_PRICING.map((f) => (
              <details key={f.q} className="bg-panel border border-rule rounded-lg p-5">
                <summary className="cursor-pointer text-ink font-semibold list-none flex items-start gap-3">
                  <span className="text-accent font-mono mt-0.5">›</span>
                  <span className="flex-1">{f.q}</span>
                </summary>
                <p className="text-ink-2 text-sm mt-3 pl-6 leading-relaxed">{f.a}</p>
              </details>
            ))}
          </div>
        </section>

        <div className="mt-16 text-center text-sm text-ink-3">
          Billing via <a href="https://paddle.com" target="_blank" rel="noopener noreferrer" className="text-accent hover:underline">Paddle</a> (Merchant of Record).
          See <Link href="/refund/" className="text-accent hover:underline">Refund policy</Link> ·{" "}
          <Link href="/terms/" className="text-accent hover:underline">Terms</Link> ·{" "}
          <Link href="/dpa/" className="text-accent hover:underline">DPA</Link>.
        </div>
      </main>
      <Footer />
    </>
  );
}
