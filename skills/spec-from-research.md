---
name: spec-from-research
description: |
  Toma un report de research-multi-pane y lo convierte en un spec-kit `specify` draft.
  Sintetiza requirements, NFRs, edge cases, plan de implementación.
  Output: `specs/<topic>/SPEC.md` + `specs/<topic>/PLAN.md` listos para `/speckit.tasks`.
trigger: "/spec-from-research"
---

# Skill: Spec from Research

## Cuándo usarla

Cuando ya corriste `/research <topic>` y tenés el report en `~/Desktop/Informes/Research-*.md`.
Esta skill convierte el report (research) en un spec ejecutable (build).

## Cómo ejecuta

1. Lee el último Research-*.md (o el path provisto).
2. Detecta secciones: Resumen, Por modelo, Disagreements, Próximos pasos.
3. Convierte "próximos pasos" en SPEC.md con formato spec-kit.
4. Para cada paso, genera PLAN.md con tareas atómicas (≤4h cada una).
5. Audit log `spec.generated` con correlation_id linkeado al research run.

## Output

```
~/proyectos/<project>/specs/<topic>/
├── SPEC.md          # spec-kit specify format
├── PLAN.md          # impl plan + risk register
└── README.md        # link back to Research-*.md
```

## Sinergia

- `research-multi-pane` → `spec-from-research` → `/speckit.tasks` → `/speckit.implement` (loop completo).
- `disagreement-detector`: si el research tenía divergencias high-confidence, este skill las marca
  como **uncertainties** en el SPEC.md para que el plan las explicite.
