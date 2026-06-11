import type { Metadata } from "next";
import LegalLayout from "@/components/LegalLayout";

export const metadata: Metadata = {
  title: "Subprocessors",
  description: "Public, up-to-date list of Furx subprocessors (GDPR Art. 28(2)) — Cloudflare, Paddle, GitHub, Better Stack, opt-in Sentry.",
  alternates: {
    canonical: "https://furx.cloud/subprocessors/",
    types: { "application/rss+xml": [{ url: "/subprocessors/rss.xml", title: "Furx subprocessors changelog" }] },
  },
};

const SUBPROCESSORS = [
  {
    name: "Cloudflare, Inc.",
    purpose: "DNS, CDN, Pages hosting (public site + dashboard), TLS termination, rate limiting, WAF.",
    location: "United States (HQ) + global edge locations · routing data may be replicated in EU/AR/AP regions.",
    dpa: "https://www.cloudflare.com/cloudflare-customer-dpa/",
    scc: "Yes — SCCs 2021/914 Module 2",
  },
  {
    name: "Paddle.com Market Ltd.",
    purpose: "Merchant of Record (MoR): payment processing, invoicing, handling of EU VAT / AR taxes / US sales tax / others.",
    location: "United Kingdom (HQ) + United States.",
    dpa: "https://www.paddle.com/legal/dpa",
    scc: "Yes — SCCs 2021/914 (processor → processor)",
  },
  {
    name: "GitHub, Inc.",
    purpose: "Open-source code hosting (Apache-2.0 core), release distribution (signed binaries), Issues, Discussions.",
    location: "United States.",
    dpa: "https://docs.github.com/en/site-policy/privacy-policies/github-data-protection-agreement",
    scc: "Yes — via the Microsoft DPA",
  },
  {
    name: "Better Stack (BetterStack OÜ)",
    purpose: "External status page (status.furx.cloud), uptime monitoring.",
    location: "Estonia (EU)",
    dpa: "https://betterstack.com/dpa",
    scc: "Covered (intra-EU, SCCs not required)",
  },
  {
    name: "Sentry (Functional Software, Inc.)",
    purpose: "Opt-in crash capture (default OFF). Stack trace + platform + version, PII-scrubbed.",
    location: "United States + DE region selected for EU users",
    dpa: "https://sentry.io/legal/dpa/",
    scc: "Yes — SCCs 2021/914 Module 2",
  },
  {
    name: "Hetzner Online GmbH",
    purpose: "Hosting of the PostgreSQL primary + standby (dashboard data, audit metadata sync, license API).",
    location: "Germany (EU)",
    dpa: "https://www.hetzner.com/legal/data-processing-agreement",
    scc: "Covered (intra-EU, SCCs not required)",
  },
  {
    name: "Fastmail Pty Ltd.",
    purpose: "Transactional email (magic links, invoices via Paddle relay, support). Corporate email @furx.cloud.",
    location: "Australia + United States.",
    dpa: "https://www.fastmail.com/about/dpa.html",
    scc: "Yes — SCCs 2021/914 Module 2",
  },
];

const CHANGES = [
  { date: "2026-05-27", change: "Initial list published (version 1.0)." },
];

export default function SubprocessorsPage() {
  return (
    <LegalLayout title="Subprocessors · Furx">
      <p>
        In accordance with Art. 28(2) of the GDPR and the transparency commitments in our{" "}
        <a href="/privacy/">Privacy Policy</a> and{" "}
        <a href="/dpa/">DPA</a>, we publish the up-to-date list of subprocessors that INVERSO HUB S.R.L.
        uses to provide the Furx service.
      </p>
      <p>
        <strong>Change notifications</strong>: Team / Enterprise customers receive 30 days&apos; advance
        notice before a subprocessor is added or replaced, by email and via the RSS feed{" "}
        <a href="/subprocessors/rss.xml">/subprocessors/rss.xml</a>. The Customer may object
        on reasonable grounds; if the objection is not resolved, the Customer may terminate without penalty.
      </p>

      <h2>Active subprocessors</h2>
      <table>
        <thead>
          <tr><th>Subprocessor</th><th>Purpose</th><th>Data location</th><th>DPA / SCCs</th></tr>
        </thead>
        <tbody>
          {SUBPROCESSORS.map((s) => (
            <tr key={s.name}>
              <td><strong>{s.name}</strong></td>
              <td>{s.purpose}</td>
              <td>{s.location}</td>
              <td>
                <a href={s.dpa} target="_blank" rel="noopener noreferrer">DPA</a><br />
                <small>{s.scc}</small>
              </td>
            </tr>
          ))}
        </tbody>
      </table>

      <h2>Data NOT processed by third parties</h2>
      <p>
        As a reminder, the following User data is <strong>NOT transmitted to any
        subprocessor</strong> (because it NEVER reaches INVERSO HUB S.R.L. infrastructure in the first place):
      </p>
      <ul>
        <li>Prompts sent to LLM providers.</li>
        <li>Responses received from LLM providers.</li>
        <li>API keys for LLM providers (they live in the User&apos;s OS keychain).</li>
        <li>The User&apos;s source code.</li>
        <li>The User&apos;s local audit log.</li>
      </ul>

      <h2>Change history</h2>
      <table>
        <thead><tr><th>Date</th><th>Change</th></tr></thead>
        <tbody>
          {CHANGES.map((c) => (
            <tr key={c.date}><td><code>{c.date}</code></td><td>{c.change}</td></tr>
          ))}
        </tbody>
      </table>

      <p className="text-ink-3 text-sm mt-8">
        To subscribe to change notifications: <a href="/subprocessors/rss.xml">RSS</a>{" "}
        or email <a href="mailto:dpo@furx.cloud?subject=Subprocessor%20notifications">dpo@furx.cloud</a>{" "}
        with the subject &quot;Subprocessor notifications&quot;.
      </p>
    </LegalLayout>
  );
}
