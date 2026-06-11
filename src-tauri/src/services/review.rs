// services/review.rs — 019 F0/F1 — capa de REVIEW hunk-level (layer-on-top de orchestration).
//
// Council UNÁNIME (opción L): `orchestration.rs` es el SSOT de EJECUCIÓN (TaskGroup/OrchTask,
// worktrees, locks, lifecycle, choose/discard de variante, `collect_diff`). `review.rs` NO duplica
// nada de eso: añade SÓLO la semántica hunk-level que faltaba (la "superficie de diff/review
// unificada" del informe) — approve/reject por hunk, cross-variante, con detección de conflictos —
// sobre los diffs YA colectados, keyed por `group_id` + `task_id` (de orchestration) + `hunk_id`.
//
// PURO/testeable (sin Tauri/DB/git). La persistencia de la "review projection" (group/task/hunk →
// estado) y los comandos Tauri van en tareas aparte; acá sólo el parser de diff + el modelo de
// decisión versionado + detección de conflictos. NUNCA toca procesos/worktrees (eso es orchestration).

use anyhow::{anyhow, Result};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

type Db = Arc<parking_lot::Mutex<rusqlite::Connection>>;

/// Estado de revisión de un hunk (unidad de diff que el usuario aprueba/rechaza).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HunkState {
    Pending,
    Approved,
    Rejected,
}

/// Una unidad de diff revisable: un hunk de un archivo de una variante, con su estado.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hunk {
    /// Id único en TODO el grupo: `{task_id}:{n}` (n = índice 1-based del hunk en la variante).
    pub id: String,
    /// Archivo afectado (relativo al repo).
    pub file: String,
    /// Header unified-diff (`@@ -a,b +c,d @@`), para ubicar/renderizar el hunk.
    pub header: String,
    pub state: HunkState,
}

/// El conjunto de hunks de UNA variante (parseado de su `collect_diff`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangeSet {
    pub hunks: Vec<Hunk>,
}

/// La review de una variante (OrchTask) — su `task_id` de orchestration + sus hunks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VariantReview {
    /// `task_id` de la OrchTask en orchestration (NO se duplica el lifecycle/worktree acá).
    pub task_id: String,
    pub change_set: ChangeSet,
}

/// La review hunk-level de un best-of-N (proyección sobre el `TaskGroup` de orchestration).
/// `revision` monotónica: cada decisión la incrementa; un write stale se rechaza (FR-004).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GroupReview {
    /// `group_id` del `TaskGroup` de orchestration (identidad/lifecycle viven allá).
    pub group_id: String,
    pub revision: u64,
    pub variants: Vec<VariantReview>,
}

/// Un conflicto entre dos hunks APROBADOS de variantes DISTINTAS sobre regiones solapadas del mismo
/// archivo base. R3 (council 🔴): NO se hace merge automático — el caller exige resolución manual.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Conflict {
    pub file: String,
    pub hunk_a: String,
    pub hunk_b: String,
}

pub type ValidationError = String;

/// Quita el prefijo `a/`/`b/` de un path de diff git.
fn strip_ab(p: &str) -> String {
    let p = p.trim();
    p.strip_prefix("a/")
        .or_else(|| p.strip_prefix("b/"))
        .unwrap_or(p)
        .to_string()
}

/// Parsea un UNIFIED diff de git (de `orchestration::collect_unified_diff`) de UNA variante en Hunks
/// revisables. PURO. Detalles (audit codex 019):
///   - Trackea el lado BASE (`--- a/…`) y el NUEVO (`+++ b/…`); resetea en cada `diff --git` (límite
///     de archivo robusto, sin fugar el path del archivo anterior).
///   - El `file` del hunk = el path NUEVO; si es un DELETE (`+++ /dev/null`), usa el path BASE
///     (`--- a/…`) — así un delete y un modify del MISMO archivo conflictúan (no falso negativo).
///   - `id` CONTENT-BASED y estable: `{task_id}:{file}:{old_start},{old_count}` (del header). Estable
///     al re-parsear el mismo diff (un decision viejo no se re-asocia a otro hunk). Header no
///     parseable → respaldo ordinal `{task_id}:{file}:#{n}`.
pub fn parse_unified_diff(task_id: &str, diff: &str) -> Vec<Hunk> {
    let mut hunks = Vec::new();
    let mut base_path = String::new(); // "--- a/…"
    let mut new_path = String::new(); // "+++ b/…"
    let mut ordinal = 0u32;
    for line in diff.lines() {
        if let Some(rest) = line.strip_prefix("diff --git ") {
            base_path.clear();
            new_path.clear();
            let toks: Vec<&str> = rest.split_whitespace().collect();
            if toks.len() == 2 {
                base_path = strip_ab(toks[0]);
                new_path = strip_ab(toks[1]);
            }
        } else if let Some(rest) = line.strip_prefix("--- ") {
            base_path = if rest.trim() == "/dev/null" {
                String::new()
            } else {
                strip_ab(rest)
            };
        } else if let Some(rest) = line.strip_prefix("+++ ") {
            new_path = if rest.trim() == "/dev/null" {
                String::new()
            } else {
                strip_ab(rest)
            };
        } else if line.starts_with("@@") {
            // Path del hunk: el NUEVO; si es delete (nuevo vacío), el BASE.
            let file = if !new_path.is_empty() {
                new_path.clone()
            } else {
                base_path.clone()
            };
            ordinal += 1;
            let id = match parse_base_range(line) {
                Some((s, c)) => format!("{task_id}:{file}:{s},{c}"),
                None => format!("{task_id}:{file}:#{ordinal}"),
            };
            hunks.push(Hunk {
                id,
                file,
                header: line.to_string(),
                state: HunkState::Pending,
            });
        }
    }
    hunks
}

