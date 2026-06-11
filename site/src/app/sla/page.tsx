import type { Metadata } from "next";
import LegalLayout from "@/components/LegalLayout";

export const metadata: Metadata = {
  title: "SLA · Service Level Agreement",
  description: "Furx Service Level Agreement for Team and Enterprise customers — uptime, response times, credits.",
  alternates: { canonical: "https://furx.cloud/sla/" },
};

export default function SlaPage() {
  return (
    <LegalLayout title="Service Level Agreement (SLA)">
      <p>
        This SLA applies to customers with a <strong>Team</strong> or <strong>Enterprise</strong> Furx
        subscription. It does <strong>not apply</strong> to Free or individual Pro.
      </p>

      <h2>1. Scope</h2>
      <p>
        The SLA covers the availability of Furx&apos;s <strong>online</strong> components:
      </p>
      <ul>
        <li>License API (<code>api.furx.cloud/license</code>) — token validation, activation.</li>
        <li>Customer dashboard (<code>app.furx.cloud</code>) — account management, seats, downloads, audit replay.</li>
        <li>Cloud sync of <code>.mcp.json</code>, skills and audit metadata (Pro+).</li>
        <li>Public website (<code>furx.cloud</code>) — only where unavailability prevents customer activation.</li>
      </ul>
      <p>
        The SLA does <strong>not cover</strong> the desktop Application (local on the User&apos;s machine), nor
        third-party LLM providers (Anthropic, OpenAI, etc.) that the User accesses with their own
        BYOK credentials.
      </p>

      <h2>2. Availability Commitments</h2>
      <table>
        <thead><tr><th>Plan</th><th>Monthly uptime</th><th>Maintenance window</th></tr></thead>
        <tbody>
          <tr><td>Team</td><td><strong>99.5%</strong></td><td>Scheduled maintenance: Tuesdays 02:00–04:00 UTC, notified 7 days in advance</td></tr>
          <tr><td>Enterprise</td><td><strong>99.9%</strong></td><td>Same, notified 14 days in advance; HA standby reduces expected downtime to ~0%</td></tr>
        </tbody>
      </table>
      <p>
        <strong>Calculation</strong>: available minutes / total minutes in the calendar month, excluding
        scheduled maintenance windows.
      </p>

      <h2>3. Credits for Non-Compliance</h2>
      <p>If monthly uptime falls below the commitment, the Customer may request a credit:</p>
      <table>
        <thead><tr><th>Actual uptime</th><th>Credit (% of monthly fee)</th></tr></thead>
        <tbody>
          <tr><td>≥ commitment</td><td>0%</td></tr>
          <tr><td>99.0% – 99.49% (Team)</td><td>10%</td></tr>
          <tr><td>95.0% – 98.99% (Team)</td><td>25%</td></tr>
          <tr><td>&lt; 95% (Team)</td><td>50%</td></tr>
          <tr><td>99.5% – 99.89% (Enterprise)</td><td>10%</td></tr>
          <tr><td>95.0% – 99.49% (Enterprise)</td><td>25%</td></tr>
          <tr><td>&lt; 95% (Enterprise)</td><td>50%</td></tr>
        </tbody>
      </table>
      <p>
        Credit requests: <a href="mailto:support@furx.cloud">support@furx.cloud</a> within
        30 days of the close of the affected month. The credit is applied to the next invoice.
      </p>

      <h2>4. Exclusions</h2>
      <p>The following events do NOT count as downtime:</p>
      <ul>
        <li>Duly notified scheduled maintenance windows.</li>
        <li>Force majeure (events beyond reasonable control: natural disasters, armed conflict, widespread backbone network outages).</li>
        <li>Unavailability caused by third parties (Cloudflare, Paddle, the Customer&apos;s internet provider).</li>
        <li>Customer non-compliance (excessive API usage, attack originating from the Customer&apos;s side).</li>
        <li>Unavailability of the desktop Application due to causes local to the Customer.</li>
      </ul>

      <h2>5. Support Response Times</h2>
      <table>
        <thead><tr><th>Severity</th><th>Definition</th><th>Team (response)</th><th>Enterprise (response)</th></tr></thead>
        <tbody>
          <tr><td>P1 — Critical</td><td>Service down / data loss</td><td>24 business hours</td><td><strong>4 business hours</strong></td></tr>
          <tr><td>P2 — High</td><td>Major feature unusable, workaround exists</td><td>48 business hours</td><td>8 business hours</td></tr>
          <tr><td>P3 — Medium</td><td>Minor bug, workaround available</td><td>5 days</td><td>2 days</td></tr>
          <tr><td>P4 — Low</td><td>Enhancement / inquiry</td><td>10 days</td><td>5 days</td></tr>
        </tbody>
      </table>
      <p>Business hours: Monday to Friday, 9:00–18:00 UTC-3 (Argentina).</p>

      <h2>6. Support Channels</h2>
      <ul>
        <li><strong>Team</strong>: email <a href="mailto:support@furx.cloud">support@furx.cloud</a> + GitHub Issues priority label.</li>
        <li><strong>Enterprise</strong>: additionally, dedicated Slack/Discord channel and emergency phone line (P1 only).</li>
      </ul>

      <h2>7. Status Page</h2>
      <p>
        Real-time status:{" "}
        <a href="https://status.furx.cloud" target="_blank" rel="noopener noreferrer">status.furx.cloud</a>{" "}
        (Better Stack). Subscribe to the RSS feed / webhook for alerts.
      </p>

      <h2>8. SLA Modifications</h2>
      <p>
        Changes unfavorable to the Customer are notified 90 days in advance. The Customer may
        cancel without penalty by rejecting the change before it takes effect.
      </p>

      <h2>9. Contact</h2>
      <p>
        SLA / incidents: <a href="mailto:support@furx.cloud">support@furx.cloud</a>
      </p>
    </LegalLayout>
  );
}
