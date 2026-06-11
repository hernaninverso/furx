import type { Metadata } from "next";
import LegalLayout from "@/components/LegalLayout";

export const metadata: Metadata = {
  title: "Source Code Escrow",
  description: "Source Code Escrow policy for Enterprise customers — protection against bankruptcy, end-of-life, or material breach via NCC Group / Escrow.com.",
  alternates: { canonical: "https://furx.cloud/escrow/" },
};

export default function EscrowPage() {
  return (
    <LegalLayout title="Source Code Escrow · Enterprise">
      <p>
        This page describes the Source Code Escrow policy included with the Furx Enterprise
        subscription. Its purpose is to <strong>protect the customer</strong> in scenarios where
        INVERSO HUB is unable to continue providing the service.
      </p>

      <h2>1. What is deposited</h2>
      <ul>
        <li>Complete source code of the notarized binary delivered to the customer (exact commit hash).</li>
        <li>Reproducible build scripts (cargo, npm, tauri-cli versions).</li>
        <li>Self-hosted deployment documentation.</li>
        <li>The minisign public key to verify the binary.</li>
        <li>PostgreSQL schema + migrations for the licensing API (where applicable for self-hosted).</li>
      </ul>
      <p>
        What is <strong>NOT</strong> deposited:
      </p>
      <ul>
        <li>Private keys (Apple Dev ID, Azure Trusted Signing, minisign private keys).</li>
        <li>LLM provider credentials (these belong to the customer, BYOK).</li>
        <li>Data belonging to other Enterprise customers.</li>
      </ul>

      <h2>2. Escrow agent</h2>
      <p>
        The deposit is made with one of the following providers, at the customer&apos;s choice:
      </p>
      <ul>
        <li><strong>NCC Group</strong> (UK, global escrow leader): <a href="https://www.nccgroup.com/services/software-resilience/" target="_blank" rel="noopener noreferrer">nccgroup.com/services/software-resilience</a></li>
        <li><strong>Escrow.com</strong> (US, lighter-weight alternative): <a href="https://www.escrow.com" target="_blank" rel="noopener noreferrer">escrow.com</a></li>
        <li>Another accredited escrow agent, subject to INVERSO HUB&apos;s approval.</li>
      </ul>
      <p>
        The cost of escrow is included in the Enterprise subscription.
      </p>

      <h2>3. Release conditions (release triggers)</h2>
      <p>
        The escrow agent releases the deposit to the customer upon the occurrence of one of the
        following verified events:
      </p>
      <ul>
        <li><strong>Bankruptcy</strong>: INVERSO HUB S.R.L. enters insolvency or liquidation proceedings.</li>
        <li><strong>End-of-life</strong>: INVERSO HUB formally announces the discontinuation of the Furx product (does not apply if only a specific version is discontinued).</li>
        <li><strong>Unremedied material breach</strong>: breach of SLA obligations for 3 consecutive months without remediation, duly notified and verifiable.</li>
        <li><strong>Cessation of operations</strong>: INVERSO HUB fails to respond to customer communications for 60 consecutive days.</li>
      </ul>

      <h2>4. Integrity verification</h2>
      <p>
        The escrow agent may perform a <strong>periodic verification</strong> (annual
        update) to confirm that the deposit:
      </p>
      <ul>
        <li>Compiles from source code without errors.</li>
        <li>The resulting binary matches the hash of the current notarized binary.</li>
        <li>The documentation is sufficient for an average developer to reproduce the deployment.</li>
      </ul>
      <p>
        The Enterprise customer may request one additional verification per year at no cost.
      </p>

      <h2>5. Update frequency</h2>
      <ul>
        <li>Major releases (X.0.0): deposit required.</li>
        <li>Minor releases (0.X.0): deposit required if 6 months have passed since the last deposit.</li>
        <li>Patches (0.0.X): no deposit required (the customer may request one at their own cost).</li>
      </ul>

      <h2>6. Customer rights upon release</h2>
      <p>
        If the deposit is released, the customer receives a <strong>perpetual, irrevocable license</strong>{" "}
        to:
      </p>
      <ul>
        <li>Compile, modify, and deploy the deposited code for its own internal purposes.</li>
        <li>Continue using the commercial features (no commercial license renewal required).</li>
      </ul>
      <p>
        The license does NOT include the right to:
      </p>
      <ul>
        <li>Redistribute the source code.</li>
        <li>Operate Furx as a commercial SaaS for third parties.</li>
        <li>Use the &quot;Furx&quot; trademark in derivative products.</li>
      </ul>

      <h2>7. Confidentiality</h2>
      <p>
        The deposit is subject to a tripartite NDA (Customer, INVERSO HUB, Escrow Agent). The
        agent may not disclose the contents to any party except in a verified release
        scenario.
      </p>

      <h2>8. Release disputes</h2>
      <p>
        If INVERSO HUB disputes a release request, the agent convenes arbitration under
        its standard rules (NCC: London Court of International Arbitration; Escrow.com: AAA).
        The decision is binding.
      </p>

      <h2>9. Cost and duration</h2>
      <p>
        Included with an active Enterprise subscription. After cancellation, the escrow remains
        accessible for an additional 12 months (migration period).
      </p>

      <h2>10. How to activate</h2>
      <p>
        To activate Source Code Escrow, contact{" "}
        <a href="mailto:sales@furx.cloud?subject=Source%20Code%20Escrow">sales@furx.cloud</a>.
        Full onboarding within 30 days of an active Enterprise subscription.
      </p>
    </LegalLayout>
  );
}