/// Parsea el rango del LADO BASE (`-old_start,old_count`) de un header `@@ -a,b +c,d @@`. `count`
/// por defecto 1 si se omite (formato git). `None` si no tiene la forma esperada.
fn parse_base_range(header: &str) -> Option<(u64, u64)> {
    let at = header.find("@@")?;
    let rest = &header[at + 2..];
    let minus = rest.find('-')?;
    let seg = rest[minus + 1..].split_whitespace().next()?;
    let mut it = seg.split(',');
    let start: u64 = it.next()?.trim().parse().ok()?;
    let count: u64 = match it.next() {
        Some(c) => c.trim().parse().ok()?,
        None => 1,
    };
    Some((start, count))
}

/// ¿Se solapan `[start, start+count)`? `count == 0` (inserción pura) → posición puntual.
fn ranges_overlap(a: (u64, u64), b: (u64, u64)) -> bool {
    let a_end = a.0 + a.1.max(1);
    let b_end = b.0 + b.1.max(1);
    a.0 < b_end && b.0 < a_end
}

impl GroupReview {
    /// Construye la review de un grupo a partir de los diffs YA colectados por orchestration
    /// (`collect_diff` por variante). `variants` = lista de (task_id, diff_string). `revision` 0.
    pub fn from_diffs(group_id: impl Into<String>, variants: &[(String, String)]) -> Self {
        let variants = variants
            .iter()
            .map(|(task_id, diff)| VariantReview {
                task_id: task_id.clone(),
                change_set: ChangeSet {
                    hunks: parse_unified_diff(task_id, diff),
                },
            })
            .collect();
        GroupReview {
            group_id: group_id.into(),
            revision: 0,
            variants,
        }
    }

    fn find_hunk_mut(&mut self, hunk_id: &str) -> Option<&mut Hunk> {
        self.variants
            .iter_mut()
            .flat_map(|v| v.change_set.hunks.iter_mut())
            .find(|h| h.id == hunk_id)
    }

    /// TRANSICIÓN de la decisión de un hunk (approve/reject/revert-a-pending). VERSIONADA: exige
    /// `expected_revision == self.revision` (rechaza decisiones sobre estado obsoleto, FR-004),
    /// aplica el estado e incrementa `revision`. Revertir = `decide(..., Pending)`, una acción nueva
    /// (R2; el histórico inmutable lo da el audit append-only). `Err` si el hunk no existe o stale.
    pub fn decide_hunk(
        &mut self,
        hunk_id: &str,
        new_state: HunkState,
        expected_revision: u64,
    ) -> Result<u64, ValidationError> {
        if expected_revision != self.revision {
            return Err(format!(
                "revisión obsoleta: esperaba {}, el grupo está en {}",
                expected_revision, self.revision
            ));
        }
        match self.find_hunk_mut(hunk_id) {
            Some(h) => {
                h.state = new_state;
                self.revision = self.revision.saturating_add(1);
                Ok(self.revision)
            }
            None => Err(format!("hunk no encontrado: {hunk_id}")),
        }
    }

    /// R3 — conflictos entre hunks APROBADOS de variantes DISTINTAS sobre el mismo archivo con rangos
    /// base solapados. PURA. El caller (apply) DEBE exigir resolución manual — NUNCA merge automático.
    /// Header no parseable → conflicto por precaución (no se asume que no chocan).
    pub fn detect_conflicts(&self) -> Vec<Conflict> {
        // (task_id, hunk_id, file, parsed_range) de los hunks aprobados.
        let mut approved: Vec<(&str, &str, &str, Option<(u64, u64)>)> = Vec::new();
        for v in &self.variants {
            for h in &v.change_set.hunks {
                if h.state == HunkState::Approved {
                    approved.push((&v.task_id, &h.id, &h.file, parse_base_range(&h.header)));
                }
            }
        }
        let mut out = Vec::new();
        for i in 0..approved.len() {
            for j in (i + 1)..approved.len() {
                let (ti, hi, fi, ri) = approved[i];
                let (tj, hj, fj, rj) = approved[j];
                if ti == tj || fi != fj {
                    continue; // misma variante, o archivos distintos → no conflicto
                }
                let conflict = match (ri, rj) {
                    (Some(a), Some(b)) => ranges_overlap(a, b),
                    _ => true,
                };
                if conflict {
                    out.push(Conflict {
                        file: fi.into(),
                        hunk_a: hi.into(),
                        hunk_b: hj.into(),
                    });
                }
            }
        }
        out
    }

