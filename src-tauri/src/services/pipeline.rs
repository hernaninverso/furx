//! 029 F0 · Pipelines de orquestación declarativos (YAML, estilo Conductor).
//!
//! Define un pipeline multi-agente como YAML → `PipelineSpec` tipado y VALIDADO que mapea a la
//! primitiva existente `orchestration::{create_batch, TaskSpec}`. Convierte "definir un pipeline de N
//! agentes con dependencias" en config versionable, no código (tesis 022: cero hardcode).
//!
//! F0 = parse + validate + topo (PURO, no ejecuta). F1 = mapear a `create_batch` + comando Tauri.
//!
//! INVARIANTES:
//!  - **determinista**: parse/validate/topo sin estado ni red.
//!  - **fail-safe de validación**: un pipeline con ciclo / ref inexistente / id duplicado / límites
//!    excedidos se RECHAZA con error claro (nunca produce un plan inválido).
//!  - **argv-only**: los campos (title/objective) son datos; el spawn de cada task usa la vía argv
//!    del orquestador existente (NO shell).

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

/// Límites (defensa anti-DoS/abuso).
const MAX_TASKS: usize = 64;
const MAX_DEPS_PER_TASK: usize = 16;
const MAX_NAME_LEN: usize = 120;
const MAX_TITLE_LEN: usize = 200;
const MAX_OBJECTIVE_LEN: usize = 8000;
const MAX_ID_LEN: usize = 48;
const MAX_AGENT_LEN: usize = 64;
/// Tope de bytes del YAML de entrada ANTES de parsear (council ALTA: mitiga "YAML bomb" / expansión
/// exponencial de anchors&aliases). El threat model es config LOCAL autorada por el usuario, no input
/// adversarial de red; aun así acotamos el source para bound el parse. Un pipeline legítimo (≤64
/// tasks con objetivos) entra holgado en 256 KB.
const MAX_YAML_BYTES: usize = 256 * 1024;

/// Una etapa/tarea del pipeline.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PipelineTask {
    /// Id estable de la tarea dentro del pipeline (referenciable por `depends_on`).
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub objective: String,
    /// Agente que ejecuta la tarea, por **slug/nombre PORTABLE** (no por id de DB) — council MEDIA:
    /// referenciar por id acopla el pipeline a una máquina. Se resuelve a `agent_profile_id` en la
    /// ejecución (F1). `None` = el orquestador resuelve por mode/default.
    #[serde(default)]
    pub agent: Option<String>,
    /// Mode/cli override. `None` = derivado del perfil.
    #[serde(default)]
    pub mode: Option<String>,
    /// Ids de tareas de las que ésta DEPENDE (deben completar antes). Vacío = sin dependencias.
    #[serde(default)]
    pub depends_on: Vec<String>,
}

/// Un pipeline declarativo completo.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PipelineSpec {
    pub name: String,
    #[serde(default)]
    pub repo: Option<String>,
    #[serde(default)]
    pub base_branch: Option<String>,
    pub tasks: Vec<PipelineTask>,
}

// ── Parse ─────────────────────────────────────────────────────────────────────

/// ¿El YAML contiene un anchor (`&name`) o alias (`*name`) en CUALQUIER posición de token?
///
/// Scanner char-level QUOTE-AWARE (audit codex: el detector line-based previo no cazaba aliases en
/// flow inline `[*x]`/`{k: *x}` ni anchors tras tags `!!str &x`). Marca un `&`/`*` como token YAML
/// (no contenido de string) cuando: NO está dentro de comillas simples/dobles, está al inicio o
/// precedido por whitespace o un delimitador de flow (`[ { ,`) o `:`, y el SIGUIENTE byte existe y NO
/// es whitespace ni un delimitador YAML (`[]{}:,`) — así caza también nombres no-alnum (`&-x`/`*-x`).
/// Caza `&t`, `*d`, `[*x]`, `{k: *x}`, `!!str &a x`, `&-x`, pero NO `"A & B"` ni `"*.rs"` (el `&`/`*`
/// queda dentro de comillas) ni `a * b` (el `*` va seguido de espacio → no es token de nodo).
fn has_yaml_anchor_or_alias(yaml: &str) -> bool {
    let bytes = yaml.as_bytes();
    let mut in_single = false;
    let mut in_double = false;
    let mut prev: u8 = b'\n';
    for (i, &c) in bytes.iter().enumerate() {
        match c {
            b'\'' if !in_double => in_single = !in_single,
            b'"' if !in_single => in_double = !in_double,
            b'&' | b'*' if !in_single && !in_double => {
                let boundary =
                    matches!(prev, b'\n' | b'\r' | b' ' | b'\t' | b'[' | b'{' | b',' | b':');
                // Audit codex: NO exigir alnum — un nombre de anchor/alias puede empezar con `-`/`!`
                // (`&-x`, `*-x`). Marcar si el siguiente byte EXISTE y no es whitespace ni un
                // delimitador YAML (`[]{}:,`): por la gramática YAML, `&`/`*` en boundary seguido de
                // un char de nodo ES un anchor/alias. (`* ` o `*` al final NO se marcan.)
                let next_is_node_char = bytes
                    .get(i + 1)
                    .map(|&n| {
                        !n.is_ascii_whitespace()
                            && !matches!(n, b'[' | b']' | b'{' | b'}' | b':' | b',')
                    })
                    .unwrap_or(false);
                if boundary && next_is_node_char {
                    return true;
                }
            }
            _ => {}
        }
        prev = c;
    }
    false
}

