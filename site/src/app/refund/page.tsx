import type { Metadata } from "next";
import LegalLayout from "@/components/LegalLayout";

export const metadata: Metadata = {
  title: "Refund Policy",
  description: "Furx Refund Policy — 14-day no-questions-asked refunds via Paddle MoR. Pro, Team, Enterprise, Compliance Pack.",
  alternates: { canonical: "https://furx.cloud/refund/" },
};

export default function RefundPage() {
  return (
    <LegalLayout title="Refund Policy · Furx">
      <h2>1. General Window · 14 Days</h2>
      <p>
        We offer a <strong>full, no-questions-asked</strong> refund within 14 calendar days
        of the first charge of any Furx subscription. The refund is processed by
        Paddle (Merchant of Record) and appears on your payment method within 5–10 business days
        depending on the issuing bank.
      </p>

      <h2>2. By Product</h2>
      <table>
        <thead><tr><th>Product</th><th>Window</th><th>Conditions</th></tr></thead>
        <tbody>
          <tr><td>Pro monthly ($12)</td><td>14 days from first charge</td><td>No questions asked</td></tr>
          <tr><td>Pro annual ($99)</td><td>14 days from first charge</td><td>No questions asked; pro-rated beyond the window</td></tr>
          <tr><td>Team ($30/seat/mo)</td><td>14 days from first charge</td><td>No questions asked; applies to the first billing cycle</td></tr>
          <tr><td>Enterprise annual ($49/seat/mo)</td><td>30 days from signing</td><td>No questions asked if not notarized/installed</td></tr>
          <tr><td>Enterprise perpetual ($2,500)</td><td>30 days from delivery</td><td>Refundable if not installed/notarized</td></tr>
          <tr><td>Compliance Pack ($199 one-time)</td><td>14 days from purchase</td><td><strong>Refundable only if the deliverable material has NOT been downloaded</strong></td></tr>
        </tbody>
      </table>

      <h2>3. Automatic Renewals</h2>
      <p>
        Automatic renewals do <strong>NOT</strong> open a new 14-day window. To
        avoid renewal, cancel from the customer dashboard before the end of the paid period.
      </p>
      <p>
        If you were charged for a renewal you did not want, contact{" "}
        <a href="mailto:support@furx.cloud">support@furx.cloud</a> within 7 days of the
        charge — we review these on a case-by-case basis.
      </p>

      <h2>3.bis What Does Pro Actually Cover?</h2>
      <p>
        Pro does not charge for Council voices or for panes — Council Mode stays free (up to 6 voices per dispatch) and panes are not gated.
        Pro charges for the cloud infrastructure we provide:
      </p>
      <ul>
        <li>Cross-device sync of skills and <code>.mcp.json</code></li>
        <li>Daily encrypted Memory Hub backups</li>
        <li>Session replay scrubber (30-day retention)</li>
        <li>Cost Meter Pro with alerts and CSV export</li>
        <li>Latency heatmap with 7- and 30-day trends</li>
        <li>Priority support via private GitHub + Discord channel</li>
      </ul>
      <p>
        If you cancel, all local features (Council, panes, local Memory Hub, voice, mobile, audit
        log, skills) keep working forever. You only stop syncing and lose replay/backups.
      </p>

      <h2>4. How to Request a Refund</h2>
      <ol>
        <li>Sign in at <a href="https://app.furx.cloud">app.furx.cloud</a>.</li>
        <li>Go to <em>Account → Billing → Request refund</em>.</li>
        <li>The system automatically triggers the Paddle refund flow.</li>
      </ol>
      <p>
        Alternatively: write to <a href="mailto:support@furx.cloud">support@furx.cloud</a>{" "}
        from the email address associated with the subscription.
      </p>

      <h2>5. Refunds Through Paddle (MoR)</h2>
      <p>
        Paddle is the Merchant of Record (MoR) and processes the refund in accordance with its terms.
        This includes the return of VAT / sales tax paid, depending on jurisdiction.
      </p>
      <p>
        Disputes via bank chargeback trigger automatic suspension of the account until
        resolved. We prefer to resolve matters by mutual agreement.
      </p>

      <h2>6. Exceptions / Non-Refundable</h2>
      <ul>
        <li>Compliance Pack already downloaded.</li>
        <li>Enterprise already installed, notarized and in use.</li>
        <li>Requests outside the window without justified cause.</li>
        <li>Accounts suspended for violation of the <a href="/aup/">AUP</a>.</li>
        <li>Fraudulent use (multiple trials from the same person/IP).</li>
      </ul>

      <h2>7. EU Right of Withdrawal (Directive 2011/83/EU)</h2>
      <p>
        Consumers residing in the EU/EEA have a statutory right of withdrawal of{" "}
        <strong>14 days</strong> from the conclusion of the contract, without giving any reason,
        pursuant to Art. 9 of Directive 2011/83/EU.
      </p>
      <p>
        <strong>Important exception (Art. 16(m))</strong>: the supply of digital content
        that begins with your express consent and acknowledgment of the loss of the right of
        withdrawal (this is the case for a downloaded Compliance Pack). To avoid this loss, do NOT
        download the material if you plan to withdraw.
      </p>

      <h2>8. Argentina · Consumers</h2>
      <p>
        For users residing in Argentina, Consumer Protection Law No. 24,240 additionally
        applies. The right of revocation is 10 calendar days from the conclusion of the
        contract or the receipt of the good, whichever occurs later (Art. 34).
      </p>

      <h2>9. Trial · No Charge</h2>
      <p>
        The 14-day Pro trial does NOT require a credit card and generates NO charge. At the end,
        the Application automatically reverts to Free. No refund applies (there was no charge).
      </p>

      <h2>10. Contact</h2>
      <p>
        <a href="mailto:support@furx.cloud">support@furx.cloud</a> · Response within
        24 business hours for refund requests.
      </p>
    </LegalLayout>
  );
}
