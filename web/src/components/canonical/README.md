# Componentes canónicos — US8 (spec `015-frontend-reform-kernel`)

Cimiento de la reforma del frontend: un set chico de componentes **canónicos**
que reemplazan los patrones ad-hoc dispersos por la app. Todos consumen **sólo
tokens semánticos** (ver abajo) y respetan la estética **V3 atelier** (F-VI):
hairlines ruled, labels en Space Mono uppercase, títulos en Fraunces wonky.

> Esta es la base. La migración masiva de los ~35 modales / 22 vistas a estos
> componentes se hace en olas posteriores. Acá sólo se migró **un** modal
> (`MergeReviewModal`) como prueba de que el patrón funciona.

## Capa de tokens semánticos

`web/src/styles/tokens.css` define una capa **semántica** sobre las primitivas
V3 atelier que ya viven en `web/src/styles.css` (`:root` light / `.dark` dark).
**No inventa colores**: cada token apunta con `var()` a una primitiva V3
existente, así un theme switch / brand refresh sigue siendo un solo lugar.
Como las primitivas V3 ya flipean light/dark por la clase `.dark`, los tokens
semánticos heredan el theme automáticamente (estructura lista para dark futuro;
el dark V3 ya existe).

CSS de los componentes: `web/src/styles/canonical.css` (clases `fxc-*`).
Ambas hojas se `@import`an desde `styles.css` (después de las primitivas V3).

### Tokens definidos

| Grupo | Tokens |
|---|---|
| `--color-bg-*` | `base` `subtle` `surface` `elevated` `overlay` `hover` `active` `accent` |
| `--color-fg-*` | `default` `muted` `subtle` `accent` `on-accent` |
| `--color-border-*` | `default` `strong` `accent` |
| `--color-intent-*` | `danger` / `danger-bg` · `warning` / `warning-bg` · `success` / `success-bg` · `info` / `info-bg` |
| `--space-*` | `xs`(4) `sm`(8) `md`(12) `lg`(16) `xl`(24) `2xl`(32) `3xl`(48) |
| `--radius-*` | `sm`(3) `md`(5) `lg`(8) `pill`(999) |
| `--font-*` | `display` (Fraunces) · `body` (Hanken Grotesk) · `mono` (Space Mono) |
| `--shadow-*` | `low` `mid` `high` `modal` |
| `--z-*` | `base` `pane` `sticky` `drawer` `overlay`(999) `modal`(1000) `toast` `tooltip` |

## Componentes

| Componente | Reemplaza | Estados |
|---|---|---|
| **`ModalFrame`** | El boilerplate por-modal (`Modal` + header/footer/`wizard-actions` a mano) en ~35 modales. Envuelve el `<Modal>` probado (focus-trap, ESC, backdrop, body-scroll-lock, portal, z-stack) y aporta estructura canónica. | `loading` (spinner), `error` (bloque), `danger` (tono clay), default |
| **`PageHeader`** | Encabezados ad-hoc de vistas (eyebrow + título + descripción + acciones). | default |
| **`EmptyState`** | Empty-states accionables dispersos (icono + título + descripción + acción). | `default`, `error` |
| **`DangerZone`** | Bloques de acciones destructivas sin patrón común. Confirmación visual opcional (`confirmPhrase` → gate tipeado, expone `confirmed` por render-prop). No ejecuta: la puerta real es Rust (US4). | gate confirmado / sin confirmar |
| **`CommandRow`** | Fila de comando para la futura ⌘K palette (US2): icono + label + descripción + shortcut + badge de riesgo (`safe`/`destructive`/`credential`/`external`, alineado al Command Registry US1). | `active` (teclado), `hover`, `disabled` |

## Uso

```tsx
import { ModalFrame, PageHeader, EmptyState, DangerZone, CommandRow } from "components/canonical";
```

`ModalFrame` ejemplo (ver migración real en `web/src/components/MergeReviewModal.tsx`):

```tsx
<ModalFrame
  title="Merge review · main"
  subtitle="diff stat + risky paths"
  loading={loading}
  error={error}
  footer={<button className="fxc-btn" onClick={onClose}>Cerrar</button>}
  onClose={onClose}
>
  {/* body scrollable */}
</ModalFrame>
```

## Inventario visual baseline

El spec menciona screenshots de rutas principales como baseline. En esta ola se
deja el inventario **estructural** (este README + el catálogo de tokens) en vez
de screenshots automáticos: la app es Tauri (no hay harness headless de
screenshot en el repo) y el gate de la ola es el build TS + Vite limpio. Los
screenshots de regresión visual quedan para la ola de migración masiva, cuando
haya N pantallas migradas que comparar.
