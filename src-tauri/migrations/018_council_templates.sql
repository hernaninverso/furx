-- BLOQUE J ext · Council Templates (phase-aware presets)
-- Adds workflow-phase templates ON TOP of the provider-type presets.
-- Each template is a filter applied AFTER the preset narrows by provider type.
-- Council 3 unanimous (5/5): ship to both Free + Pro, no cloud dep.

CREATE TABLE IF NOT EXISTS council_templates (
  name TEXT PRIMARY KEY,             -- "planning" | "implementation" | "review" | "debug" | "refactor"
  display_name TEXT NOT NULL,
  description TEXT NOT NULL,
  model_filter TEXT NOT NULL,        -- pipe-separated substrings to match against provider+model name (case-insensitive)
  max_voices INTEGER NOT NULL DEFAULT 6,
  sort_order INTEGER NOT NULL DEFAULT 0,
  built_in INTEGER NOT NULL DEFAULT 1,  -- 1 = ships with (Furx) (cannot delete), 0 = user-defined
  created_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_council_templates_sort ON council_templates(sort_order);

-- 5 built-in templates (Council 3 unanimous naming):
INSERT OR IGNORE INTO council_templates
  (name, display_name, description, model_filter, max_voices, sort_order, built_in, created_at)
VALUES
  ('planning',
   'Planning Council',
   'Heavy reasoning models for spec, architecture, high-level decisions. Time-tolerant.',
   'opus|gpt-5|qwen-235b|qwen3-235b|deepseek-v3|deepseek-r1|o4-mini|o1',
   5, 1, 1, datetime('now')),
  ('implementation',
   'Implementation Council',
   'Fast + code-focused models for diff/code generation. Latency under 500ms p50.',
   'sonnet|codex|gemini-flash|gemini-2.5-flash|aider|llama-4|qwen-coder',
   4, 2, 1, datetime('now')),
  ('review',
   'Review Council',
   'Diverse model families for PR review, audit. Cross-family blind-spot detection.',
   'opus|gpt-5|gemini-2.5-pro|llama-4-maverick|qwen-235b|claude-sonnet',
   4, 3, 1, datetime('now')),
  ('debug',
   'Debug Council',
   'Stack-trace aware mix of cloud + local fast models to keep velocity.',
   'sonnet|codex|aider|qwen-coder|deepseek',
   4, 4, 1, datetime('now')),
  ('refactor',
   'Refactor Council',
   'Large-context models (>200K tokens) for cross-file refactors on big codebases.',
   'gemini-2.5-pro|opus|gpt-5|qwen-235b',
   3, 5, 1, datetime('now'));
