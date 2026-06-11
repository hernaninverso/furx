import type { Metadata } from "next";
import Link from "next/link";

import Footer from "@/components/Footer";
import Navbar from "@/components/Navbar";

export const metadata: Metadata = {
  title: "Page not found",
  description: "The page you are looking for does not exist or has been moved.",
};

export default function NotFound() {
  return (
    <>
      <Navbar />
      <main className="max-w-base mx-auto px-6 py-24 text-center">
        <pre className="inline-block text-left text-xs md:text-sm font-mono leading-snug text-accent bg-term-bg text-term-accent rounded-lg p-5 mb-8 shadow-embed">
{`$ furx open /that/page
furx: error: 404 — route not found
hint: try ⌘P, or pick one below`}
        </pre>
        <h1 className="text-4xl md:text-5xl font-semibold mb-4 text-balance text-ink">Page not found.</h1>
        <p className="text-ink-2 text-lg mb-10 max-w-xl mx-auto">
          The URL has moved, was deleted, or never existed. The links below should get you
          where you wanted to go.
        </p>
        <div className="flex flex-wrap gap-3 justify-center mb-12">
          <Link href="/" className="btn-primary">Home</Link>
          <Link href="/docs/" className="btn-secondary">Docs</Link>
          <Link href="/download/" className="btn-secondary">Download</Link>
          <Link href="/pricing/" className="btn-secondary">Pricing</Link>
        </div>
        <p className="text-ink-3 text-sm">
          Think this is a bug? Email{" "}
          <a href="mailto:support@furx.cloud" className="text-accent hover:underline">support@furx.cloud</a>{" "}
          or open an issue on{" "}
          <a href="https://github.com/hernaninverso/furx/issues" className="text-accent hover:underline" target="_blank" rel="noopener noreferrer">GitHub</a>.
        </p>
      </main>
      <Footer />
    </>
  );
}
