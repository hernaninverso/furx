export const TOOL_URLS = {
  tauri: "https://tauri.app",
  ollama: "https://ollama.com",
  openrouter: "https://openrouter.ai",
  cerebras: "https://cerebras.ai",
  groq: "https://groq.com",
  mistral: "https://mistral.ai",
  sambanova: "https://sambanova.ai",
  anthropic: "https://anthropic.com",
  openai: "https://openai.com",
  gemini: "https://ai.google.dev",
  litellm: "https://litellm.ai",
  paddle: "https://paddle.com",
  mcp: "https://modelcontextprotocol.io",
  pagefind: "https://pagefind.app",
  plausible: "https://plausible.io",
} as const;

export type ToolName = keyof typeof TOOL_URLS;

export const TOOL_LABELS: Record<ToolName, string> = {
  tauri: "Tauri",
  ollama: "Ollama",
  openrouter: "OpenRouter",
  cerebras: "Cerebras",
  groq: "Groq",
  mistral: "Mistral",
  sambanova: "SambaNova",
  anthropic: "Anthropic",
  openai: "OpenAI",
  gemini: "Google Gemini",
  litellm: "LiteLLM",
  paddle: "Paddle",
  mcp: "MCP",
  pagefind: "Pagefind",
  plausible: "Plausible",
};
