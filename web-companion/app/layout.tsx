import type { Metadata } from "next";

export const metadata: Metadata = {
  title: "Furx Companion · audit replay read-only",
  description: "Browser companion for Furx desktop — read-only audit replay + approve tool-calls.",
};

export default function RootLayout({ children }: { children: React.ReactNode }) {
  return (
    <html lang="es">
      <head>
        {/* 058 — fuentes de marca Atelier Terminal */}
        <link rel="preconnect" href="https://fonts.googleapis.com" />
        <link rel="preconnect" href="https://fonts.gstatic.com" crossOrigin="anonymous" />
        <link
          href="https://fonts.googleapis.com/css2?family=Fraunces:ital,opsz,wght,SOFT,WONK@0,9..144,400..700,0..100,0..1;1,9..144,400..600,0..100,0..1&family=Hanken+Grotesk:wght@400;500;600;700&family=Space+Mono:wght@400;700&display=swap"
          rel="stylesheet"
        />
      </head>
      <body style={{ margin: 0, background: "#16130f", color: "#f2e8d8" }}>{children}</body>
    </html>
  );
}
