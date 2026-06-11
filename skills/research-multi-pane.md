---
name: research-multi-pane
description: |
  Lanza la misma research-task EN PARALELO sobre los 4 paneles de Furx (Claude Max A, Claude Max B,
  Codex CLI, Gemini CLI, opcionalmente Aider). Cada modelo investiga independientemente usando
  WebSearch + MCPs (codebase-memory, mnemo recall, claude-mem search), después Furx sintetiza
  un diff report en markdown a `~/Desktop/Informes/Research-<topic>-<timestamp>.md`.

  Diferencial vs Cursor Canvas: Cursor genera React inline en agent window. Esta skill hace
  multi-pane RESEARCH con diff cross-LLM — único en su clase. Conecta con #1.10 disagreement detector.
trigger: "/research"
---

# Skill: Research Multi-Pane (Furx)

## Cuándo usarla

- "Investigá X" con N&gt;1 fuente de información (web + repo + memory + jurisprudencia/docs locales).
- "Comparame estas opciones" — multi-modelo evita sesgos individuales.
- "Qué pasa con Y" — research deep cross-modelo + síntesis convergente.
- "Spec de feature Z" — research → spec → diff de opiniones técnicas (4 modelos).

## Inputs esperados

- Topic / pregunta concreta (no abstracta).
- Repo path opcional para context (default = cwd).
- Output: `~/Desktop/Informes/Research-<slug>-<YYYY-MM-DD-HHmm>.md` o stdout si flag `--inline`.

## Cómo ejecuta

1. Cada pane recibe el mismo prompt con instrucciones:
   - Usar herramientas disponibles (WebSearch, mnemo recall, codebase-memory search_graph/trace_path).
   - Outputear sección con encabezado `## <Modelo>` + bullets + URLs/citas.
2. Esperar todas (timeout 60s por pane).
3. Furx sintetiza el report con secciones:
   - **Resumen ejecutivo** (1 párrafo, lo que TODOS los modelos acordaron).
   - **Por modelo** (4 sub-secciones con hallazgos detallados).
   - **Disagreements** (donde divergen — pista de incertidumbre real).
   - **Fuentes citadas** (consolidadas, deduplicadas).
   - **Próximos pasos** (action items derivados del consenso).
4. Audit log entry `research.completed` con metadata.

## Prompt template por pane

```
Estás investigando: "{topic}"

Contexto:
- Working directory: {cwd}
- Topic slug: {slug}
- Otros modelos investigando esto mismo: { Claude Max A, Claude Max B, Codex, Gemini } (sin coordinación entre ustedes)

Usá las herramientas que tengas (WebSearch, mnemo recall, codebase-memory, file reads).
Output formato:

## {modelo}
- Hallazgo 1 (con URL/cita)
- Hallazgo 2
...

### Confidence: high|medium|low
### Tiempo: ~Ns
### URLs: <lista>
```

## Sinergia con otros skills/features Furx

- **disagreement-detector**: cuando esta skill emite el report, el detector
  marca las divergencias automáticamente.
- **session-replay** (Pro): el run del research queda capturable + replayeable.
- **audit-log**: cada research run audita correlation_id que une los 4 panes.

## Limitaciones conocidas

- Necesita ≥2 panes activos para que tenga sentido (con 1 solo es un Claude/Codex chat normal).
- WebSearch availability depende del CLI (Claude Code tiene WebSearch, Aider no).
- Costo: ver CouncilModal estimator — promedio $0.001-0.01 USD por run en preset Frontier.

## Ejemplos de uso

```
/research "best practices for Tauri Mobile iOS 2026"
/research "Anthropic API rate limits free tier comparison" --inline
/research "vLLM vs LiteLLM proxy production tradeoffs"
```
