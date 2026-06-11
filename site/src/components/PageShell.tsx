import type { ReactNode } from "react";

import Footer from "@/components/Footer";
import Navbar from "@/components/Navbar";

/** Shared page chrome for content pages (errors, samples, mandates, FatturaPA). */
export default function PageShell({
  children,
  wide = false,
}: {
  children: ReactNode;
  wide?: boolean;
}) {
  return (
    <>
      <Navbar />
      <main className={`${wide ? "max-w-6xl" : "max-w-3xl"} mx-auto px-6 pt-14 pb-20`}>
        {children}
      </main>
      <Footer />
    </>
  );
}

export function Crumbs({ items }: { items: { label: string; href?: string }[] }) {
  return (
    <nav className="text-xs text-ink-2 mb-6 flex flex-wrap gap-1">
      {items.map((it, i) => (
        <span key={i}>
          {it.href ? (
            <a href={it.href} className="hover:text-accent">
              {it.label}
            </a>
          ) : (
            <span>{it.label}</span>
          )}
          {i < items.length - 1 && <span className="mx-1 text-border">/</span>}
        </span>
      ))}
    </nav>
  );
}

export function JsonLd({ data }: { data: unknown }) {
  return (
    <script
      type="application/ld+json"
      dangerouslySetInnerHTML={{ __html: JSON.stringify(data) }}
    />
  );
}