    /// VALIDADOR PRE-PERSIST (T002, FR-004): chequea las invariantes del modelo ANTES de persistir
    /// una GroupReview construida en memoria. PURO. Detecta:
    ///   - hunk HUÉRFANO: un `id` que no respeta el formato `{task_id}:…` de su variante (un hunk de
    ///     una variante no puede llevar el prefijo de otra) → indicaría un cross-wire del parser.
    ///   - ids DUPLICADOS en todo el grupo (la PK persistida es (group_id, hunk_id); un duplicado
    ///     reventaría el INSERT o pisaría una decisión).
    ///   - `file`/`id` vacíos (un hunk sin identidad/archivo no es revisable).
    /// Devuelve `Ok(())` si el modelo es persistible; `Err(lista)` con cada violación.
    pub fn validate(&self) -> Result<(), Vec<ValidationError>> {
        let mut errs = Vec::new();
        let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
        for v in &self.variants {
            if v.task_id.is_empty() {
                errs.push("variante con task_id vacío".to_string());
            }
            let prefix = format!("{}:", v.task_id);
            for h in &v.change_set.hunks {
                if h.id.is_empty() {
                    errs.push(format!("hunk con id vacío en variante {}", v.task_id));
                }
                if h.file.is_empty() {
                    errs.push(format!("hunk {} sin archivo (huérfano)", h.id));
                }
                // El id debe pertenecer a SU variante (`{task_id}:…`) — sino es un hunk huérfano
                // (mapeado a la variante equivocada).
                if !v.task_id.is_empty() && !h.id.starts_with(&prefix) {
                    errs.push(format!(
                        "hunk huérfano: id '{}' no pertenece a la variante '{}'",
                        h.id, v.task_id
                    ));
                }
                if !seen.insert(h.id.as_str()) {
                    errs.push(format!("hunk id duplicado en el grupo: {}", h.id));
                }
            }
        }
        if errs.is_empty() {
            Ok(())
        } else {
            Err(errs)
        }
    }

    /// Hunks aprobados (para que el caller los aplique tras resolver conflictos). Read-only.
    pub fn approved_hunks(&self) -> Vec<&Hunk> {
        self.variants
            .iter()
            .flat_map(|v| v.change_set.hunks.iter())
            .filter(|h| h.state == HunkState::Approved)
            .collect()
    }
}

/// Un hunk con su CUERPO completo (header `@@` + líneas ` `/`+`/`-`), para construir el patch de
/// apply. Se re-deriva del diff en apply-time; NO se persiste (los cuerpos son grandes). `old_start`
/// se usa para ordenar los hunks de un archivo al combinarlos.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HunkWithBody {
    pub id: String,
    pub file: String,
    pub old_start: u64,
    /// Header + líneas del hunk, tal cual del diff (sin el trailing newline final agregado al unir).
    pub body: String,
}

/// Parsea un UNIFIED diff conservando el CUERPO de cada hunk (mismos ids que `parse_unified_diff`).
/// El cuerpo = el header `@@` + todas sus líneas hasta el próximo `@@`/`diff --git`/`--- `/EOF. PURO.
pub fn parse_hunks_with_body(task_id: &str, diff: &str) -> Vec<HunkWithBody> {
    // Primero ubicamos los hunks (id/file/header) con el parser canónico (misma lógica de ids/paths).
    let metas = parse_unified_diff(task_id, diff);
    if metas.is_empty() {
        return Vec::new();
    }
    // Recorremos las líneas acumulando el cuerpo de cada hunk (desde su `@@` hasta el próximo límite).
    let lines: Vec<&str> = diff.lines().collect();
    let mut out: Vec<HunkWithBody> = Vec::new();
    let mut meta_idx = 0usize;
    let mut i = 0usize;
    while i < lines.len() && meta_idx < metas.len() {
        if lines[i].starts_with("@@") {
            // cuerpo desde aquí hasta el próximo @@/diff --git/--- (nuevo archivo)/EOF.
            let start = i;
            let mut j = i + 1;
            while j < lines.len()
                && !lines[j].starts_with("@@")
                && !lines[j].starts_with("diff --git ")
                && !lines[j].starts_with("--- ")
            {
                j += 1;
            }
            let body = lines[start..j].join("\n");
            let m = &metas[meta_idx];
            let old_start = parse_base_range(lines[start]).map(|(s, _)| s).unwrap_or(0);
            out.push(HunkWithBody {
                id: m.id.clone(),
                file: m.file.clone(),
                old_start,
                body,
            });
            meta_idx += 1;
            i = j;
        } else {
            i += 1;
        }
    }
    out
}

/// Construye un patch unified APLICABLE con `git apply` que contiene SÓLO los hunks cuyo id está en
/// `approved`, agrupados por archivo (un bloque `--- a/f` / `+++ b/f` por archivo) y ordenados por
/// `old_start` ascendente (git apply recalcula offsets acumulados dentro de un archivo). PURO. El
/// caller corre `git apply` sobre Main DESPUÉS de chequear `detect_conflicts` (sin solapamientos).
/// `variants` = (task_id, unified_diff) de cada variante. Devuelve "" si no hay hunks aprobados.
pub fn build_apply_patch(
    variants: &[(String, String)],
    approved: &std::collections::HashSet<String>,
) -> String {
    let hunks: Vec<HunkWithBody> = variants
        .iter()
        .flat_map(|(task_id, diff)| parse_hunks_with_body(task_id, diff))
        .filter(|h| approved.contains(&h.id))
        .collect();
    build_patch_from_hunks(&hunks)
}

