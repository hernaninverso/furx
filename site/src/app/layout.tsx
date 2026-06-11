import type { Metadata } from "next";
import "./globals.css";

const SITE_URL = process.env.NEXT_PUBLIC_SITE_URL || "https://furx.cloud";

export const metadata: Metadata = {
  metadataBase: new URL(SITE_URL),
  title: {
    default: "Furx — One layer under every coding agent",
    template: "%s · Furx",
  },
  description:
    "Furx is the shared layer under every coding agent you run — Claude Code, Codex, Gemini, Aider. One unified memory of every session, a signed plugin layer that works in any agent, a mobile companion to drive them from your phone, and an append-only audit trail no agent can rewrite. Runs your agents side-by-side; keys in your OS keychain, no proxy, Apache-2.0 core. macOS · Linux · Windows.",
  applicationName: "Furx",
  authors: [{ name: "INVERSO HUB S.R.L.", url: "https://furx.cloud" }],
  creator: "INVERSO HUB S.R.L.",
  publisher: "INVERSO HUB S.R.L.",
  keywords: [
    "unified memory for coding agents",
    "cross-CLI agent memory",
    "signed agent plugins",
    "mobile companion coding agent",
    "coding agent governance audit log",
    "run multiple coding agents side by side",
    "claude code codex gemini cli aider",
    "byok terminal no proxy",
    "append-only audit log",
    "tauri terminal app",
  ],
  openGraph: {
    type: "website",
    siteName: "Furx",
    title: "Furx — One layer under every coding agent",
    description:
      "Unified memory, signed plugins, a mobile companion, and a governance audit trail — across Claude Code, Codex, Gemini and Aider. Keys in your keychain, no proxy, Apache-2.0 core.",
    url: SITE_URL,
    images: [{ url: "/og.png", width: 1200, height: 630, alt: "Furx — One layer under every coding agent" }],
    locale: "en_US",
  },
  twitter: {
    card: "summary_large_image",
    title: "Furx — One layer under every coding agent",
    description: "Unified memory, signed plugins, a mobile companion, and a governance audit trail — across every coding agent. Keys in your keychain, no proxy, Apache-2.0.",
    images: ["/og.png"],
  },
  alternates: {
    types: {
      "application/rss+xml": [{ url: "/changelog/rss.xml", title: "Furx changelog" }],
    },
  },
  robots: { index: true, follow: true },
  icons: {
    icon: [
      { url: "/favicon.svg", type: "image/svg+xml" },
      { url: "/favicon.ico", sizes: "any" },
    ],
    apple: "/apple-touch-icon.png",
  },
};

const ORG_JSONLD = {
  "@context": "https://schema.org",
  "@type": "Organization",
  name: "INVERSO HUB S.R.L.",
  url: SITE_URL,
  logo: `${SITE_URL}/favicon.svg`,
  sameAs: ["https://github.com/hernaninverso/furx"],
  contactPoint: [
    { "@type": "ContactPoint", contactType: "customer support", email: "support@furx.cloud", availableLanguage: ["en", "es"] },
    { "@type": "ContactPoint", contactType: "DPO", email: "dpo@furx.cloud" },
    { "@type": "ContactPoint", contactType: "security", email: "security@furx.cloud" },
  ],
};

const APP_JSONLD = {
  "@context": "https://schema.org",
  "@type": "SoftwareApplication",
  name: "Furx",
  applicationCategory: "DeveloperApplication",
  operatingSystem: "macOS, Linux, Windows",
  description: "A local-first desktop app for running coding agents side by side. Council Mode sends one prompt to up to six models in parallel. Keys stay in the OS keychain — no proxy. Apache-2.0 core.",
  offers: { "@type": "Offer", price: "0", priceCurrency: "USD", category: "free, trial, subscription" },
  url: SITE_URL,
  downloadUrl: `${SITE_URL}/download/`,
  publisher: { "@type": "Organization", name: "INVERSO HUB S.R.L." },
};

export default function RootLayout({ children }: { children: React.ReactNode }) {
  return (
    <html lang="en" suppressHydrationWarning>
      <head>
        <meta name="color-scheme" content="light dark" />
        <meta name="theme-color" content="#f3efe6" media="(prefers-color-scheme: light)" />
        <meta name="theme-color" content="#16130f" media="(prefers-color-scheme: dark)" />
        {/* Anti-FOUC: apply V3 theme class before first paint (stored choice, else system). */}
        <script
          dangerouslySetInnerHTML={{
            __html:
              "(function(){var t=null;try{t=localStorage.getItem('furx-theme');}catch(e){}var d=t?t==='dark':(window.matchMedia&&matchMedia('(prefers-color-scheme:dark)').matches);document.documentElement.classList.toggle('dark',!!d);})();",
          }}
        />
        <link rel="alternate" type="application/rss+xml" title="Furx changelog" href="/changelog/rss.xml" />
        <script type="application/ld+json" dangerouslySetInnerHTML={{ __html: JSON.stringify(ORG_JSONLD) }} />
        <script type="application/ld+json" dangerouslySetInnerHTML={{ __html: JSON.stringify(APP_JSONLD) }} />
      </head>
      <body>
        <a href="#main" className="skip-link">Skip to main content</a>
        {children}
        {/* Cloudflare Web Analytics — privacy-first, no cookies, no PII. Funnel: traffic + conversions. */}
        <script
          defer
          src="https://static.cloudflareinsights.com/beacon.min.js"
          data-cf-beacon='{"token": "57f17080132f42fcada996c9b787f8b0"}'
        />
      </body>
    </html>
  );
}
