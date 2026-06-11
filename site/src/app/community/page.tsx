import type { Metadata } from "next";

import Footer from "@/components/Footer";
import Navbar from "@/components/Navbar";

export const metadata: Metadata = {
  title: "Community",
  description: "Furx community channels: GitHub Discussions, Discord, Telegram, Matrix, RSS.",
  alternates: { canonical: "https://furx.cloud/community/" },
};

const CHANNELS = [
  {
    name: "GitHub Discussions",
    desc: "Q&A, feature requests, RFCs. Async-friendly, threaded.",
    href: "https://github.com/hernaninverso/furx/discussions",
    label: "github.com/hernaninverso/furx/discussions",
  },
  {
    name: "Discord",
    desc: "Live chat, voice channels for pair-programming and Council Mode demos.",
    href: "https://discord.gg/furx",
    label: "discord.gg/furx",
  },
  {
    name: "Telegram",
    desc: "Releases, security bulletins, low-volume announce channel.",
    href: "https://t.me/furx_releases",
    label: "t.me/furx_releases",
  },
  {
    name: "Matrix",
    desc: "Federated alternative to Discord — for users with strict allow-lists.",
    href: "https://matrix.to/#/#furx:furx.cloud",
    label: "#furx:furx.cloud",
  },
  {
    name: "Changelog RSS",
    desc: "Subscribe via your feed reader to get releases pushed to wherever you read.",
    href: "/changelog/rss.xml",
    label: "/changelog/rss.xml",
  },
  {
    name: "Mastodon",
    desc: "Microblog on Fosstodon — release notes and threads.",
    href: "https://fosstodon.org/@furx",
    label: "@furx@fosstodon.org",
  },
];

const COC = [
  "Be kind, assume good faith.",
  "Stay on-topic in product channels (Q&A → discussions, off-topic → Discord #random).",
  "No vendor bashing — comparing tools is fine, attacking people is not.",
  "Security vulnerabilities go to security@furx.cloud (PGP). NOT public channels.",
  "Free tier users get the same support quality as Pro on public channels.",
];

export default function CommunityPage() {
  return (
    <>
      <Navbar />
      <main id="main" className="max-w-base mx-auto px-6 pt-16 pb-24">
        <header className="mb-12">
          <div className="brand-mark mb-6 text-[26px]" aria-hidden="true" />
          <h1 className="text-4xl font-extrabold mb-3">Community</h1>
          <p className="text-ink-2 text-lg max-w-2xl">
            Async-first (GitHub, RSS), live for synchronous ones (Discord, Telegram).
            All channels are mirrored on the changelog RSS — pick whichever feed reader you prefer.
          </p>
        </header>

        <div className="grid md:grid-cols-2 gap-5 mb-16">
          {CHANNELS.map((c) => (
            <a
              key={c.name}
              href={c.href}
              target="_blank"
              rel="noopener noreferrer"
              className="bg-panel border border-rule rounded-lg p-5 hover:border-accent-dim transition-colors block"
            >
              <h2 className="text-lg font-display font-medium text-accent mb-2">{c.name}</h2>
              <p className="text-ink-2 text-sm mb-2 leading-relaxed">{c.desc}</p>
              <code className="text-xs text-ink-3">{c.label}</code>
            </a>
          ))}
        </div>

        <section className="bg-panel border border-rule rounded-lg p-6">
          <h2 className="text-xl font-display font-medium mb-3">Code of conduct</h2>
          <ul className="text-ink-2 text-sm space-y-2 list-disc pl-5">
            {COC.map((line) => (
              <li key={line}>{line}</li>
            ))}
          </ul>
          <p className="text-ink-3 text-xs mt-4">
            Full text:{" "}
            <a href="https://github.com/hernaninverso/furx/blob/main/CODE_OF_CONDUCT.md" className="text-accent hover:underline" target="_blank" rel="noopener noreferrer">
              CODE_OF_CONDUCT.md
            </a>{" "}
            (Contributor Covenant 2.1). Violations → conduct@furx.cloud.
          </p>
        </section>
      </main>
      <Footer />
    </>
  );
}
