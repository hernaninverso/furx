import type { Config } from "tailwindcss";

/* V3 "atelier" — colors are CSS variables from furx-theme.css (light :root / .dark).
   Utility classes (bg-bg, text-ink, border-rule, …) become theme-aware automatically.
   NOTE: alpha modifiers (text-ink/50) do NOT work on var() solid colors — use the
   *-dim/*-pale tokens or color-mix() in furx-theme.css instead. */
export default {
  content: ["./src/**/*.{ts,tsx,mdx}"],
  darkMode: "class",
  theme: {
    extend: {
      colors: {
        bg: "var(--bg)",
        "bg-soft": "var(--bg-2)",
        "bg-2": "var(--bg-2)",
        "bg-3": "var(--bg-3)",
        "bg-blur": "var(--bg-blur)",
        panel: "var(--bg-1)",
        "panel-2": "var(--bg-2)",
        ink: "var(--ink)",
        "ink-2": "var(--ink-2)",
        "ink-3": "var(--ink-3)",
        "ink-4": "var(--line-2)",
        rule: "var(--line)",
        "rule-2": "var(--line-2)",
        accent: "var(--accent)",
        "accent-bright": "var(--accent-2)",
        "accent-pale": "var(--accent-dim)",
        "accent-soft": "var(--accent-dim)",
        "accent-ink": "var(--bg)",
        clay: "var(--clay)",
        "clay-pale": "var(--clay-dim)",
        // Embedded terminal island — stays dark in both themes (a console is always dark).
        "term-bg": "#16130f",
        "term-bg-2": "#1e1a15",
        "term-line": "#322b21",
        "term-ink": "#efe7d8",
        "term-ink-2": "#b3a892",
        "term-ink-3": "#968b76",
        "term-accent": "#ff8a6e",
        ok: "var(--ok)",
        warn: "var(--warn)",
        err: "var(--err)",
        info: "var(--accent)",
        scrim: "rgba(8,6,4,0.42)",
      },
      fontFamily: {
        sans: ['"Hanken Grotesk"', "system-ui", "sans-serif"],
        display: ['"Fraunces"', "Georgia", "serif"],
        serif: ['"Fraunces"', "Georgia", "serif"],
        mono: ['"Space Mono"', '"SF Mono"', "ui-monospace", "monospace"],
      },
      maxWidth: { narrow: "720px", base: "980px", wide: "1280px" },
      boxShadow: {
        card: "var(--shadow)",
        lift: "var(--shadow-lift)",
        embed: "0 0 0 1px rgba(20,16,12,0.10), 0 24px 64px -20px rgba(20,16,12,0.40), 0 4px 16px rgba(20,16,12,0.10)",
      },
      borderRadius: { DEFAULT: "3px", md: "3px", lg: "5px" },
    },
  },
  plugins: [require("@tailwindcss/typography")],
} satisfies Config;