/// Emite un patch unified APLICABLE (`git apply`) desde hunks-con-cuerpo (de `load_approved_with_bodies`,
/// el snapshot persistido — anti-drift, codex #1), agrupados por archivo y ordenados por `old_start`
/// (git apply ajusta offsets acumulados dentro de un archivo). PURO. "" si no hay hunks.
pub fn build_patch_from_hunks(hunks: &[HunkWithBody]) -> String {
    let mut by_file: std::collections::BTreeMap<&str, Vec<&HunkWithBody>> =
        std::collections::BTreeMap::new();
    for h in hunks {
        by_file.entry(h.file.as_str()).or_default().push(h);
    }
    if by_file.is_empty() {
        return String::new();
    }
    let mut patch = String::new();
    for (file, mut hs) in by_file {
        hs.sort_by_key(|h| h.old_start);
        patch.push_str(&format!("--- a/{file}\n+++ b/{file}\n"));
        for h in &hs {
            patch.push_str(&h.body);
            if !h.body.ends_with('\n') {
                patch.push('\n');
            }
        }
    }
    patch
}

impl HunkState {
    fn as_str(self) -> &'static str {
        match self {
            HunkState::Pending => "pending",
            HunkState::Approved => "approved",
            HunkState::Rejected => "rejected",
        }
    }
    pub fn from_str(s: &str) -> Result<HunkState> {
        match s {
            "pending" => Ok(HunkState::Pending),
            "approved" => Ok(HunkState::Approved),
            "rejected" => Ok(HunkState::Rejected),
            other => Err(anyhow!("estado de hunk inválido: {other}")),
        }
    }
}

// ── Persistencia de la review projection (migración 032) ───────────────────────────────────────
// Estado MUTABLE de la decisión por hunk; la historia inmutable la da el audit append-only (R2).
// `revision` por grupo = optimistic concurrency (FR-004): save_decision rechaza writes stale.

/// Persiste la review INICIAL de un grupo (tras parsear los diffs de las variantes). Idempotente
/// por (group_id, hunk_id): re-init no pisa decisiones existentes (INSERT OR IGNORE). Transaccional.
pub fn init_group(db: &Db, group_id: &str, variants: &[(String, String)]) -> Result<()> {
    let mut conn = db.lock();
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    tx.execute(
        "INSERT OR IGNORE INTO review_groups (group_id, revision) VALUES (?1, 0)",
        params![group_id],
    )?;
    for (task_id, diff) in variants {
        // FIRST-WRITE-WINS POR VARIANTE (audit codex #3): si la variante YA tiene hunks en este grupo,
        // NO la re-insertamos — su diff/cuerpo es fijo una vez que la variante terminó; re-init con un
        // diff distinto NO debe mezclar snapshots. Las decisiones existentes quedan intactas.
        let existing: i64 = tx.query_row(
            "SELECT COUNT(*) FROM review_hunks WHERE group_id = ?1 AND task_id = ?2",
            params![group_id, task_id],
            |r| r.get(0),
        )?;
        if existing > 0 {
            continue;
        }
        // SNAPSHOT del cuerpo (audit codex #1): guardamos el cuerpo del hunk para aplicar EXACTAMENTE
        // lo aprobado, sin re-derivar del worktree vivo (que podría haber cambiado).
        for h in parse_hunks_with_body(task_id, diff) {
            let header = h.body.lines().next().unwrap_or("").to_string();
            tx.execute(
                "INSERT INTO review_hunks (group_id, task_id, hunk_id, file, header, state, body) \
                 VALUES (?1,?2,?3,?4,?5,'pending',?6)",
                params![group_id, task_id, h.id, h.file, header, h.body],
            )?;
        }
    }
    tx.commit()?;
    Ok(())
}

/// Hunks APROBADOS de un grupo CON su cuerpo persistido (snapshot al abrir la review), para apply.
/// El patch se construye SIEMPRE desde estos cuerpos guardados — NUNCA re-derivando del worktree
/// vivo (audit codex #1: el worktree pudo cambiar el cuerpo del mismo hunk tras la revisión).
pub fn load_approved_with_bodies(db: &Db, group_id: &str) -> Result<Vec<HunkWithBody>> {
    let conn = db.lock();
    let mut stmt = conn.prepare(
        "SELECT hunk_id, file, header, body FROM review_hunks \
         WHERE group_id = ?1 AND state = 'approved' ORDER BY file, hunk_id",
    )?;
    let rows = stmt.query_map(params![group_id], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, String>(3)?,
        ))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (id, file, header, body) = row?;
        let old_start = parse_base_range(&header).map(|(s, _)| s).unwrap_or(0);
        out.push(HunkWithBody {
            id,
            file,
            old_start,
            body,
        });
    }
    Ok(out)
}

/// Carga la review de un grupo desde la DB → `GroupReview` (hunks agrupados por `task_id` en
/// variantes, orden estable por task_id+hunk_id). `None` si el grupo no existe.
pub fn load_group(db: &Db, group_id: &str) -> Result<Option<GroupReview>> {
    let conn = db.lock();
    let rev: Option<i64> = conn
        .query_row(
            "SELECT revision FROM review_groups WHERE group_id = ?1",
            params![group_id],
            |r| r.get(0),
        )
        .optional()?;
    let Some(rev) = rev else { return Ok(None) };
    let revision = u64::try_from(rev).map_err(|_| {
        anyhow!("review_groups.revision negativa ({rev}) para {group_id} — corrupto")
    })?;
    let mut stmt = conn.prepare(
        "SELECT task_id, hunk_id, file, header, state FROM review_hunks \
         WHERE group_id = ?1 ORDER BY task_id, hunk_id",
    )?;
    let rows = stmt.query_map(params![group_id], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, String>(3)?,
            r.get::<_, String>(4)?,
        ))
    })?;
    // Agrupar por task_id preservando el orden de aparición.
    let mut variants: Vec<VariantReview> = Vec::new();
    for row in rows {
        let (task_id, hunk_id, file, header, state) = row?;
        let h = Hunk {
            id: hunk_id,
            file,
            header,
            state: HunkState::from_str(&state)?,
        };
        match variants.iter_mut().find(|v| v.task_id == task_id) {
            Some(v) => v.change_set.hunks.push(h),
            None => variants.push(VariantReview {
                task_id,
                change_set: ChangeSet { hunks: vec![h] },
            }),
        }
    }
    Ok(Some(GroupReview {
        group_id: group_id.to_string(),
        revision,
        variants,
    }))
}

