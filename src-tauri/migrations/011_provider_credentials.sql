-- BLOQUE 1 · WIZARD-1
-- Tabla provider_credentials: registro de cada provider que el user conecta vía Furx Connect.
-- Soporta los 5 caminos del wizard:
--   1. OpenRouter Quick Start (key_ref keychain, endpoint_url NULL → default)
--   2. Free tiers individuales (Cerebras, Groq, Mistral, SambaNova, Gemini Studio, OpenRouter free)
--   3. APIs pagas individuales (Anthropic, OpenAI, Gemini paga, custom)
--   4. Local inference (Ollama, LM Studio, llama.cpp, vLLM) — key_ref NULL, endpoint_url set
--   5. LiteLLM / proxy custom (key_ref + endpoint_url)
--
-- key_ref es el nombre del Keychain entry (service "furx-provider", account=alias).
-- Las keys NUNCA viven en la DB.

CREATE TABLE IF NOT EXISTS provider_credentials (
  alias TEXT PRIMARY KEY NOT NULL,           -- ej "openrouter-main", "ollama-local", "anthropic-personal"
  provider TEXT NOT NULL,                    -- enum (ver providers.rs::ProviderKind)
  key_ref TEXT,                              -- Keychain account name (NULL para Ollama / local sin auth)
  endpoint_url TEXT,                         -- override (Ollama 11434, vLLM custom, LiteLLM URL)
  status TEXT NOT NULL DEFAULT 'unconfigured', -- healthy | amber | red | unconfigured
  last_ping_ms INTEGER,                      -- latencia última (ms)
  last_ping_at TEXT,                         -- ISO-8601 timestamp último ping
  last_error_msg TEXT,                       -- mensaje de error último (council EDGE_1)
  scope_workspace TEXT,                      -- NULL = global; else workspace alias
  preset_member TEXT,                        -- comma-list: "quick,cheapo,frontier,local,mix"
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_provider_status ON provider_credentials(status);
CREATE INDEX IF NOT EXISTS idx_provider_kind ON provider_credentials(provider);

-- NOTA: a diferencia de la tabla `events` (append-only por triggers), `provider_credentials`
-- permite DELETE legítimo cuando el user desconecta un provider. Cada mutación se audita
-- desde Rust (providers.rs) escribiendo a `events` con kind="provider.{persist,delete,test}".
