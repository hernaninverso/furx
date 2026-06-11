import type { Metadata } from "next";
import LegalLayout from "@/components/LegalLayout";

export const metadata: Metadata = {
  title: "Privacy Policy",
  description: "Furx Privacy Policy — INVERSO HUB S.R.L. Compliant with the GDPR, Argentine Law 25,326, and the CCPA. BYOK pass-through, minimal data.",
  alternates: { canonical: "https://furx.cloud/privacy/" },
};

export default function PrivacyPage() {
  return (
    <LegalLayout title="Privacy Policy · Furx">
      <h2>1. Data controller</h2>
      <p>
        <strong>INVERSO HUB S.R.L.</strong><br />
        Registered office: Autonomous City of Buenos Aires, Argentina<br />
        CUIT (Argentine tax ID): registration with AFIP in progress<br />
        DPO contact: <a href="mailto:dpo@furx.cloud">dpo@furx.cloud</a>
      </p>
      <p>
        INVERSO HUB S.R.L. acts as the <strong>data controller</strong> with respect to the
        User&apos;s account and billing data.
      </p>
      <p>
        INVERSO HUB S.R.L. <strong>does NOT act as a processor or controller</strong> of the User&apos;s
        prompts, code, LLM responses, or API keys — that data travels directly from the User&apos;s
        device to the LLM provider the User has chosen, without passing through INVERSO HUB S.R.L. infrastructure.
      </p>

      <h2>2. Data we collect</h2>
      <h3>2.1 Account data (minimum necessary)</h3>
      <ul>
        <li><strong>Email</strong> used for signup/contact. Source: you provide it in the Paddle checkout flow.</li>
        <li><strong>Billing data</strong>: name, address, card details (processed by Paddle, not stored by us), country (for tax calculation).</li>
        <li><strong>Active plan</strong> and subscription status.</li>
        <li><strong>License token</strong> issued by us (linked to the email, the unlocked features, and the device count).</li>
      </ul>

      <h3>2.2 Technical data (opt-in telemetry, default OFF)</h3>
      <ul>
        <li><strong>Sentry crash reports</strong>: stack trace + platform + version. PII-scrubbed (no paths, no env vars, no clipboard). Can be enabled manually from Settings → Telemetry.</li>
        <li><strong>Audit metadata cloud sync</strong> (only if enabled and on a Pro+ plan): timestamps, event types, LLM model name. Never the prompt/response body.</li>
      </ul>

      <h3>2.3 Website analytics data</h3>
      <ul>
        <li><strong>Self-hosted Plausible</strong> with consent gating: page views, referrer, screen size, browser. Cookie-less, no fingerprinting, IP addresses anonymized immediately.</li>
        <li><strong>Cloudflare server access logs</strong>: IP, user agent, URL, status. Retained for 30 days.</li>
      </ul>

      <h2>3. Data we NEVER collect</h2>
      <ul>
        <li><strong>Prompts</strong> sent to LLM providers.</li>
        <li><strong>Responses</strong> received from LLM providers.</li>
        <li><strong>API keys</strong> for LLM providers (they live in the User&apos;s OS keychain).</li>
        <li><strong>Source code</strong> or the contents of the User&apos;s files.</li>
        <li>The User&apos;s <strong>local audit log</strong> (it lives in <code>~/.furx/furx.db</code>; the opt-in cloud sync transmits metadata only, never bodies).</li>
        <li><strong>Clipboard contents</strong>.</li>
        <li><strong>Biometric data, precise location, or any GDPR Art. 9 special category of data</strong>.</li>
      </ul>

      <h2>4. Legal bases for processing (GDPR Art. 6)</h2>
      <table>
        <thead><tr><th>Activity</th><th>GDPR legal basis</th></tr></thead>
        <tbody>
          <tr><td>Provision of the Pro/Team/Enterprise subscription</td><td>Art. 6(1)(b) — performance of a contract</td></tr>
          <tr><td>Billing processing via Paddle</td><td>Art. 6(1)(b) — performance of a contract</td></tr>
          <tr><td>Compliance with tax/accounting obligations</td><td>Art. 6(1)(c) — legal obligation</td></tr>
          <tr><td>Newsletter (opt-in)</td><td>Art. 6(1)(a) — consent</td></tr>
          <tr><td>Crash telemetry (opt-in)</td><td>Art. 6(1)(a) — consent</td></tr>
          <tr><td>Platform security, fraud prevention</td><td>Art. 6(1)(f) — legitimate interest</td></tr>
        </tbody>
      </table>

      <h2>5. Retention periods</h2>
      <ul>
        <li><strong>Email + account</strong>: for as long as the subscription is active + 30 days after cancellation. Deletable on request via dpo@.</li>
        <li><strong>Billing data</strong>: 7 years (legal obligation — AFIP Argentina + EU VAT).</li>
        <li><strong>Crash telemetry</strong>: 90 days.</li>
        <li><strong>Audit metadata cloud sync</strong>: 30 days (Pro), 90 days (Team), configurable (Enterprise), 3 years (Compliance Pack).</li>
        <li><strong>Cloudflare logs</strong>: 30 days.</li>
      </ul>

      <h2>6. Subprocessors</h2>
      <p>
        A public, up-to-date list is available at{" "}
        <a href="/subprocessors/">subprocessors</a>. Change notifications are provided via RSS and by email to
        Team/Enterprise customers with 30 days&apos; advance notice.
      </p>

      <h2>7. International transfers</h2>
      <p>
        Your data may be transferred to the United States, the EU, and the United Kingdom by our subprocessors
        (Paddle UK, Cloudflare US/global, GitHub US, Better Stack EU). For transfers from
        the EEA we apply <strong>Standard Contractual Clauses (SCCs) 2021/914, Module 2</strong>{" "}
        controller → processor.
      </p>

      <h2>8. Your rights · GDPR (EU)</h2>
      <p>If you reside in the EU/EEA, you have the right to:</p>
      <ul>
        <li><strong>Access</strong> (Art. 15): obtain confirmation and a copy of your data.</li>
        <li><strong>Rectification</strong> (Art. 16): correct inaccurate data.</li>
        <li><strong>Erasure</strong> (Art. 17, &quot;right to be forgotten&quot;): have your data deleted.</li>
        <li><strong>Restriction of processing</strong> (Art. 18).</li>
        <li><strong>Data portability</strong> (Art. 20): export in structured JSON.</li>
        <li><strong>Object</strong> (Art. 21): to processing based on legitimate interest.</li>
        <li><strong>Not to be subject to automated decision-making</strong> (Art. 22).</li>
        <li><strong>Withdraw consent</strong> at any time without affecting the lawfulness of processing carried out beforehand.</li>
        <li><strong>Lodge a complaint with a supervisory authority</strong> (your national DPA).</li>
      </ul>
      <p>
        To exercise these rights: write to <a href="mailto:dpo@furx.cloud">dpo@furx.cloud</a>{" "}
        and identify yourself. Response within <strong>30 days</strong> (extendable to 60 days with
        documented justification, Art. 12(3)).
      </p>

      <h2>9. Your rights · Law 25,326 (Argentina)</h2>
      <p>If you reside in Argentina, you have the right to:</p>
      <ul>
        <li><strong>Access</strong> (Art. 14): free of charge once every 6 months, response within <strong>10 calendar days</strong>.</li>
        <li><strong>Rectification, update, or deletion</strong> (Art. 16): response within <strong>5 business days</strong>.</li>
        <li><strong>Lodge a complaint with the Agencia de Acceso a la Información Pública (AAIP)</strong> (<a href="https://www.argentina.gob.ar/aaip" target="_blank" rel="noopener noreferrer">www.argentina.gob.ar/aaip</a>).</li>
      </ul>
      <p>
        Procedure: written request (email to dpo@furx.cloud) with proof of identity
        (DNI or equivalent). Response format: PDF or exportable JSON, at the data subject&apos;s choice.
      </p>

      <h2>10. Your rights · CCPA / CPRA (California, US)</h2>
      <p>If you reside in California:</p>
      <ul>
        <li>Right to know what personal data we collect and how we use it.</li>
        <li>Right to deletion.</li>
        <li>Right to opt out of sale (we do not sell personal data).</li>
        <li>Right to non-discrimination for exercising these rights.</li>
      </ul>

      <h2>11. Security</h2>
      <p>
        Key controls (details in the <a href="/security/">Security Policy</a>):
      </p>
      <ul>
        <li>TLS 1.3 on all endpoints.</li>
        <li>Encryption at rest: AES-256.</li>
        <li>License tokens signed with HMAC-SHA256.</li>
        <li>Magic links with a 10-minute expiry, single-use.</li>
        <li>Rate limiting (3/min/IP) on sensitive endpoints via Cloudflare.</li>
        <li>Daily encrypted backups, retained for 30 days.</li>
        <li>Breach notification within <strong>72 hours</strong> to the supervisory authority and affected individuals (GDPR Art. 33/34).</li>
      </ul>

      <h2>12. Cookies</h2>
      <p>
        Details in the <a href="/cookies/">Cookie Policy</a>. By default, only strictly
        necessary cookies are used. Measurement (Plausible) is opt-in.
      </p>

      <h2>13. Minors</h2>
      <p>
        Furx is not directed at minors. We do not provide the service to:
      </p>
      <ul>
        <li>Anyone under <strong>16 years of age</strong> (GDPR Art. 8(1) — digital age of consent).</li>
        <li>Anyone under <strong>13 years of age</strong> in the United States (COPPA).</li>
      </ul>

      <h2>14. Changes to this policy</h2>
      <p>
        Material changes will be announced 30 days in advance by email + banner. The
        version history is available at{" "}
        <a href="https://github.com/hernaninverso/furx/tree/main/legal" target="_blank" rel="noopener noreferrer">
          github.com/hernaninverso/furx/tree/main/legal
        </a>.
      </p>

      <h2>15. Contact</h2>
      <p>
        <strong>DPO / Privacy</strong>: <a href="mailto:dpo@furx.cloud">dpo@furx.cloud</a><br />
        <strong>Security incidents</strong>: <a href="mailto:security@furx.cloud">security@furx.cloud</a>{" "}
        (PGP key at <a href="/.well-known/security.txt">/.well-known/security.txt</a>)
      </p>
    </LegalLayout>
  );
}