/// Parsea un YAML de pipeline. YAML inválido → Err claro (sin panic). Mitigaciones anti "YAML bomb"
/// (council ALTA + audit codex): (1) rechaza inputs > `MAX_YAML_BYTES` ANTES de parsear; (2) rechaza
/// anchors/aliases (`&`/`*`) en cualquier posición (scanner quote-aware) — un pipeline declarativo no
/// los necesita y son el vector de expansión exponencial. Threat model: config LOCAL del usuario.
pub fn parse_yaml(yaml: &str) -> Result<PipelineSpec> {
    if yaml.len() > MAX_YAML_BYTES {
        return Err(anyhow!(
            "YAML de pipeline demasiado grande ({} bytes, máx {MAX_YAML_BYTES})",
            yaml.len()
        ));
    }
    if has_yaml_anchor_or_alias(yaml) {
        return Err(anyhow!(
            "YAML de pipeline con anchors/aliases (&/*) no soportado: un pipeline declarativo no los necesita (mitigación anti expansión exponencial)"
        ));
    }
    serde_yaml::from_str(yaml).map_err(|e| anyhow!("YAML de pipeline inválido: {e}"))
}

// ── Validación ────────────────────────────────────────────────────────────────

/// Charset seguro para ids: `[A-Za-z0-9_-]`.
fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_ID_LEN
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// Un campo de texto libre (title/objective/name): sin control chars (salvo `\n`/`\t` en objective),
/// acotado. No se ejecuta por shell (es dato), pero rechazamos control chars de terminal por higiene.
fn valid_text(s: &str, max: usize, allow_newlines: bool) -> bool {
    s.len() <= max
        && s.chars().all(|c| {
            !c.is_control() || (allow_newlines && (c == '\n' || c == '\t' || c == '\r'))
        })
}

/// Valida un PipelineSpec completo. Determinista, fail-safe: cualquier inconsistencia → Err.
pub fn validate(spec: &PipelineSpec) -> Result<()> {
    let name = spec.name.trim();
    if name.is_empty() || !valid_text(name, MAX_NAME_LEN, false) {
        return Err(anyhow!("name del pipeline inválido (no vacío, sin control chars, ≤120)"));
    }
    if spec.tasks.is_empty() {
        return Err(anyhow!("el pipeline necesita al menos 1 task"));
    }
    if spec.tasks.len() > MAX_TASKS {
        return Err(anyhow!("demasiadas tasks (máx {MAX_TASKS})"));
    }
    // Ids únicos + charset.
    let mut ids = HashSet::new();
    for t in &spec.tasks {
        if !valid_id(&t.id) {
            return Err(anyhow!("id de task inválido {:?} (permitido [A-Za-z0-9_-]{{1,48}})", t.id));
        }
        if !ids.insert(t.id.as_str()) {
            return Err(anyhow!("id de task duplicado: {}", t.id));
        }
        let title = t.title.trim();
        if title.is_empty() || !valid_text(title, MAX_TITLE_LEN, false) {
            return Err(anyhow!("title inválido en task {}", t.id));
        }
        if !valid_text(&t.objective, MAX_OBJECTIVE_LEN, true) {
            return Err(anyhow!("objective inválido en task {} (control chars o muy largo)", t.id));
        }
        if t.depends_on.len() > MAX_DEPS_PER_TASK {
            return Err(anyhow!("task {} tiene demasiadas dependencias (máx {MAX_DEPS_PER_TASK})", t.id));
        }
        // `agent` (si está): SLUG portable estricto `[A-Za-z0-9_-.]` ≤64 (audit codex: alinear con la
        // convención del repo `agent_profiles::valid_slug`; sin espacios, `/`, `;`, unicode). Se
        // resuelve a un agent_profile_id en la ejecución (F1) — F0 NO lo mapea a id (evita persistir
        // un slug como id local).
        if let Some(agent) = &t.agent {
            let ok = !agent.is_empty()
                && agent.len() <= MAX_AGENT_LEN
                && agent
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.');
            if !ok {
                return Err(anyhow!(
                    "agent inválido en task {} (slug portable [A-Za-z0-9_-.] ≤64, sin espacios/rutas/unicode)",
                    t.id
                ));
            }
        }
    }
    // depends_on: referencias existen, no auto-dependencia.
    for t in &spec.tasks {
        for dep in &t.depends_on {
            if dep == &t.id {
                return Err(anyhow!("task {} depende de sí misma", t.id));
            }
            if !ids.contains(dep.as_str()) {
                return Err(anyhow!("task {} depende de un id inexistente: {}", t.id, dep));
            }
        }
    }
    // Sin ciclos (el topo-sort falla si hay ciclo).
    topo_order(spec)?;
    Ok(())
}

