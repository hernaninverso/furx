import Footer from "./Footer";
import Navbar from "./Navbar";

interface Props {
  title: string;
  version?: string;
  lastUpdated?: string;
  locale?: "en" | "es";
  children: React.ReactNode;
}

export default function LegalLayout({
  title,
  version = "1.2",
  lastUpdated = "June 9, 2026",
  locale = "en",
  children,
}: Props) {
  void locale;
  return (
    <>
      <Navbar />
      <main id="main" className="max-w-narrow mx-auto px-6 py-16">
        <div className="border-b border-rule pb-6 mb-8">
          <h1 className="text-3xl font-semibold mb-2 text-ink">{title}</h1>
          <p className="text-sm text-ink-3 font-mono">
            Version {version} · Last updated: {lastUpdated} · Operator:{" "}
            <strong className="text-ink">Furx</strong> (INVERSO HUB S.R.L.)
          </p>
        </div>
        <article className="prose-furx">{children}</article>
        <div className="mt-12 pt-6 border-t border-rule text-xs text-ink-3">
          Questions? Write to{" "}
          <a href="mailto:legal@furx.cloud" className="text-accent hover:underline">legal@furx.cloud</a>.{" "}
          Privacy: <a href="mailto:dpo@furx.cloud" className="text-accent hover:underline">dpo@furx.cloud</a>.
        </div>
      </main>
      <Footer />
    </>
  );
}
