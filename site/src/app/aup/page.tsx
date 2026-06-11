import type { Metadata } from "next";
import LegalLayout from "@/components/LegalLayout";

export const metadata: Metadata = {
  title: "Acceptable Use Policy",
  description: "Furx Acceptable Use Policy — prohibited uses, OFAC sanctions, BYOK clarification, right to terminate.",
  alternates: { canonical: "https://furx.cloud/aup/" },
};

export default function AupPage() {
  return (
    <LegalLayout title="Acceptable Use Policy (AUP)">
      <p>
        This Acceptable Use Policy (hereinafter, the <strong>&quot;AUP&quot;</strong>) defines the
        prohibited uses of Furx and forms an integral part of the{" "}
        <a href="/terms/">Terms and Conditions</a>. Violation of this AUP may result in
        immediate suspension or termination of the service.
      </p>

      <h2>1. Prohibited uses · User content</h2>
      <p>The User may not use Furx to:</p>
      <ul>
        <li>Generate, store, or distribute material that infringes the intellectual property rights of third parties.</li>
        <li>Generate illegal content: child sexual abuse material (CSAM), incitement to violence, fraud, money laundering.</li>
        <li>Harassment, threats, doxxing, or impersonation.</li>
        <li>Generate deepfakes of real persons without their consent.</li>
        <li>Create or distribute <strong>malware</strong>, ransomware, unauthorized exploits, or DDoS tools.</li>
        <li>Conduct social engineering, phishing, or pretexting attacks.</li>
        <li>Generate disinformation targeting democratic electoral processes.</li>
        <li>Coordinate terrorist activities or activities of designated organizations (UN/OFAC/EU).</li>
      </ul>

      <h2>2. Prohibited uses · technical</h2>
      <ul>
        <li>Circumvent or attempt to circumvent the license gate (Pro / Team / Enterprise).</li>
        <li>Reverse engineer the license tokens, the Ed25519 signatures, or the magic-link protocol, except to the extent permitted by mandatory law.</li>
        <li>Distribute modified versions of the notarized binary under the &quot;Furx&quot; trademark.</li>
        <li>Attack the licensing API (<code>api.furx.cloud</code>): brute force, credential stuffing, DDoS, unauthorized fuzzing (responsible disclosure via <a href="/security/">security.txt</a>).</li>
        <li>Scrape the public site at a rate that affects availability for other users.</li>
        <li>Load the customer dashboard as an iframe on an unauthorized site (clickjacking).</li>
      </ul>

      <h2>3. Prohibited uses · LLM providers (User responsibility)</h2>
      <p>
        Under the BYOK model, the User is responsible for complying with the TOS of the LLM provider
        whose credentials they use. INVERSO HUB does not audit or enforce compliance with third-party
        TOS, but the User acknowledges that:
      </p>
      <ul>
        <li>Anthropic, OpenAI, Google, and others maintain their own lists of prohibited uses.</li>
        <li>The User is responsible for reviewing and complying with them.</li>
        <li>Revocation of credentials by the provider is at the provider&apos;s sole discretion and does not give rise to any right to a refund from INVERSO HUB.</li>
      </ul>

      <h2>4. International sanctions (OFAC / EU)</h2>
      <p>
        INVERSO HUB <strong>does not provide service</strong> to individuals or entities:
      </p>
      <ul>
        <li>Resident in countries subject to comprehensive sanctions: Cuba, Iran, North Korea, Syria, the Crimea / Donetsk / Luhansk regions of Ukraine, Belarus.</li>
        <li>Listed on OFAC&apos;s SDN (Specially Designated Nationals) list, the EU Consolidated List, the UN Sanctions List, or the UK HM Treasury Sanctions List.</li>
        <li>Majority owned or controlled by sanctioned entities (OFAC&apos;s 50% rule).</li>
      </ul>
      <p>
        Upon detection, we will suspend the account immediately and report as required by applicable
        legal obligations.
      </p>

      <h2>5. Minimum ages</h2>
      <ul>
        <li>EU/EEA: 16 years (GDPR Art. 8(1) — age of digital consent).</li>
        <li>United States: 13 years (COPPA).</li>
        <li>Argentina: 18 years to enter commercial contracts independently; minors require a legal representative.</li>
      </ul>

      <h2>6. Reporting violations</h2>
      <p>
        Seen something that violates this AUP? Report it to{" "}
        <a href="mailto:legal@furx.cloud">legal@furx.cloud</a> with:
      </p>
      <ul>
        <li>A description of the content / activity.</li>
        <li>URL, screenshot, or reasonable evidence.</li>
        <li>Your name and contact details (optional, appreciated).</li>
      </ul>
      <p>We review reports within 5 business days. Repeated abuse results in a permanent ban.</p>

      <h2>7. Penalties for violations</h2>
      <table>
        <thead><tr><th>Severity</th><th>First occurrence</th><th>Repeat offense</th></tr></thead>
        <tbody>
          <tr><td>Minor (e.g. accidental scraping)</td><td>Warning + adjustment</td><td>Hard rate limit</td></tr>
          <tr><td>Material (e.g. attempted license gate circumvention)</td><td>7-day suspension + review</td><td>Termination + pro-rata refund</td></tr>
          <tr><td>Severe (CSAM, OFAC sanctions, malware)</td><td><strong>Immediate termination</strong> + report to authorities</td><td>N/A</td></tr>
        </tbody>
      </table>

      <h2>8. Appeals</h2>
      <p>
        If you believe a suspension was made in error, write to{" "}
        <a href="mailto:legal@furx.cloud">legal@furx.cloud</a> within 15 days with your
        case. We respond within 10 business days.
      </p>

      <h2>9. Changes</h2>
      <p>
        This AUP may be updated to reflect new categories of abuse. Material changes come with
        30 days&apos; notice. Urgent changes required by law take effect immediately.
      </p>

      <h2>10. Contact</h2>
      <p>
        Report abuse: <a href="mailto:legal@furx.cloud">legal@furx.cloud</a><br />
        Appeals / legal inquiries: <a href="mailto:legal@furx.cloud">legal@furx.cloud</a>
      </p>
    </LegalLayout>
  );
}