// ── Orden topológico (etapas por dependencias) ────────────────────────────────

/// Devuelve el orden de ejecución (Kahn): cada task aparece DESPUÉS de todas sus `depends_on`.
/// Err si hay un ciclo O una referencia colgada (audit codex: fail-safe STANDALONE — no asume un
/// spec ya validado; un `depends_on` a un id inexistente es Err, no se ignora). Determinista: ante
/// empate, respeta el orden de declaración. Panic-free (sin index/unwrap sobre input).
pub fn topo_order(spec: &PipelineSpec) -> Result<Vec<String>> {
    let ids: Vec<&str> = spec.tasks.iter().map(|t| t.id.as_str()).collect();
    let id_set: HashSet<&str> = ids.iter().copied().collect();
    // Audit codex: ids duplicados → Err (fail-safe standalone; con dups el orden sería ambiguo y
    // `out.len()==ids.len()` no detectaría el problema porque `id_set` colapsa los dups).
    if id_set.len() != ids.len() {
        return Err(anyhow!("hay ids de task duplicados"));
    }
    // in-degree + adyacencia (dep → dependiente).
    let mut indeg: HashMap<&str, usize> = ids.iter().map(|&i| (i, 0usize)).collect();
    let mut adj: HashMap<&str, Vec<&str>> = HashMap::new();
    for t in &spec.tasks {
        for dep in &t.depends_on {
            // Audit codex: una ref colgada es Err (fail-safe standalone), NO se ignora silenciosa.
            if !id_set.contains(dep.as_str()) {
                return Err(anyhow!(
                    "task {} depende de un id inexistente: {} (referencia colgada)",
                    t.id,
                    dep
                ));
            }
            adj.entry(dep.as_str()).or_default().push(t.id.as_str());
            if let Some(d) = indeg.get_mut(t.id.as_str()) {
                *d += 1;
            }
        }
    }
    // Cola inicial: in-degree 0, en orden de declaración (determinismo).
    let mut queue: Vec<&str> = ids
        .iter()
        .copied()
        .filter(|i| indeg.get(i).copied().unwrap_or(0) == 0)
        .collect();
    let mut out: Vec<String> = Vec::with_capacity(ids.len());
    let mut head = 0;
    while head < queue.len() {
        let n = queue[head];
        head += 1;
        out.push(n.to_string());
        if let Some(deps) = adj.get(n) {
            // procesar en orden de declaración para determinismo
            for &m in deps {
                if let Some(d) = indeg.get_mut(m) {
                    *d -= 1;
                    if *d == 0 {
                        queue.push(m);
                    }
                }
            }
        }
    }
    if out.len() != ids.len() {
        return Err(anyhow!(
            "el pipeline tiene un ciclo de dependencias (no se puede ordenar topológicamente)"
        ));
    }
    Ok(out)
}

// El mapeo a `orchestration::TaskSpec` vive en F1 (ejecución): requiere RESOLVER el slug `agent`
// portable a un `agent_profile_id` LOCAL (audit codex: F0 no debe meter un slug donde va un id). F0
// se queda en parse + validate + topo_order (el contrato declarativo puro).

#[cfg(test)]
mod tests {
    use super::*;

    const VALID: &str = r#"
name: "mi-pipeline"
base_branch: "main"
tasks:
  - id: impl
    title: "Implementar feature"
    objective: "hacé X"
    agent: "claude-prof"
  - id: test
    title: "Tests"
    objective: "testeá X"
    agent: "codex-prof"
    depends_on: [impl]
  - id: review
    title: "Review"
    agent: "gemini-prof"
    depends_on: [impl, test]
"#;

