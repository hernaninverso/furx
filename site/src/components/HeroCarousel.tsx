"use client";
import { useEffect, useState } from "react";

const SLIDES = [
  { id: "memory", label: "Memory Hub · cross-CLI" },
  { id: "voice", label: "Voice → active pane" },
  { id: "mobile", label: "Mobile remote control" },
] as const;

type SlideId = (typeof SLIDES)[number]["id"];

export default function HeroCarousel() {
  const [active, setActive] = useState<SlideId>("memory");
  const [paused, setPaused] = useState(false);

  useEffect(() => {
    if (paused) return;
    const t = setInterval(() => {
      setActive((cur) => {
        const idx = SLIDES.findIndex((s) => s.id === cur);
        return SLIDES[(idx + 1) % SLIDES.length].id;
      });
    }, 7000);
    return () => clearInterval(t);
  }, [paused]);

  return (
    <div className="w-full" onMouseEnter={() => setPaused(true)} onMouseLeave={() => setPaused(false)}>
      <div className="embed relative">
        {/* Traffic + tab strip */}
        <div className="flex items-center gap-2 px-3.5 py-2.5 bg-term-bg-2 border-b border-term-line">
          <span className="w-2.5 h-2.5 rounded-full bg-[#ff5f57]" />
          <span className="w-2.5 h-2.5 rounded-full bg-[#febc2e]" />
          <span className="w-2.5 h-2.5 rounded-full bg-[#28c840]" />
          <div className="ml-3 flex items-center gap-3 text-[11px] font-mono text-term-ink-2">
            <span>~/furx</span>
            <span className="text-term-accent">▸ {SLIDES.find((s) => s.id === active)?.label}</span>
          </div>
          <div className="ml-auto flex gap-1.5">
            {SLIDES.map((s) => (
              <button
                key={s.id}
                onClick={() => setActive(s.id)}
                aria-label={`Show ${s.label}`}
                className={`w-1.5 h-1.5 rounded-full transition-all ${active === s.id ? "bg-term-accent w-4" : "bg-term-ink-3 hover:bg-term-ink-2"}`}
              />
            ))}
          </div>
        </div>

        {/* Slides */}
        <div className="relative min-h-[360px]">
          {active === "memory" && <MemorySlide />}
          {active === "voice" && <VoiceSlide />}
          {active === "mobile" && <MobileSlide />}
        </div>
      </div>
      <div className="flex items-center justify-between mt-3 px-1">
        <div className="text-xs text-ink-3 font-mono">
          {SLIDES.findIndex((s) => s.id === active) + 1} / {SLIDES.length} · auto-rotate {paused ? "paused" : "every 7s"}
        </div>
        <div className="text-xs text-ink-3 font-mono">hover to pause</div>
      </div>
    </div>
  );
}

/* ──────────────────────────── Slide 1: Memory Hub cross-CLI ──────────────────────────── */

function MemorySlide() {
  return (
    <div className="grid grid-cols-2 gap-px bg-term-line min-h-[360px]">
      {/* Pane A: Claude writes a note */}
      <div className="bg-term-bg p-4 flex flex-col">
        <div className="text-[10px] font-mono uppercase tracking-wider text-term-ink-3 mb-2 flex items-center gap-2">
          <span className="w-1.5 h-1.5 rounded-full bg-[#FF5C35]" />
          claude-A · pane 01
        </div>
        <div className="font-mono text-[12px] text-term-ink leading-relaxed flex-1">
          <div className="text-term-accent">$ remember "auth uses bcrypt 12-round salt, rotate keys quarterly"</div>
          <div className="text-term-ink-2 mt-1.5">→ memory.write ok · id mem_3f2a · indexed · 18ms</div>
          <div className="mt-3 text-term-ink-3 text-[11px]">// 4 seconds later, in another pane…</div>
        </div>
      </div>

      {/* Pane B: Codex recalls */}
      <div className="bg-term-bg-2 p-4 flex flex-col">
        <div className="text-[10px] font-mono uppercase tracking-wider text-term-ink-3 mb-2 flex items-center gap-2">
          <span className="w-1.5 h-1.5 rounded-full bg-[#f4b860]" />
          codex · pane 02
        </div>
        <div className="font-mono text-[12px] text-term-ink leading-relaxed flex-1">
          <div className="text-term-accent">$ recall "auth"</div>
          <div className="text-term-ink-2 mt-1.5">
            <div>1. <span className="text-term-ink">mem_3f2a</span> · claude-A · 4s ago</div>
            <div className="pl-3 mt-0.5">&quot;auth uses bcrypt 12-round salt,</div>
            <div className="pl-3">rotate keys quarterly&quot;</div>
          </div>
          <div className="mt-3 text-term-ink-3 text-[11px]">
            cross-CLI memory · SQLite FTS5 · local
          </div>
        </div>
      </div>

      {/* Status footer */}
      <div className="col-span-2 bg-term-bg-2 border-t border-term-line px-4 py-2.5 flex items-center justify-between text-[11px] font-mono text-term-ink-3">
        <span><span className="text-term-accent">memory_hub</span> · 1 write · 1 read · 23ms total</span>
        <span>~/.furx/memory.db · ∞ retention · 0 bytes leaked</span>
      </div>
    </div>
  );
}

/* ──────────────────────────── Slide 2: Voice → active pane ──────────────────────────── */

