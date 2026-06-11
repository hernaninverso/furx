import type { Metadata } from "next";
import LegalLayout from "@/components/LegalLayout";

export const metadata: Metadata = {
  title: "Terms of Service",
  description: "Furx Terms and Conditions of Use — INVERSO HUB S.R.L., Argentine jurisdiction, billing through Paddle as Merchant of Record.",
  alternates: { canonical: "https://furx.cloud/terms/" },
};

export default function TermsPage() {
  return (
    <LegalLayout title="Terms and Conditions of Use · Furx">
      <p>
        These Terms and Conditions (the <strong>&quot;Terms&quot;</strong>) govern access to and use
        of the <strong>Furx</strong> software (the &quot;Application&quot;), the website{" "}
        <code>furx.cloud</code> and the customer dashboard <code>app.furx.cloud</code> (together,
        the <strong>&quot;Platform&quot;</strong>), operated by <strong>INVERSO HUB S.R.L.</strong> (hereinafter,{" "}
        <strong>&quot;INVERSO HUB&quot;</strong> or <strong>&quot;we&quot;</strong>).
      </p>

      <h2>1. Acceptance</h2>
      <p>
        By installing the Application, creating an account, signing in, or using any part of the
        Platform, you (the <strong>&quot;User&quot;</strong>) acknowledge that you have read, understood
        and accepted these Terms. If you do not agree, do not install or use the Platform.
      </p>

      <h2>2. Nature of the Service · BYOK (Bring Your Own Keys)</h2>
      <p>
        Furx is a <strong>local orchestrator</strong> of command-line interfaces for Large Language
        Models (Claude Code, Codex, Gemini, Aider and others) plus a capability called{" "}
        <em>Council Mode</em> that dispatches a single prompt to up to six LLM providers in parallel.
      </p>
      <p>
        <strong>Strict pass-through.</strong> The credentials (API keys) for LLM providers are the{" "}
        <strong>sole property and responsibility of the User</strong>, are stored in the User&apos;s
        operating system keychain (Keychain on macOS, Secret Service on Linux, Credential Manager
        on Windows), and calls are made directly from the User&apos;s machine to the provider with no
        intermediate proxy on INVERSO HUB infrastructure.
      </p>
      <p>
        INVERSO HUB <strong>does not process, store, transmit or have access to</strong> the
        User&apos;s prompts, responses, API keys or source code. The audit log is a local SQLite file
        on the User&apos;s machine (<code>~/.furx/furx.db</code>), of an append-only nature.
      </p>

      <h2>3. License · Apache-2.0 Core + Commercial Features</h2>
      <p>
        The core of the Application is licensed under <strong>Apache-2.0</strong> (full text at{" "}
        <a href="https://github.com/hernaninverso/furx/blob/main/LICENSE" target="_blank" rel="noopener noreferrer">github.com/hernaninverso/furx/blob/main/LICENSE</a>).
      </p>
      <p>
        The commercial features — cloud sync of skills and .mcp.json, encrypted Memory Hub
        backups, session replay scrubber, Cost Meter Pro with alerts, latency heatmap with
        trends, and premium themes — are gated behind a{" "}
        <strong>license token</strong> issued by INVERSO HUB, valid for the paid period and
        subject to a separate commercial license (Pro / Team / Enterprise / Compliance Pack).
      </p>
      <p>
        <strong>Features that are NOT gated on Free</strong>: Council Mode (up to 6 voices per
        dispatch, free on all plans), number of panes, local Memory Hub, local Skills Registry, voice-to-pane,
        mobile bridge, local audit log, all hotkeys. The Pro license does <em>not</em> buy more
        voices or more panes — it buys the cloud infrastructure that powers sync, replay and backups.
      </p>

      <h2>4. Plans and Pricing</h2>
      <p>The current plans are:</p>
      <ul>
        <li><strong>Free</strong>: USD 0. Apache-2.0 core, all non-gated features.</li>
        <li><strong>Pro</strong>: USD 12/month (USD 99/year). Commercial features detailed on the <a href="/pricing/">pricing</a> page. 14-day trial, no credit card required.</li>
        <li><strong>Team</strong>: USD 30/seat/month, 5-seat minimum.</li>
        <li><strong>Enterprise</strong>: USD 49/seat/month with a 20-seat minimum, or USD 2,500 for a notarized self-hosted perpetual license.</li>
        <li><strong>Compliance Pack</strong>: USD 199 one-time (Team/Enterprise add-on).</li>
      </ul>
      <p>
        Prices are in United States dollars (USD) and include taxes where applicable
        (handled by Paddle as Merchant of Record).
      </p>

      <h2>5. Billing · Merchant of Record</h2>
      <p>
        Billing is handled by <strong>Paddle.com Market Limited</strong> (United Kingdom)
        acting as <strong>Merchant of Record (MoR)</strong>. Paddle issues invoices,
        manages collection and remits applicable taxes (EU VAT, US sales tax, Argentine VAT,
        and others).
      </p>
      <p>
        Renewals are automatic. The User may cancel at any time from the customer dashboard{" "}
        <code>app.furx.cloud</code>; cancellation takes effect at the end of the paid period.
      </p>

      <h2>6. Trial</h2>
      <p>
        Every first installation automatically activates <strong>14 days of Pro features</strong>{" "}
        with no credit card required. At the end of the trial, the Application automatically
        reverts to Free.
      </p>

      <h2>7. Refunds</h2>
      <p>
        Subscriptions (Pro/Team): <strong>14-day</strong> no-questions-asked refund from the
        first charge, in accordance with the Paddle MoR policy. Details in the{" "}
        <a href="/refund/">Refund Policy</a>.
      </p>
      <p>
        One-time Compliance Pack: refundable within 14 days only if the deliverable material
        has NOT been downloaded.
      </p>
      <p>
        Enterprise perpetual: 30-day review period, refundable if it has not been installed
        or notarized.
      </p>

      <h2>8. Acceptable Use</h2>
      <p>
        Use of the Platform is subject to the <a href="/aup/">Acceptable Use Policy</a>.
        Principal prohibitions: distributing malware, scraping competitors, abusing third-party
        LLM providers, attempting to circumvent the license gate, reverse-engineering the
        license tokens, and offering the service to entities in countries sanctioned by OFAC.
      </p>

      <h2>9. Intellectual Property</h2>
      <p>
        9.1. The <strong>Furx</strong> trademark, the logo (coral F icon), the interface
        design, the source code of the commercial component, the Council Mode dispatch
        algorithms and all associated documentation are the exclusive property of INVERSO HUB
        S.R.L. and are protected by Argentine Intellectual Property Law No. 11,723, Trademark
        and Designations Law No. 22,362, and applicable international treaties.
      </p>
      <p>
        9.2. The User retains <strong>full ownership</strong> of:
      </p>
      <ul>
        <li>The User&apos;s source code.</li>
        <li>The prompts sent to LLM providers.</li>
        <li>The responses received.</li>
        <li>The data stored in the User&apos;s operating system keychain.</li>
        <li>The local audit file <code>~/.furx/furx.db</code>.</li>
      </ul>
      <p>
        9.3. The Apache-2.0 core may be forked, modified and redistributed in accordance with its license.
      </p>

      <h2>10. Personal Data</h2>
      <p>
        The processing of personal data is governed by our{" "}
        <a href="/privacy/">Privacy Policy</a> and, for Team/Enterprise customers, by the{" "}
        <a href="/dpa/">Data Processing Agreement (DPA)</a>.
      </p>

      <h2>11. SLA</h2>
      <p>
        The service level applies only to the customer dashboard and the license API (not to the
        desktop Application, which runs locally). Commitments in the{" "}
        <a href="/sla/">SLA</a>: 99.5% for Team, 99.9% for Enterprise.
      </p>

      <h2>12. Availability and Warranty</h2>
      <p>
        The Platform is provided <strong>&quot;as is&quot; and &quot;as available&quot;</strong>. To
        the maximum extent permitted by applicable law, INVERSO HUB disclaims all implied or
        express warranties of merchantability, fitness for a particular purpose, and
        non-infringement.
      </p>
      <p>
        INVERSO HUB does NOT warrant the quality, accuracy, absence of errors, or availability of
        responses generated by third-party LLM providers. The User acknowledges that language
        models may produce incorrect, biased or misleading information, and is responsible for
        validating outputs before relying on them for critical decisions.
      </p>

      <h2>13. Limitation of Liability</h2>
      <p>
        To the maximum extent permitted by applicable law, the total aggregate liability of
        INVERSO HUB to the User for all claims arising out of or relating to the
        Platform shall not exceed the <strong>amount actually paid</strong> by the User to
        INVERSO HUB (via Paddle MoR) during the <strong>twelve (12) months</strong> preceding the
        event giving rise to the claim.
      </p>
      <p>
        In no event shall INVERSO HUB be liable for indirect, incidental, special,
        consequential or punitive damages, nor for lost profits, loss of data, loss of
        opportunities or cost of cover.
      </p>

      <h2>14. Mutual Indemnification</h2>
      <p>
        Each party agrees to hold the other harmless against third-party claims
        arising from the responsible party&apos;s breach of these Terms or of applicable
        law.
      </p>

      <h2>15. Suspension and Termination</h2>
      <p>
        <strong>For cause.</strong> INVERSO HUB may suspend or terminate access to the commercial
        features in the event of a material breach by the User (non-payment, violation of the AUP,
        intellectual property infringement), with prior notice where practicable.
      </p>
      <p>
        <strong>For convenience.</strong> Either party may terminate the relationship
        upon 30 days&apos; written notice, without prejudice to accrued obligations.
      </p>
      <p>
        Upon termination, the User retains the right to use the Apache-2.0 core and their local
        data indefinitely. Access to commercial features and to any data synced to the cloud
        ceases at the end of the paid period.
      </p>

      <h2>16. Jurisdiction and Governing Law</h2>
      <p>
        For Users domiciled in <strong>Argentina</strong>: these Terms are governed by the
        laws of the Argentine Republic; any dispute shall be submitted to the jurisdiction of the
        Ordinary Commercial Courts of the Autonomous City of Buenos Aires, without prejudice
        to consumer rights under Law No. 24,240.
      </p>
      <p>
        For Users domiciled outside Argentina, the commercial Terms are governed by the
        laws of the United Kingdom (Paddle MoR), with jurisdiction of the courts of England and
        Wales, without prejudice to non-derogable consumer protection rights of the country of
        residence.
      </p>

      <h2>17. Changes</h2>
      <p>
        INVERSO HUB reserves the right to modify these Terms. Material changes will be
        notified at least 30 days in advance to the User&apos;s email address and via a banner
        on the Platform. If the User does not accept the changes, the User may cancel before they
        take effect.
      </p>

      <h2>18. Definitions</h2>
      <ul>
        <li><strong>User</strong>: natural or legal person using the Platform.</li>
        <li><strong>Platform</strong>: the Application + website + customer dashboard + license API.</li>
        <li><strong>BYOK</strong>: Bring Your Own Keys — the LLM provider keys are supplied by the User.</li>
        <li><strong>Council Mode</strong>: parallel dispatch feature across multiple LLMs.</li>
        <li><strong>MoR</strong>: Merchant of Record (Paddle).</li>
        <li><strong>AUP</strong>: Acceptable Use Policy.</li>
      </ul>

      <h2>19. Contact</h2>
      <p>
        Legal inquiries: <a href="mailto:legal@furx.cloud">legal@furx.cloud</a><br />
        General support: <a href="mailto:support@furx.cloud">support@furx.cloud</a><br />
        Privacy / DPO: <a href="mailto:dpo@furx.cloud">dpo@furx.cloud</a><br />
        Security: <a href="mailto:security@furx.cloud">security@furx.cloud</a>
      </p>
    </LegalLayout>
  );
}