    /// SC-001: un YAML válido parsea+valida; el orden topológico arranca con impl.
    #[test]
    fn valid_pipeline_parses_and_validates() {
        let spec = parse_yaml(VALID).unwrap();
        assert_eq!(spec.name, "mi-pipeline");
        assert_eq!(spec.tasks.len(), 3);
        validate(&spec).unwrap();
        let order = topo_order(&spec).unwrap();
        assert_eq!(order.len(), 3);
        assert_eq!(order[0], "impl");
    }

    /// Audit codex: `topo_order` STANDALONE rechaza una ref colgada (no la ignora).
    #[test]
    fn topo_order_rejects_dangling_ref_standalone() {
        let yaml = "name: p\ntasks:\n  - id: a\n    title: \"A\"\n    depends_on: [ghost]\n";
        let spec = parse_yaml(yaml).unwrap();
        assert!(topo_order(&spec).is_err());
    }

    /// Audit codex: `agent` debe ser slug estricto — espacios/`/`/`;` se rechazan.
    #[test]
    fn agent_must_be_strict_slug() {
        for bad in ["claude prof", "a/b", "a;rm", "a\u{00e9}b"] {
            let yaml = format!("name: p\ntasks:\n  - id: a\n    title: \"A\"\n    agent: \"{bad}\"\n");
            let spec = parse_yaml(&yaml).unwrap();
            assert!(validate(&spec).is_err(), "agent {bad:?} debió rechazarse");
        }
        // slug válido pasa.
        let yaml = "name: p\ntasks:\n  - id: a\n    title: \"A\"\n    agent: claude-prof.v2\n";
        let spec = parse_yaml(yaml).unwrap();
        assert!(validate(&spec).is_ok());
    }

    /// Audit codex: anchors/aliases YAML se rechazan en CUALQUIER posición (scanner quote-aware),
    /// incluyendo flow inline `[*x]`/`{k: *x}` y anchors tras tag `!!str &x`; pero `&`/`*` dentro de
    /// comillas NO false-positivea.
    #[test]
    fn yaml_anchors_and_aliases_rejected() {
        let casos_malos = [
            "name: p\ntasks:\n  - id: a\n    title: &big \"x\"\n",          // anchor en valor
            "name: p\ndefaults: &d hola\ntasks:\n  - id: a\n    title: *d\n", // alias en valor
            "name: p\ntasks:\n  - id: a\n    title: \"A\"\n    depends_on: [*x]\n", // alias en flow seq
            "name: p\nx: &x [a, b]\ntasks:\n  - id: a\n    title: \"A\"\n    depends_on: [*x]\n",
            "name: p\nm: {k: *a}\ntasks:\n  - id: a\n    title: \"A\"\n",     // alias en flow map
            "name: p\ntasks:\n  - id: a\n    title: !!str &t A\n",            // anchor tras tag
            "name: p\ntasks:\n  - id: a\n    title: &-x \"x\"\n",             // anchor nombre no-alnum
            "name: p\nd: &-x hola\ntasks:\n  - id: a\n    title: *-x\n",       // alias nombre no-alnum
            "name: p\nm: {k: *-x}\ntasks:\n  - id: a\n    title: \"A\"\n",     // alias no-alnum en flow map
        ];
        for c in casos_malos {
            assert!(parse_yaml(c).is_err(), "debió rechazar anchor/alias: {c:?}");
        }
        // `&`/`*` DENTRO de strings entre comillas NO false-positivea.
        let ok = "name: p\ntasks:\n  - id: a\n    title: \"A & B\"\n    objective: \"usar *.rs y x & y\"\n";
        let spec = parse_yaml(ok).unwrap();
        assert!(validate(&spec).is_ok());
        // Una multiplicación tipo `a * b` (precedida por identificador, no boundary) NO false-positivea.
        let mult = "name: p\ntasks:\n  - id: a\n    title: \"A\"\n    objective: area = base * altura\n";
        assert!(parse_yaml(mult).is_ok());
    }

    /// Audit codex: `topo_order` STANDALONE rechaza ids duplicados (no devuelve orden ambiguo).
    #[test]
    fn topo_order_rejects_duplicate_ids_standalone() {
        let yaml = "name: p\ntasks:\n  - id: a\n    title: \"A1\"\n  - id: a\n    title: \"A2\"\n";
        let spec = parse_yaml(yaml).unwrap();
        assert!(topo_order(&spec).is_err());
    }