function VoiceSlide() {
  return (
    <div className="bg-term-bg p-5 min-h-[360px] flex flex-col">
      {/* Toolbar */}
      <div className="flex items-center gap-3 mb-4">
        <div className="text-[10px] font-mono uppercase tracking-wider text-term-ink-3 flex items-center gap-2">
          <span className="w-1.5 h-1.5 rounded-full bg-[#FF5C35] animate-pulse" />
          recording · 2.3 s
        </div>
        <div className="ml-auto flex items-center gap-2 text-[10px] font-mono text-term-ink-3">
          <span className="text-term-accent">⌘⇧M</span>
          <span>→ active pane: <span className="text-term-ink">claude-A</span></span>
        </div>
      </div>

      {/* Waveform */}
      <div className="flex items-end gap-[2px] h-14 mb-4 px-1">
        {Array.from({ length: 56 }).map((_, i) => {
          const h = 18 + Math.abs(Math.sin(i * 0.7 + i * 0.13) * 32);
          return (
            <span
              key={i}
              className="flex-1 rounded-sm transition-all"
              style={{
                height: `${Math.min(h, 56)}px`,
                background: i < 38 ? "#FF5C35" : "#21262e",
              }}
            />
          );
        })}
      </div>

      {/* Live transcription */}
      <div className="font-mono text-[13px] text-term-ink leading-relaxed mb-4">
        <div className="text-term-ink-3 text-[10px] uppercase tracking-wider mb-1">live transcript · whisper.cpp local</div>
        <div>
          <span className="text-term-ink">refactor the auth module to use bcrypt with</span>
          <span className="text-term-ink-2"> twelve-round salt and rotate</span>
          <span className="text-term-accent animate-pulse">▌</span>
        </div>
      </div>

      {/* Output (writing into pane) */}
      <div className="bg-term-bg-2 border border-term-line rounded p-3 font-mono text-[12px] text-term-ink mt-auto">
        <div className="text-[10px] uppercase tracking-wider text-term-ink-3 mb-1.5">writing to pane: <span className="text-term-accent">claude-A</span></div>
        <div className="text-term-ink-2">claude-A &gt; </div>
        <div className="text-term-ink">refactor the auth module to use bcrypt with twelve-round salt and rotate<span className="text-term-accent">▌</span></div>
      </div>

      <div className="text-[11px] font-mono text-term-ink-3 mt-3">
        whisper-cli tiny.en · ~/.furx/whisper · audio deleted on transcribe · zero cloud
      </div>
    </div>
  );
}

/* ──────────────────────────── Slide 3: Mobile remote control ──────────────────────────── */

function MobileSlide() {
  return (
    <div className="bg-term-bg p-5 min-h-[360px] grid grid-cols-[200px_1fr] gap-5 items-stretch">
      {/* Phone */}
      <div className="bg-term-bg-2 border border-term-line rounded-2xl p-3 flex flex-col self-start">
        <div className="text-[9px] font-mono text-term-ink-3 mb-2 flex justify-between">
          <span>iPhone · Furx Companion</span>
          <span className="text-term-accent">●</span>
        </div>
        <div className="bg-[#0a0c10] rounded-lg p-3 flex-1 flex flex-col gap-2 text-[11px] font-mono">
          <div className="text-term-ink-3 text-[9px] uppercase tracking-wider">desktop: hernan-mbp</div>
          <div className="text-term-ink">
            <span className="text-term-accent">›</span> /prompt: <span className="text-term-ink-2">explain the bcrypt rotation</span>
          </div>
          <div className="bg-term-bg-2 border border-term-line rounded p-2 mt-1">
            <div className="text-[9px] text-term-ink-3 uppercase tracking-wider mb-1">tool call · approve?</div>
            <div className="text-term-ink text-[10px] leading-snug">claude-A wants to read /src/auth.py</div>
            <div className="flex gap-1.5 mt-2">
              <button className="bg-[#3b8a55] text-[10px] px-2 py-1 rounded text-bg font-semibold">approve</button>
              <button className="bg-term-line text-[10px] px-2 py-1 rounded text-term-ink-2">deny</button>
            </div>
          </div>
          <div className="text-term-ink-3 text-[9px] mt-auto">over Tailscale · HMAC-SHA256 · mDNS auto-discovery</div>
        </div>
      </div>

      {/* Desktop receiving */}
      <div className="bg-term-bg-2 border border-term-line rounded-lg p-4 flex flex-col">
        <div className="text-[10px] font-mono uppercase tracking-wider text-term-ink-3 mb-3 flex items-center gap-2">
          <span className="w-1.5 h-1.5 rounded-full bg-[#FF5C35]" />
          desktop · claude-A (pane 01) · receiving from iphone
        </div>
        <div className="font-mono text-[12px] text-term-ink leading-relaxed flex-1">
          <div className="text-term-accent">[mobile] /prompt received · 14:02:31</div>
          <div className="text-term-ink-2 mt-1">→ explain the bcrypt rotation</div>
          <div className="mt-3 text-term-ink-3 text-[11px]">claude-A is thinking…</div>
          <div className="mt-3 bg-[#0a0c10] rounded p-2.5 border border-term-line">
            <div className="text-term-ink-3 text-[10px] uppercase tracking-wider mb-1">tool call requested</div>
            <div className="text-term-ink text-[11px]">Read file: /src/auth.py (1.2 KB)</div>
            <div className="text-term-ink-3 text-[10px] mt-1 italic">awaiting approval from mobile…</div>
          </div>
        </div>
        <div className="text-[11px] font-mono text-term-ink-3 mt-3 pt-3 border-t border-term-line">
          mobile_bridge.rs · ws://127.0.0.1:43118 · nonce + ts skew ±60s
        </div>
      </div>
    </div>
  );
}