/// Persiste UNA decisión de hunk, VERSIONADA (FR-004): transaccional Immediate; lee la revisión
/// actual; si `!= expected_revision` → rechaza (stale, sin escribir); UPDATE del estado del hunk +
/// `revision+1`. Devuelve la nueva revisión. `Err` si el hunk/grupo no existe o la revisión es stale.
/// El audit (append-only) lo escribe el caller como acción aparte (R2).
/// Resultado de `save_decision`: el estado PREVIO del hunk (para auditar la transición correcta,
/// audit-3 L3) + la nueva revisión del grupo.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DecisionSaved {
    /// El estado del hunk ANTES de esta decisión (leído atómicamente dentro de la misma tx).
    pub previous: HunkState,
    /// La nueva revisión del grupo tras aplicar la decisión.
    pub revision: u64,
}

pub fn save_decision(
    db: &Db,
    group_id: &str,
    hunk_id: &str,
    new_state: HunkState,
    expected_revision: u64,
) -> Result<DecisionSaved> {
    let mut conn = db.lock();
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let stored: Option<i64> = tx
        .query_row(
            "SELECT revision FROM review_groups WHERE group_id = ?1",
            params![group_id],
            |r| r.get(0),
        )
        .optional()?;
    let Some(stored) = stored else {
        return Err(anyhow!("grupo de review no encontrado: {group_id}"));
    };
    let stored = u64::try_from(stored)
        .map_err(|_| anyhow!("review_groups.revision negativa ({stored}) — corrupto"))?;
    if stored != expected_revision {
        return Err(anyhow!(
            "revisión obsoleta: esperaba {expected_revision}, está en {stored}"
        ));
    }
    // Leer el estado PREVIO del hunk DENTRO de la tx (atómico vs decisiones concurrentes, audit-3
    // L3): la transición auditada se deriva de (previous, new), no se asume `Pending → revert`.
    let previous: Option<String> = tx
        .query_row(
            "SELECT state FROM review_hunks WHERE group_id = ?1 AND hunk_id = ?2",
            params![group_id, hunk_id],
            |r| r.get(0),
        )
        .optional()?;
    let Some(previous) = previous else {
        return Err(anyhow!("hunk no encontrado: {hunk_id} (grupo {group_id})"));
    };
    let previous = HunkState::from_str(&previous)?;
    let n = tx.execute(
        "UPDATE review_hunks SET state = ?1 WHERE group_id = ?2 AND hunk_id = ?3",
        params![new_state.as_str(), group_id, hunk_id],
    )?;
    if n == 0 {
        return Err(anyhow!("hunk no encontrado: {hunk_id} (grupo {group_id})"));
    }
    let new_rev = stored.saturating_add(1);
    tx.execute(
        "UPDATE review_groups SET revision = ?1, updated_at = datetime('now') WHERE group_id = ?2",
        params![new_rev as i64, group_id],
    )?;
    tx.commit()?;
    Ok(DecisionSaved {
        previous,
        revision: new_rev,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const DIFF_A: &str = "\
diff --git a/src/a.rs b/src/a.rs
--- a/src/a.rs
+++ b/src/a.rs
@@ -10,5 +10,6 @@
 ctx
-old
+new
@@ -30,2 +31,3 @@
 ctx2
+added
diff --git a/src/b.rs b/src/b.rs
--- a/src/b.rs
+++ b/src/b.rs
@@ -1,1 +1,2 @@
+line
";

    #[test]
    fn parses_diff_into_hunks_with_files_and_ids() {
        let hunks = parse_unified_diff("t1", DIFF_A);
        assert_eq!(hunks.len(), 3);
        // ids CONTENT-BASED (estables): {task}:{file}:{old_start},{old_count}.
        assert_eq!(hunks[0].id, "t1:src/a.rs:10,5");
        assert_eq!(hunks[0].file, "src/a.rs");
        assert_eq!(hunks[0].header, "@@ -10,5 +10,6 @@");
        assert_eq!(hunks[1].id, "t1:src/a.rs:30,2");
        assert_eq!(hunks[1].file, "src/a.rs");
        assert_eq!(hunks[2].id, "t1:src/b.rs:1,1");
        assert_eq!(hunks[2].file, "src/b.rs");
        assert!(hunks.iter().all(|h| h.state == HunkState::Pending));
        // ESTABILIDAD: re-parsear el mismo diff da los MISMOS ids (audit #4).
        let again = parse_unified_diff("t1", DIFF_A);
        assert_eq!(hunks, again);
    }

    #[test]
    fn delete_hunk_attributed_to_base_path_not_dev_null() {
        // audit #2: un delete (`+++ /dev/null`) debe atribuirse al archivo BASE, no a /dev/null,
        // para que un delete y un modify del MISMO archivo conflictúen.
        let del = "diff --git a/src/x.rs b/src/x.rs\n--- a/src/x.rs\n+++ /dev/null\n@@ -1,3 +0,0 @@\n-a\n-b\n-c\n";
        let hunks = parse_unified_diff("t1", del);
        assert_eq!(hunks.len(), 1);
        assert_eq!(
            hunks[0].file, "src/x.rs",
            "delete atribuido al path base, no /dev/null"
        );
    }

    #[test]
    fn empty_or_no_hunk_diff_yields_empty() {
        assert!(parse_unified_diff("t1", "").is_empty());
        assert!(parse_unified_diff("t1", "diff --git a/x b/x\n(binary)\n").is_empty());
    }

    fn group_two() -> GroupReview {
        GroupReview::from_diffs(
            "g1",
            &[
                ("t1".into(), DIFF_A.to_string()),
                (
                    "t2".into(),
                    "--- a/src/a.rs\n+++ b/src/a.rs\n@@ -10,5 +10,6 @@\n+x\n".to_string(),
                ),
            ],
        )
    }

    #[test]
    fn from_diffs_builds_variants_keyed_by_task() {
        let g = group_two();
        assert_eq!(g.revision, 0);
        assert_eq!(g.variants.len(), 2);
        assert_eq!(g.variants[0].task_id, "t1");
        assert_eq!(g.variants[0].change_set.hunks.len(), 3);
        assert_eq!(g.variants[1].task_id, "t2");
        assert_eq!(g.variants[1].change_set.hunks.len(), 1);
        assert_eq!(g.variants[1].change_set.hunks[0].id, "t2:src/a.rs:10,5");
    }

    #[test]
    fn decide_hunk_versioned_and_bumps_revision() {
        let mut g = group_two();
        let rev = g
            .decide_hunk("t1:src/a.rs:10,5", HunkState::Approved, 0)
            .expect("ok");
        assert_eq!(rev, 1);
        // stale → rechazo.
        assert!(g
            .decide_hunk("t2:src/a.rs:10,5", HunkState::Approved, 0)
            .is_err());
        // correcta → ok.
        assert!(g
            .decide_hunk("t2:src/a.rs:10,5", HunkState::Rejected, 1)
            .is_ok());
        assert_eq!(g.revision, 2);
    }

    #[test]
    fn revert_is_new_transition_to_pending() {
        let mut g = group_two();
        g.decide_hunk("t1:src/a.rs:10,5", HunkState::Approved, 0)
            .unwrap();
        let rev = g
            .decide_hunk("t1:src/a.rs:10,5", HunkState::Pending, 1)
            .expect("revert ok");
        assert_eq!(rev, 2);
    }

    #[test]
    fn decide_missing_hunk_errs() {
        let mut g = group_two();
        assert!(g.decide_hunk("ghost", HunkState::Approved, 0).is_err());
    }

    #[test]
    fn conflict_when_two_variants_approve_overlapping_same_file() {
        // t1:1 (src/a.rs @@ -10,5) y t2:1 (src/a.rs @@ -10,5) → mismo archivo, rangos solapados.
        let mut g = group_two();
        g.decide_hunk("t1:src/a.rs:10,5", HunkState::Approved, 0)
            .unwrap();
        g.decide_hunk("t2:src/a.rs:10,5", HunkState::Approved, 1)
            .unwrap();
        let c = g.detect_conflicts();
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].file, "src/a.rs");
    }

    #[test]
    fn no_conflict_within_same_variant_or_disjoint() {
        // t1:1 y t1:2 son de la MISMA variante → no conflicto aunque ambos aprobados.
        let mut g = group_two();
        g.decide_hunk("t1:src/a.rs:10,5", HunkState::Approved, 0)
            .unwrap();
        g.decide_hunk("t1:src/a.rs:30,2", HunkState::Approved, 1)
            .unwrap();
        assert!(g.detect_conflicts().is_empty());
    }

    #[test]
    fn approved_hunks_lists_only_approved() {
        let mut g = group_two();
        g.decide_hunk("t1:src/a.rs:10,5", HunkState::Approved, 0)
            .unwrap();
        g.decide_hunk("t1:src/a.rs:30,2", HunkState::Rejected, 1)
            .unwrap();
        let ap = g.approved_hunks();
        assert_eq!(ap.len(), 1);
        assert_eq!(ap[0].id, "t1:src/a.rs:10,5");
    }

    #[test]
    fn parse_base_range_forms() {
        assert_eq!(parse_base_range("@@ -10,5 +10,6 @@"), Some((10, 5)));
        assert_eq!(parse_base_range("@@ -7 +7,2 @@"), Some((7, 1)));
        assert_eq!(parse_base_range("nope"), None);
    }

    #[test]
    fn validate_accepts_well_formed_group() {
        // T002 — un grupo construido por el parser canónico SIEMPRE valida (ids con prefijo correcto,
        // sin duplicados, file/id no vacíos).
        let g = group_two();
        assert!(g.validate().is_ok(), "el grupo del parser debe validar");
    }

    #[test]
    fn validate_catches_orphan_and_duplicate_hunks() {
        // T002 — construimos a mano una GroupReview INVÁLIDA (no via el parser) para probar el guard.
        let g = GroupReview {
            group_id: "g1".into(),
            revision: 0,
            variants: vec![
                VariantReview {
                    task_id: "t1".into(),
                    change_set: ChangeSet {
                        hunks: vec![
                            // huérfano: el id lleva el prefijo de OTRA variante (t2).
                            Hunk {
                                id: "t2:src/a.rs:1,1".into(),
                                file: "src/a.rs".into(),
                                header: "@@ -1,1 +1,2 @@".into(),
                                state: HunkState::Pending,
                            },
                            // sin archivo.
                            Hunk {
                                id: "t1:nofile:1,1".into(),
                                file: String::new(),
                                header: "@@ -1,1 +1,1 @@".into(),
                                state: HunkState::Pending,
                            },
                        ],
                    },
                },
                VariantReview {
                    task_id: "t2".into(),
                    change_set: ChangeSet {
                        // DUPLICADO del id que ya apareció arriba.
                        hunks: vec![Hunk {
                            id: "t2:src/a.rs:1,1".into(),
                            file: "src/a.rs".into(),
                            header: "@@ -1,1 +1,2 @@".into(),
                            state: HunkState::Pending,
                        }],
                    },
                },
            ],
        };
        let errs = g.validate().expect_err("debe fallar");
        // al menos: huérfano (t2:… en t1), file vacío, duplicado.
        assert!(errs.iter().any(|e| e.contains("huérfano")), "{errs:?}");
        assert!(errs.iter().any(|e| e.contains("sin archivo")), "{errs:?}");
        assert!(errs.iter().any(|e| e.contains("duplicado")), "{errs:?}");
    }

    // ── Persistencia (migraciones 032 + 033) ───────────────────────────────────────────────────
    fn test_db() -> Db {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(include_str!("../../migrations/032_review_projection.sql"))
            .unwrap();
        conn.execute_batch(include_str!("../../migrations/033_review_hunk_body.sql"))
            .unwrap();
        Arc::new(parking_lot::Mutex::new(conn))
    }

    /// Las MISMAS variantes (task_id, diff) que `group_two`, para alimentar `init_group`.
    fn variants_two() -> Vec<(String, String)> {
        vec![
            ("t1".into(), DIFF_A.to_string()),
            (
                "t2".into(),
                "--- a/src/a.rs\n+++ b/src/a.rs\n@@ -10,5 +10,6 @@\n+x\n".to_string(),
            ),
        ]
    }

    #[test]
    fn persistence_roundtrip_and_versioned_decisions() {
        let db = test_db();
        init_group(&db, "g1", &variants_two()).unwrap();

        // load reconstruye las variantes keyed por task_id.
        let loaded = load_group(&db, "g1").unwrap().expect("grupo existe");
        assert_eq!(loaded.revision, 0);
        assert_eq!(loaded.variants.len(), 2);
        assert_eq!(loaded.variants[0].task_id, "t1");
        assert_eq!(loaded.variants[0].change_set.hunks.len(), 3);
        assert!(loaded
            .variants
            .iter()
            .flat_map(|v| &v.change_set.hunks)
            .all(|h| h.state == HunkState::Pending));

        // save_decision versionado: approve t1:1 con rev 0 → rev 1. Previous = Pending (inicial).
        let saved = save_decision(&db, "g1", "t1:src/a.rs:10,5", HunkState::Approved, 0).unwrap();
        assert_eq!(saved.revision, 1);
        assert_eq!(saved.previous, HunkState::Pending);
        // write stale (rev 0 otra vez) → rechazo.
        assert!(save_decision(&db, "g1", "t2:src/a.rs:10,5", HunkState::Approved, 0).is_err());
        // rev correcta (1) → ok.
        assert_eq!(
            save_decision(&db, "g1", "t2:src/a.rs:10,5", HunkState::Rejected, 1)
                .unwrap()
                .revision,
            2
        );

        // recarga refleja los estados + la revisión.
        let l2 = load_group(&db, "g1").unwrap().unwrap();
        assert_eq!(l2.revision, 2);
        let h1 = l2
            .variants
            .iter()
            .flat_map(|v| &v.change_set.hunks)
            .find(|h| h.id == "t1:src/a.rs:10,5")
            .unwrap();
        assert_eq!(h1.state, HunkState::Approved);
        let h2 = l2
            .variants
            .iter()
            .flat_map(|v| &v.change_set.hunks)
            .find(|h| h.id == "t2:src/a.rs:10,5")
            .unwrap();
        assert_eq!(h2.state, HunkState::Rejected);
    }

    #[test]
    fn load_missing_group_is_none() {
        let db = test_db();
        assert!(load_group(&db, "ghost").unwrap().is_none());
    }

    #[test]
    fn save_decision_rejects_missing_group_or_hunk() {
        let db = test_db();
        // grupo inexistente.
        assert!(save_decision(&db, "ghost", "t1:src/a.rs:10,5", HunkState::Approved, 0).is_err());
        // grupo existe pero hunk no.
        init_group(&db, "g1", &variants_two()).unwrap();
        assert!(save_decision(&db, "g1", "nope:9", HunkState::Approved, 0).is_err());
    }

    #[test]
    fn save_decision_reports_previous_state_for_audit() {
        // audit-3 L3: la transición auditada se deriva de (previous, new). Un hunk inicial está
        // Pending; volver a Pending NO es un revert (previous == Pending). Sólo deshacer una decisión
        // (Approved/Rejected → Pending) es revert. `save_decision` reporta el estado previo para que
        // el caller (review_hunk_decide) distinga.
        let db = test_db();
        init_group(&db, "g1", &variants_two()).unwrap();
        let hunk = "t1:src/a.rs:10,5";
        // inicial Pending → Pending: previous == Pending → el caller NO lo audita como revert.
        let s = save_decision(&db, "g1", hunk, HunkState::Pending, 0).unwrap();
        assert_eq!(s.previous, HunkState::Pending, "estado inicial");
        // Pending → Approved: previous == Pending.
        let s = save_decision(&db, "g1", hunk, HunkState::Approved, s.revision).unwrap();
        assert_eq!(s.previous, HunkState::Pending);
        // Approved → Pending: previous == Approved → ESTO sí es un revert real.
        let s = save_decision(&db, "g1", hunk, HunkState::Pending, s.revision).unwrap();
        assert_eq!(
            s.previous,
            HunkState::Approved,
            "deshacer una decisión previa = revert"
        );
    }

    #[test]
    fn apply_uses_persisted_body_snapshot() {
        // audit codex #1: el patch de apply sale del CUERPO persistido al abrir la review, NO del
        // worktree vivo. init_group guarda el cuerpo; load_approved_with_bodies lo devuelve; el patch
        // contiene esas líneas exactas aunque el worktree haya cambiado después.
        let db = test_db();
        init_group(&db, "g1", &variants_two()).unwrap();
        // aprobar el primer hunk de t1 (src/a.rs:10,5 — body con "-old"/"+new").
        save_decision(&db, "g1", "t1:src/a.rs:10,5", HunkState::Approved, 0).unwrap();
        let approved = load_approved_with_bodies(&db, "g1").unwrap();
        assert_eq!(approved.len(), 1);
        assert_eq!(approved[0].file, "src/a.rs");
        assert!(
            approved[0].body.contains("-old") && approved[0].body.contains("+new"),
            "cuerpo persistido"
        );
        let patch = build_patch_from_hunks(&approved);
        assert!(patch.contains("--- a/src/a.rs\n+++ b/src/a.rs\n"));
        assert!(patch.contains("@@ -10,5 +10,6 @@"));
        assert!(patch.contains("+new"));
    }

    #[test]
    fn init_group_is_idempotent_preserving_decisions() {
        let db = test_db();
        init_group(&db, "g1", &variants_two()).unwrap();
        save_decision(&db, "g1", "t1:src/a.rs:10,5", HunkState::Approved, 0).unwrap();
        // re-init NO debe pisar la decisión existente (INSERT OR IGNORE).
        init_group(&db, "g1", &variants_two()).unwrap();
        let l = load_group(&db, "g1").unwrap().unwrap();
        let h1 = l
            .variants
            .iter()
            .flat_map(|v| &v.change_set.hunks)
            .find(|h| h.id == "t1:src/a.rs:10,5")
            .unwrap();
        assert_eq!(h1.state, HunkState::Approved, "re-init no pisa la decisión");
    }

    // ── apply: parse-con-cuerpo + build_apply_patch ────────────────────────────────────────────
    #[test]
    fn parse_with_body_captures_hunk_lines() {
        let hb = parse_hunks_with_body("t1", DIFF_A);
        assert_eq!(hb.len(), 3);
        // mismos ids que el parser liviano.
        assert_eq!(hb[0].id, "t1:src/a.rs:10,5");
        assert_eq!(hb[0].old_start, 10);
        // el cuerpo arranca con el header y contiene las líneas del hunk.
        assert!(hb[0].body.starts_with("@@ -10,5 +10,6 @@"));
        assert!(hb[0].body.contains("-old"));
        assert!(hb[0].body.contains("+new"));
        // no se cuela contenido del próximo hunk.
        assert!(!hb[0].body.contains("+added"));
        assert_eq!(hb[2].file, "src/b.rs");
        assert!(hb[2].body.contains("+line"));
    }

    #[test]
    fn build_apply_patch_only_approved_grouped_by_file() {
        use std::collections::HashSet;
        let variants = vec![("t1".to_string(), DIFF_A.to_string())];
        // aprobar sólo el primer hunk de src/a.rs y el de src/b.rs (NO el segundo de a.rs).
        let approved: HashSet<String> = ["t1:src/a.rs:10,5", "t1:src/b.rs:1,1"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let patch = build_apply_patch(&variants, &approved);
        // headers por archivo.
        assert!(patch.contains("--- a/src/a.rs\n+++ b/src/a.rs\n"));
        assert!(patch.contains("--- a/src/b.rs\n+++ b/src/b.rs\n"));
        // contiene el hunk aprobado de a.rs y NO el rechazado (-30,2).
        assert!(patch.contains("@@ -10,5 +10,6 @@"));
        assert!(!patch.contains("@@ -30,2"));
        // contiene el de b.rs.
        assert!(patch.contains("+line"));
    }

    #[test]
    fn build_apply_patch_empty_when_none_approved() {
        use std::collections::HashSet;
        let variants = vec![("t1".to_string(), DIFF_A.to_string())];
        assert!(build_apply_patch(&variants, &HashSet::new()).is_empty());
    }

    #[test]
    fn build_apply_patch_orders_hunks_by_old_start() {
        use std::collections::HashSet;
        // un diff con hunks fuera de orden NO ocurre en git, pero el builder debe ordenarlos igual.
        let variants = vec![("t1".to_string(), DIFF_A.to_string())];
        let approved: HashSet<String> = ["t1:src/a.rs:10,5", "t1:src/a.rs:30,2"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let patch = build_apply_patch(&variants, &approved);
        let p10 = patch.find("@@ -10,5").unwrap();
        let p30 = patch.find("@@ -30,2").unwrap();
        assert!(p10 < p30, "hunks ordenados por old_start ascendente");
    }
}