    /// SC-005: topo_order respeta dependencias (impl antes que test antes que review).
    #[test]
    fn topo_respects_dependencies() {
        let spec = parse_yaml(VALID).unwrap();
        let order = topo_order(&spec).unwrap();
        let pos = |id: &str| order.iter().position(|x| x == id).unwrap();
        assert!(pos("impl") < pos("test"));
        assert!(pos("impl") < pos("review"));
        assert!(pos("test") < pos("review"));
    }

    /// SC-002: un ciclo en depends_on → validate Err.
    #[test]
    fn cycle_is_rejected() {
        let yaml = r#"
name: "ciclo"
tasks:
  - id: a
    title: "A"
    depends_on: [b]
  - id: b
    title: "B"
    depends_on: [a]
"#;
        let spec = parse_yaml(yaml).unwrap();
        assert!(validate(&spec).is_err());
        assert!(topo_order(&spec).is_err());
    }

    /// SC-003: depends_on a un id inexistente → Err.
    #[test]
    fn missing_dep_reference_is_rejected() {
        let yaml = r#"
name: "p"
tasks:
  - id: a
    title: "A"
    depends_on: [no_existe]
"#;
        let spec = parse_yaml(yaml).unwrap();
        assert!(validate(&spec).is_err());
    }

    /// SC-004: ids duplicados → Err.
    #[test]
    fn duplicate_ids_rejected() {
        let yaml = r#"
name: "p"
tasks:
  - id: a
    title: "A1"
  - id: a
    title: "A2"
"#;
        let spec = parse_yaml(yaml).unwrap();
        assert!(validate(&spec).is_err());
    }

    /// Auto-dependencia → Err.
    #[test]
    fn self_dependency_rejected() {
        let yaml = r#"
name: "p"
tasks:
  - id: a
    title: "A"
    depends_on: [a]
"#;
        let spec = parse_yaml(yaml).unwrap();
        assert!(validate(&spec).is_err());
    }

    /// SC-006: YAML malformado → Err claro, sin panic.
    #[test]
    fn malformed_yaml_is_error_not_panic() {
        assert!(parse_yaml("esto: no es: un pipeline: valido: [").is_err());
        // YAML válido pero sin tasks (campo requerido) → Err de parse.
        assert!(parse_yaml("name: x").is_err());
    }

    /// Pipeline vacío (0 tasks) → Err.
    #[test]
    fn empty_tasks_rejected() {
        let yaml = "name: x\ntasks: []";
        let spec = parse_yaml(yaml).unwrap();
        assert!(validate(&spec).is_err());
    }

    /// id con charset inseguro → Err.
    #[test]
    fn unsafe_id_charset_rejected() {
        let yaml = r#"
name: "p"
tasks:
  - id: "a b;rm"
    title: "A"
"#;
        let spec = parse_yaml(yaml).unwrap();
        assert!(validate(&spec).is_err());
    }

    /// Límite de tasks (>64) → Err.
    #[test]
    fn too_many_tasks_rejected() {
        let mut yaml = String::from("name: big\ntasks:\n");
        for i in 0..65 {
            yaml.push_str(&format!("  - id: t{i}\n    title: \"T{i}\"\n"));
        }
        let spec = parse_yaml(&yaml).unwrap();
        assert!(validate(&spec).is_err());
    }

    /// Council ALTA: un YAML que excede el cap de bytes se rechaza ANTES de parsear (anti-bomb).
    #[test]
    fn oversized_yaml_rejected_before_parse() {
        let huge = format!("name: x\n# {}\ntasks: []", "a".repeat(MAX_YAML_BYTES));
        assert!(parse_yaml(&huge).is_err());
    }

    /// Council MEDIA: `agent` con control chars/comillas se rechaza.
    #[test]
    fn invalid_agent_rejected() {
        let yaml = "name: p\ntasks:\n  - id: a\n    title: \"A\"\n    agent: \"ev'il\"\n";
        let spec = parse_yaml(yaml).unwrap();
        assert!(validate(&spec).is_err());
    }

    /// Un diamante (a→b, a→c, b→d, c→d) ordena con a primero y d último.
    #[test]
    fn diamond_dag_orders_correctly() {
        let yaml = r#"
name: "diamante"
tasks:
  - id: a
    title: "A"
  - id: b
    title: "B"
    depends_on: [a]
  - id: c
    title: "C"
    depends_on: [a]
  - id: d
    title: "D"
    depends_on: [b, c]
"#;
        let spec = parse_yaml(yaml).unwrap();
        validate(&spec).unwrap();
        let order = topo_order(&spec).unwrap();
        assert_eq!(order.first().unwrap(), "a");
        assert_eq!(order.last().unwrap(), "d");
    }
}
