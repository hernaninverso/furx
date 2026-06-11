import Link from "next/link";

const GH_REPO = process.env.NEXT_PUBLIC_GH_REPO || "https://github.com/hernaninverso/furx";

interface Props { locale?: "en" | "es" }

export default function Footer({ locale = "en" }: Props) {
  void locale;
  return (
    <footer className="border-t border-rule mt-24 no-print bg-bg-2">
      <div className="max-w-wide mx-auto px-6 py-12 grid md:grid-cols-5 gap-8 text-sm">
        <div className="md:col-span-2">
          <div className="flex items-center gap-2.5 mb-3 text-base font-semibold text-ink">
            <span className="brand-mark">F</span>
            <span>Furx</span>
          </div>
          <p className="text-ink-2 max-w-xs leading-relaxed">
            Run any coding agent side-by-side. No proxy. A local-first desktop app —
            your keys never leave your machine. Apache-2.0 core.
          </p>
          <div className="mt-4 flex gap-2 flex-wrap text-xs font-mono">
            <span className="pill">Apache-2.0 core</span>
            <span className="pill">macOS · Linux · Windows</span>
          </div>
        </div>

        <div>
          <div className="text-ink-3 uppercase text-xs tracking-wider mb-3 font-mono">Product</div>
          <ul className="space-y-2">
            <li><Link href="/" className="text-ink-2 hover:text-accent">Home</Link></li>
            <li><Link href="/council-mode/" className="text-ink-2 hover:text-accent">Council Mode</Link></li>
            <li><Link href="/providers/" className="text-ink-2 hover:text-accent">Providers</Link></li>
            <li><Link href="/pricing/" className="text-ink-2 hover:text-accent">Pricing</Link></li>
            <li><Link href="/download/" className="text-ink-2 hover:text-accent">Download</Link></li>
            <li><Link href="/changelog/" className="text-ink-2 hover:text-accent">Changelog</Link></li>
          </ul>
        </div>

        <div>
          <div className="text-ink-3 uppercase text-xs tracking-wider mb-3 font-mono">Developers</div>
          <ul className="space-y-2">
            <li><Link href="/docs/" className="text-ink-2 hover:text-accent">Docs</Link></li>
            <li><Link href="/docs/quickstart/" className="text-ink-2 hover:text-accent">Quickstart</Link></li>
            <li><Link href="/docs/byok/" className="text-ink-2 hover:text-accent">BYOK guide</Link></li>
            <li><Link href="/docs/integrations/" className="text-ink-2 hover:text-accent">Integrations</Link></li>
            <li><a href={GH_REPO} target="_blank" rel="noopener noreferrer" className="text-ink-2 hover:text-accent">GitHub</a></li>
            <li><Link href="/community/" className="text-ink-2 hover:text-accent">Community</Link></li>
          </ul>
        </div>

        <div>
          <div className="text-ink-3 uppercase text-xs tracking-wider mb-3 font-mono">Legal</div>
          <ul className="space-y-2">
            <li><Link href="/terms/" className="text-ink-2 hover:text-accent">Terms</Link></li>
            <li><Link href="/privacy/" className="text-ink-2 hover:text-accent">Privacy</Link></li>
            <li><Link href="/dpa/" className="text-ink-2 hover:text-accent">DPA</Link></li>
            <li><Link href="/sla/" className="text-ink-2 hover:text-accent">SLA</Link></li>
            <li><Link href="/subprocessors/" className="text-ink-2 hover:text-accent">Subprocessors</Link></li>
            <li><Link href="/aup/" className="text-ink-2 hover:text-accent">Acceptable use</Link></li>
            <li><Link href="/refund/" className="text-ink-2 hover:text-accent">Refund</Link></li>
            <li><Link href="/cookies/" className="text-ink-2 hover:text-accent">Cookies</Link></li>
            <li><Link href="/imprint/" className="text-ink-2 hover:text-accent">Imprint</Link></li>
            <li><Link href="/security/" className="text-ink-2 hover:text-accent">Security</Link></li>
          </ul>
        </div>
      </div>

      <div className="border-t border-rule">
        <div className="max-w-wide mx-auto px-6 py-5 text-xs text-ink-3 flex flex-wrap gap-2 justify-between">
          <span>© {new Date().getFullYear()} INVERSO HUB S.R.L. · Buenos Aires, Argentina</span>
          <span>
            Built with <a href="https://tauri.app" target="_blank" rel="noopener noreferrer" className="text-accent hover:underline">Tauri</a>.
            Local audit log. BYOK pure. <Link href="/security/" className="text-accent hover:underline">Report a vulnerability</Link>.
          </span>
        </div>
      </div>
    </footer>
  );
}
